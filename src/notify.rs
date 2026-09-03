//! 系统Toast 通知（tauri-winrt-notification）。
//!
//! 服务侧钩子调用 [`toast`]；节流（同一原因 30 分钟内不重复）由调用方
//! （runtime 的 Notifier）负责，本模块保持无状态。
//!
//! 仅 Windows 编译，以 `cargo check --target x86_64-pc-windows-msvc` 验证。

#[cfg(windows)]
mod win {
    use anyhow::Result;

    /// 弹一条系统 Toast；app_id 用 "gdut-net"（无需注册 AUMID 即可尝试显示，
    /// 失败上抛由调用方决定是否告警，不中断服务）。
    pub fn toast(title: &str, body: &str) -> Result<()> {
        tauri_winrt_notification::Toast::new("gdut-net")
            .title(title)
            .text1(body)
            .show()?;
        Ok(())
    }
}

#[cfg(windows)]
pub use win::toast;
