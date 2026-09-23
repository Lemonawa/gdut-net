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

/// 一个可用网络出口：适配器名 + IPv4 + 可选网关 + 接口索引。
#[derive(Debug, Clone)]
pub struct AdapterInfo {
    pub name: String,
    pub ipv4: Ipv4Addr,
    pub gateway: Option<Ipv4Addr>,
    pub ifindex: u32,
}

#[cfg(windows)]
mod win {
    use std::mem::size_of;
    use std::net::Ipv4Addr;

    use anyhow::{anyhow, Result};
    use windows::core::PWSTR;
    use windows::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_INCLUDE_GATEWAYS, IF_TYPE_ETHERNET_CSMACD,
        IF_TYPE_IEEE80211, IF_TYPE_PPP, IP_ADAPTER_ADDRESSES_LH, IP_ADAPTER_GATEWAY_ADDRESS_LH,
        IP_ADAPTER_UNICAST_ADDRESS_LH,
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
        pub ifindex: u32,
        pub oper_up: bool,
        pub has_dns: bool,
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

    /// 把 PPPoE 接口的 IPv4 DNS 置空（netsh 侧）。只做这一件事：
    /// 2026-09-23 实测二次写回（source=dhcp）会触发 PPP 会话重协商，约 30s 后会话被判 dropped。
    fn set_ppp_dns_none(name: &str) -> Result<()> {
        let name_arg = format!("name={name}");
        let args = [
            "interface",
            "ipv4",
            "set",
            "dnsservers",
            name_arg.as_str(),
            "source=static",
            "address=none",
        ];
        let out = std::process::Command::new("netsh").args(args).output()?;
        if !out.status.success() {
            return Err(anyhow!(
                "netsh {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stdout).trim()
            ));
        }
        Ok(())
    }

    /// 除 PPP 外是否还有 up 状态的接口带着 DNS（清 PPP DNS 的前置条件）。
    pub(super) fn other_adapter_has_dns() -> bool {
        let selector: Box<Selector> = Box::new(|a: &IP_ADAPTER_ADDRESSES_LH| {
            a.IfType != IF_TYPE_PPP && a.OperStatus == IfOperStatusUp
        });
        adapters(&selector)
            .map(|v| v.iter().any(|a| a.has_dns))
            .unwrap_or(false)
    }

    fn resolves(host: &str) -> bool {
        use std::net::ToSocketAddrs;
        (host, 443)
            .to_socket_addrs()
            .map(|mut it| it.next().is_some())
            .unwrap_or(false)
    }

    fn has_aaaa(host: &str) -> bool {
        use std::net::{SocketAddr, ToSocketAddrs};
        (host, 443)
            .to_socket_addrs()
            .map(|mut it| it.any(|a| matches!(a, SocketAddr::V6(_))))
            .unwrap_or(false)
    }

    /// 见 `super::detach_ppp_dns`。
    pub(super) fn detach_ppp_dns(entry_name: &str) {
        const PROBE: &str = "www.baidu.com";
        if !other_adapter_has_dns() {
            // 物理口没 DNS 时不动 PPP 的，避免把解析清没。
            log::info!("Skip detaching PPP DNS: no other up adapter carries DNS");
            return;
        }
        if let Err(e) = set_ppp_dns_none(entry_name) {
            log::warn!("Clear PPP DNS failed ({e:#}); keeping RAS-provided DNS");
            return;
        }
        // 只观测、不回写：写回会重协商 PPP 会话（见 set_ppp_dns_none 注释）。
        // 真出问题也只是当前会话，下次拨号 RAS 重新下发即恢复。
        for attempt in 0..4u32 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(1200));
            }
            if resolves(PROBE) {
                let aaaa = if has_aaaa(PROBE) {
                    "AAAA ok"
                } else {
                    "AAAA missing"
                };
                log::info!(
                    "PPP interface '{entry_name}' DNS detached (physical NIC serves DNS; {aaaa})"
                );
                return;
            }
        }
        log::warn!(
            "DNS still failing after detaching PPP DNS; leaving as-is (next dial restores RAS-provided DNS)"
        );
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
            return Err(anyhow!("GetAdaptersAddresses failed: error {ret}"));
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
                    ifindex: unsafe { a.Anonymous1.Anonymous.IfIndex },
                    oper_up: a.OperStatus == IfOperStatusUp,
                    has_dns: !a.FirstDnsServerAddress.is_null(),
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
                    ifindex: a.ifindex,
                })
            })
            .collect();
        // 稳定排序：有网关者排前，同序保持原链表顺序
        candidates.sort_by_key(|a| a.gateway.is_none());
        candidates
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("No usable physical Ethernet adapter found"))
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
                ifindex: a.ifindex,
            })
        })
    }

    /// WLAN 适配器（IF_TYPE_IEEE80211 + OperStatus Up + 有 IPv4，非虚拟），有网关者优先。
    pub(super) fn wlan_adapter() -> Option<AdapterInfo> {
        let selector = |a: &IP_ADAPTER_ADDRESSES_LH| {
            a.IfType == IF_TYPE_IEEE80211 && a.OperStatus == IfOperStatusUp
        };
        let mut candidates: Vec<AdapterInfo> = adapters(&selector)
            .ok()?
            .into_iter()
            .filter(|a| !super::is_virtual(&a.name) && !super::is_virtual(&a.desc))
            .filter_map(|a| {
                a.ipv4.map(|ipv4| AdapterInfo {
                    name: a.name,
                    ipv4,
                    gateway: a.gateway,
                    ifindex: a.ifindex,
                })
            })
            .collect();
        candidates.sort_by_key(|a| a.gateway.is_none());
        candidates.into_iter().next()
    }

    /// 以太网链路态：`None` = 没有任何非虚拟以太网卡（无法判断，调用方不应据此禁拨）；
    /// `Some(false)` = 有网卡但全部 link down（拔线）；`Some(true)` = 至少一张 Up。
    /// 与 physical_adapter() 区别：不要求已有 IPv4（DHCP 前的 link up 也算）。
    pub(super) fn ethernet_link_up() -> Option<bool> {
        let selector = |a: &IP_ADAPTER_ADDRESSES_LH| a.IfType == IF_TYPE_ETHERNET_CSMACD;
        let list = adapters(&selector).ok()?;
        let mut found = false;
        let mut up = false;
        for a in list {
            if super::is_virtual(&a.name) || super::is_virtual(&a.desc) {
                continue;
            }
            found = true;
            if a.oper_up {
                up = true;
            }
        }
        if found {
            Some(up)
        } else {
            None
        }
    }
}

/// 找拨号应绑定的物理以太网适配器。
#[cfg(windows)]
pub fn physical_adapter() -> Result<AdapterInfo> {
    win::physical_adapter()
}

/// 拨号成功后卸掉 PPP 接口的 IPv4 DNS（DNS 交给物理口）。
///
/// 2026-09-23 真机实测：PPP 接口只要带着 RAS 下发的 DNS，Windows DNS 客户端就不向应用层
/// 交付 AAAA（`ping -6`/`curl -6`/`getaddrinfo` 全空，而 `nslookup` 正常）；清空后立刻恢复，
/// 重拨后 RAS 重新下发即再次失效——所以每次拨号成功后都要卸一次。
/// 只在**别的 up 接口确实带 DNS** 时才清（否则不动）；清完用 getaddrinfo 复核，只记录结论、
/// **不主动写回**——二次 netsh 写会触发 PPP 会话重协商（2026-09-23 实测 30s 后 dropped 触发回滚），
/// 真出问题也只是当前会话，下次拨号 RAS 会重新下发 DNS 自愈。
#[cfg(windows)]
pub fn detach_ppp_dns(entry_name: &str) {
    win::detach_ppp_dns(entry_name)
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

/// 取 WLAN 适配器（无线接管的探测/心跳绑定目标）。
#[cfg(windows)]
pub fn wlan_adapter() -> Option<AdapterInfo> {
    win::wlan_adapter()
}

/// 以太网链路态：`None` = 无非虚拟以太网卡；`Some(bool)` = 存在且是否全部 link up。
#[cfg(windows)]
pub fn ethernet_link_up() -> Option<bool> {
    win::ethernet_link_up()
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
            assert!(is_virtual(name), "{name} should be flagged as virtual");
        }
    }

    #[test]
    fn physical_names_pass() {
        for name in [
            "Realtek Gaming GbE",
            "Intel(R) Ethernet Connection",
            "Ethernet",
        ] {
            assert!(!is_virtual(name), "{name} should not be flagged as virtual");
        }
    }
}
