//! 详情面板：优先 Win32 MessageBox（零 GL 依赖、主线程兼容），
//! eframe 仅作备用（已验证 wgpu 在部分核显上仍黑屏）。

use std::sync::mpsc::Sender;

use super::SharedSnapshot;

#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_YESNO};

/// 打开状态面板：MessageBox 展示快照，Yes → 重拨。
pub fn show(snapshot: SharedSnapshot, redial_tx: Sender<()>) {
    let text = {
        let guard = snapshot.lock().expect("snapshot poisoned");
        match guard.as_ref() {
            None => "Not connected to gdut-net service (not running?)".to_string(),
            Some(s) => format!(
                "Status: {}\nUptime: {}\nIP: {}\nDrop reason: {}\nRedial attempts: {}\nHeartbeat: {}",
                s.status_text(),
                s.uptime_text(),
                s.ip.as_deref().unwrap_or("—"),
                s.last_drop_reason.as_deref().unwrap_or("—"),
                s.redial_attempts,
                s.heartbeat_text()
            ),
        }
    };
    let body = format!("{text}\n\nRedial now?");
    #[cfg(windows)]
    {
        let wide = |s: &str| {
            s.encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<u16>>()
        };
        let title = wide("gdut-net Status");
        let msg = wide(&body);
        let ret = unsafe {
            MessageBoxW(
                None,
                PCWSTR(msg.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_YESNO | MB_ICONINFORMATION,
            )
        };
        // IDYES = 6
        if ret.0 == 6 {
            let _ = redial_tx.send(());
        }
    }
    #[cfg(not(windows))]
    {
        let _ = body;
        let _ = redial_tx;
    }
}
