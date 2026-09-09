//! WLAN 关联控制：WlanAPI 直调（Session 0 可用、全用户 profile），netsh 兜底
//! （仅动作命令，不解析输出文本——中文系统 netsh 文本 GBK 且本地化，不可解析）。

use anyhow::{anyhow, bail, Result};
use windows::core::PCWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::NetworkManagement::WiFi::{
    dot11_BSS_type_infrastructure, wlan_connection_mode_profile, wlan_interface_state_connected,
    WlanCloseHandle, WlanConnect, WlanDisconnect, WlanEnumInterfaces, WlanFreeMemory,
    WlanOpenHandle, WLAN_CONNECTION_PARAMETERS, WLAN_INTERFACE_INFO_LIST,
};

const CLIENT_VERSION: u32 = 2; // WLAN_CLIENT_VERSION_LONGHORN（Vista+）

fn with_handle<T>(f: impl FnOnce(HANDLE) -> Result<T>) -> Result<T> {
    let mut negotiated: u32 = 0;
    let mut handle = HANDLE::default();
    let err = unsafe { WlanOpenHandle(CLIENT_VERSION, None, &mut negotiated, &mut handle) };
    if err != 0 {
        bail!("WlanOpenHandle failed: {err}");
    }
    let out = f(handle);
    unsafe {
        let _ = WlanCloseHandle(handle, None);
    }
    out
}

/// 首个 WLAN 接口 GUID（单无线网卡机型，够用）。
fn first_interface(handle: HANDLE) -> Result<windows::core::GUID> {
    let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
    let err = unsafe { WlanEnumInterfaces(handle, None, &mut list) };
    if err != 0 {
        bail!("WlanEnumInterfaces failed: {err}");
    }
    let n = unsafe { (*list).dwNumberOfItems };
    let guid = if n == 0 {
        None
    } else {
        Some(unsafe { (*list).InterfaceInfo[0].InterfaceGuid })
    };
    unsafe {
        WlanFreeMemory(list.cast());
    }
    guid.ok_or_else(|| anyhow!("no WLAN interface present"))
}

fn wlanapi_connect(profile: &str) -> Result<()> {
    with_handle(|h| {
        let guid = first_interface(h)?;
        let mut profile_w: Vec<u16> = profile.encode_utf16().chain(std::iter::once(0)).collect();
        let params = WLAN_CONNECTION_PARAMETERS {
            wlanConnectionMode: wlan_connection_mode_profile,
            strProfile: PCWSTR(profile_w.as_mut_ptr()),
            pDot11Ssid: std::ptr::null_mut(),
            pDesiredBssidList: std::ptr::null_mut(),
            dot11BssType: dot11_BSS_type_infrastructure,
            dwFlags: 0,
        };
        let err = unsafe { WlanConnect(h, &guid, &params, None) };
        if err != 0 {
            bail!("WlanConnect({profile}) failed: {err}");
        }
        Ok(())
    })
}

fn wlanapi_disconnect() -> Result<()> {
    with_handle(|h| {
        let guid = first_interface(h)?;
        let err = unsafe { WlanDisconnect(h, &guid, None) };
        if err != 0 {
            bail!("WlanDisconnect failed: {err}");
        }
        Ok(())
    })
}

fn netsh(args: &[&str]) -> Result<()> {
    let out = std::process::Command::new("netsh").args(args).output()?;
    if !out.status.success() {
        bail!(
            "netsh {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// 关联 profile（WlanAPI 失败回退 netsh；两种后端可切换，ADR-0005）。
pub fn associate(profile: &str) -> Result<()> {
    match wlanapi_connect(profile) {
        Ok(()) => Ok(()),
        Err(e) => {
            log::warn!("WlanApi connect failed, falling back to netsh: {e:#}");
            netsh(&["wlan", "connect", &format!("name={profile}")])
        }
    }
}

pub fn disassociate() -> Result<()> {
    match wlanapi_disconnect() {
        Ok(()) => Ok(()),
        Err(e) => {
            log::warn!("WlanApi disconnect failed, falling back to netsh: {e:#}");
            netsh(&["wlan", "disconnect"])
        }
    }
}

/// 是否已关联（state==connected）。不解析 netsh 文本（locale）。
pub fn associated() -> bool {
    with_handle(|h| {
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        let err = unsafe { WlanEnumInterfaces(h, None, &mut list) };
        if err != 0 {
            return Ok(false);
        }
        let n = unsafe { (*list).dwNumberOfItems } as usize;
        // InterfaceInfo 在结构体里声明为 [..; 1]，实际是变长尾数组：
        // 直接下标访问在 dwNumberOfItems >= 2 时会被 Rust 边界检查 panic，
        // 用切片视图安全覆盖全部条目。
        let items = unsafe { std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), n) };
        let hit = items
            .iter()
            .any(|info| info.isState == wlan_interface_state_connected);
        unsafe {
            WlanFreeMemory(list.cast());
        }
        Ok(hit)
    })
    .unwrap_or(false)
}
