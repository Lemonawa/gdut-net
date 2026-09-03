use std::net::Ipv4Addr;

#[cfg(windows)]
use anyhow::Result;

const VIRTUAL_KEYWORDS: &[&str] = &[
    "wintun",
    "tun",
    "tap",
    "tailscale",
    "clash",
    "wireguard",
    "hyper-v",
    "vmware",
    "virtualbox",
    "vethernet",
    "loopback",
    "wan miniport",
];

/// 判定适配器名称或描述是否为虚拟网卡（大小写不敏感）。
pub fn is_virtual(name_or_desc: &str) -> bool {
    let lower = name_or_desc.to_lowercase();
    VIRTUAL_KEYWORDS.iter().any(|k| lower.contains(k))
}

/// 一个可用网络出口：适配器名 + IPv4 + 可选网关。
#[derive(Debug, Clone)]
pub struct AdapterInfo {
    pub name: String,
    pub ipv4: Ipv4Addr,
    pub gateway: Option<Ipv4Addr>,
}

#[cfg(windows)]
mod win {
    use std::mem::size_of;
    use std::net::Ipv4Addr;

    use anyhow::{anyhow, Result};
    use windows::core::PWSTR;
    use windows::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_INCLUDE_GATEWAYS, IF_TYPE_ETHERNET_CSMACD, IF_TYPE_PPP,
        IP_ADAPTER_ADDRESSES_LH, IP_ADAPTER_GATEWAY_ADDRESS_LH, IP_ADAPTER_UNICAST_ADDRESS_LH,
    };
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN, SOCKET_ADDRESS};

    use super::AdapterInfo;

    /// GetAdaptersAddresses 两次调用法取出的单个适配器原始信息。
    pub(super) struct RawAdapter {
        pub name: String,
        pub desc: String,
        pub ipv4: Option<Ipv4Addr>,
        pub gateway: Option<Ipv4Addr>,
    }

    /// 未启用 is_virtual 过滤前的选择条件（按 IfType/OperStatus 等）。
    pub(super) type Selector = dyn Fn(&IP_ADAPTER_ADDRESSES_LH) -> bool;

    fn pwstr_to_string(p: PWSTR) -> String {
        if p.0.is_null() {
            String::new()
        } else {
            unsafe { p.to_string().unwrap_or_default() }
        }
    }

    /// 从 SOCKET_ADDRESS 中取 IPv4，仅接受 AF_INET。
    fn sockaddr_ipv4(sa: &SOCKET_ADDRESS) -> Option<Ipv4Addr> {
        if sa.lpSockaddr.is_null() || sa.iSockaddrLength < size_of::<SOCKADDR_IN>() as i32 {
            return None;
        }
        let sa_in = unsafe { &*(sa.lpSockaddr as *const SOCKADDR_IN) };
        if sa_in.sin_family != AF_INET {
            return None;
        }
        Some(sa_in.sin_addr.into())
    }

    fn unicast_ipv4(a: &IP_ADAPTER_ADDRESSES_LH) -> Option<Ipv4Addr> {
        let mut node: *mut IP_ADAPTER_UNICAST_ADDRESS_LH = a.FirstUnicastAddress;
        while !node.is_null() {
            if let Some(ip) = sockaddr_ipv4(unsafe { &(*node).Address }) {
                return Some(ip);
            }
            node = unsafe { (*node).Next };
        }
        None
    }

    fn gateway_ipv4(a: &IP_ADAPTER_ADDRESSES_LH) -> Option<Ipv4Addr> {
        let mut node: *mut IP_ADAPTER_GATEWAY_ADDRESS_LH = a.FirstGatewayAddress;
        while !node.is_null() {
            if let Some(ip) = sockaddr_ipv4(unsafe { &(*node).Address }) {
                return Some(ip);
            }
            node = unsafe { (*node).Next };
        }
        None
    }

    /// GetAdaptersAddresses 两次调用法：先探缓冲区大小再正式取。
    pub(super) fn adapters(selector: &Selector) -> Result<Vec<RawAdapter>> {
        let mut size: u32 = 15 * 1024;
        let mut buf: Vec<u8> = vec![0; size as usize];
        let mut ret = unsafe {
            GetAdaptersAddresses(
                AF_INET.0 as u32,
                GAA_FLAG_INCLUDE_GATEWAYS,
                None,
                Some(buf.as_mut_ptr().cast()),
                &mut size,
            )
        };
        if ret == windows::Win32::Foundation::ERROR_BUFFER_OVERFLOW.0 {
            buf = vec![0; size as usize];
            ret = unsafe {
                GetAdaptersAddresses(
                    AF_INET.0 as u32,
                    GAA_FLAG_INCLUDE_GATEWAYS,
                    None,
                    Some(buf.as_mut_ptr().cast()),
                    &mut size,
                )
            };
        }
        if ret != 0 {
            return Err(anyhow!("GetAdaptersAddresses 失败: 错误码 {ret}"));
        }

        let mut out = Vec::new();
        let mut node = buf.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        while !node.is_null() {
            let a = unsafe { &*node };
            if selector(a) {
                out.push(RawAdapter {
                    name: pwstr_to_string(a.FriendlyName),
                    desc: pwstr_to_string(a.Description),
                    ipv4: unicast_ipv4(a),
                    gateway: gateway_ipv4(a),
                });
            }
            node = a.Next;
        }
        Ok(out)
    }

    /// 物理以太网：IF_TYPE_ETHERNET_CSMACD + OperStatus Up + 非虚拟，优先有网关者。
    pub(super) fn physical_adapter() -> Result<AdapterInfo> {
        let selector = |a: &IP_ADAPTER_ADDRESSES_LH| {
            a.IfType == IF_TYPE_ETHERNET_CSMACD && a.OperStatus == IfOperStatusUp
        };
        let mut candidates: Vec<AdapterInfo> = adapters(&selector)?
            .into_iter()
            .filter(|a| !super::is_virtual(&a.name) && !super::is_virtual(&a.desc))
            .filter_map(|a| {
                a.ipv4.map(|ipv4| AdapterInfo {
                    name: a.name,
                    ipv4,
                    gateway: a.gateway,
                })
            })
            .collect();
        // 稳定排序：有网关者排前，同序保持原链表顺序
        candidates.sort_by_key(|a| a.gateway.is_none());
        candidates
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("未找到可用的物理以太网适配器"))
    }

    /// PPPoE 会话适配器（IF_TYPE_PPP）的 IPv4。
    pub(super) fn ppp_adapter_ip() -> Option<Ipv4Addr> {
        let selector = |a: &IP_ADAPTER_ADDRESSES_LH| a.IfType == IF_TYPE_PPP;
        adapters(&selector).ok()?.into_iter().find_map(|a| a.ipv4)
    }

    /// PPPoE 会话适配器完整信息（探测应绑会话口：校园网隔离 DHCP 口与 PPP 口）。
    pub(super) fn ppp_adapter() -> Option<AdapterInfo> {
        let selector = |a: &IP_ADAPTER_ADDRESSES_LH| a.IfType == IF_TYPE_PPP;
        adapters(&selector).ok()?.into_iter().find_map(|a| {
            a.ipv4.map(|ipv4| AdapterInfo {
                name: a.name,
                ipv4,
                gateway: a.gateway,
            })
        })
    }
}

/// 找拨号应绑定的物理以太网适配器。
#[cfg(windows)]
pub fn physical_adapter() -> Result<AdapterInfo> {
    win::physical_adapter()
}

/// 取 PPPoE 会话适配器的 IPv4（拨号成功后）。
#[cfg(windows)]
pub fn ppp_adapter_ip() -> Option<Ipv4Addr> {
    win::ppp_adapter_ip()
}

/// 取 PPPoE 会话适配器完整信息（探测绑定用）。
#[cfg(windows)]
pub fn ppp_adapter() -> Option<AdapterInfo> {
    win::ppp_adapter()
}

#[cfg(test)]
mod tests {
    use super::is_virtual;

    #[test]
    fn flags_known_virtual_adapters() {
        for name in [
            "wintun",
            "Tailscale",
            "Clash TUN",
            "TAP-Windows Adapter",
            "Hyper-V Virtual Ethernet",
            "VMware Virtual Ethernet",
            "VirtualBox Host-Only",
        ] {
            assert!(is_virtual(name), "{name} 应判虚拟");
        }
    }

    #[test]
    fn physical_names_pass() {
        for name in [
            "Realtek Gaming GbE",
            "Intel(R) Ethernet Connection",
            "以太网",
        ] {
            assert!(!is_virtual(name), "{name} 不应误判");
        }
    }
}
