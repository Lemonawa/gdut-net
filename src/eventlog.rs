//! Windows 事件日志：source 注册表 + RegisterEventSourceW/ReportEventW 胶水。
//!
//! 仅 Windows 编译，以 `cargo check --target x86_64-pc-windows-msvc` 验证。

#[cfg(windows)]
mod win {
    use std::path::Path;

    use anyhow::{Context, Result};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::EventLog::{
        DeregisterEventSource, RegisterEventSourceW, ReportEventW, EVENTLOG_ERROR_TYPE,
        EVENTLOG_INFORMATION_TYPE, EVENTLOG_WARNING_TYPE, REPORT_EVENT_TYPE,
    };
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE,
        KEY_WRITE, REG_DWORD, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE,
    };

    use super::EVENT_SOURCE_NAME;

    pub const SOURCE_SUBKEY: &str =
        r"SYSTEM\CurrentControlSet\Services\EventLog\Application\gdut-net";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn pw(buf: &[u16]) -> PCWSTR {
        PCWSTR(buf.as_ptr())
    }

    fn win32_err(step: &str, code: windows::Win32::Foundation::WIN32_ERROR) -> anyhow::Error {
        anyhow::anyhow!("{step} failed: error {}", code.0)
    }

    /// 打开已存在的源键（KEY_SET_VALUE）；不存在报错由调用方处理。
    fn open_source_key() -> Result<HKEY> {
        let subkey = wide(SOURCE_SUBKEY);
        let mut hkey = HKEY::default();
        let ret = unsafe {
            RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                pw(&subkey),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut hkey,
                None,
            )
        };
        if ret != ERROR_SUCCESS {
            return Err(win32_err("RegCreateKeyExW", ret));
        }
        Ok(hkey)
    }

    fn close_key(hkey: HKEY) -> Result<()> {
        let ret = unsafe { RegCloseKey(hkey) };
        if ret != ERROR_SUCCESS {
            return Err(win32_err("RegCloseKey", ret));
        }
        Ok(())
    }

    /// 注册事件源（install 时调用一次）：EventMessageFile 指向 netmsg.dll
    /// （其消息 3299 按 "%1" 原样输出），TypesSupported = 7（Error|Warning|Info）。
    pub fn register_source() -> Result<()> {
        let hkey = open_source_key()?;

        let value_name = wide("EventMessageFile");
        let msg_file = wide(r"%SystemRoot%\System32\netmsg.dll");
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(msg_file.as_ptr().cast::<u8>(), msg_file.len() * 2)
        };
        let ret_msg =
            unsafe { RegSetValueExW(hkey, pw(&value_name), None, REG_EXPAND_SZ, Some(bytes)) };

        let value_name = wide("TypesSupported");
        // REG_DWORD 按小端存储（Windows 注册表惯例）。
        let ret_types = unsafe {
            RegSetValueExW(
                hkey,
                pw(&value_name),
                None,
                REG_DWORD,
                Some(&7u32.to_le_bytes()),
            )
        };
        close_key(hkey).ok();
        if ret_msg != ERROR_SUCCESS {
            return Err(win32_err("Write EventMessageFile", ret_msg));
        }
        if ret_types != ERROR_SUCCESS {
            return Err(win32_err("Write TypesSupported", ret_types));
        }
        Ok(())
    }

    /// 删除事件源注册表键（uninstall 时调用）；键不存在视为已删除。
    pub fn unregister_source() -> Result<()> {
        let ret = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, pw(&wide(SOURCE_SUBKEY))) };
        if ret != ERROR_SUCCESS && ret != ERROR_FILE_NOT_FOUND {
            return Err(win32_err("RegDeleteTreeW", ret));
        }
        Ok(())
    }

    /// 事件级别：与 log crate 级别对应的三类经典事件。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum EventLevel {
        Info,
        Warning,
        Error,
    }

    impl EventLevel {
        fn raw(self) -> REPORT_EVENT_TYPE {
            match self {
                EventLevel::Info => EVENTLOG_INFORMATION_TYPE,
                EventLevel::Warning => EVENTLOG_WARNING_TYPE,
                EventLevel::Error => EVENTLOG_ERROR_TYPE,
            }
        }
    }

    /// 已注册的事件源句柄；Drop 时 DeregisterEventSource。
    pub struct EventLog {
        handle: windows::Win32::Foundation::HANDLE,
    }

    impl EventLog {
        pub fn open() -> Result<Self> {
            let handle =
                unsafe { RegisterEventSourceW(PCWSTR::null(), pw(&wide(EVENT_SOURCE_NAME))) }
                    .context("RegisterEventSourceW failed")?;
            Ok(EventLog { handle })
        }

        /// 报告一条事件：消息作为单个替换串走 netmsg.dll 3299 模板。
        pub fn report(&self, level: EventLevel, msg: &str) -> Result<()> {
            let wide_msg = wide(msg);
            let strings = [PCWSTR::from_raw(wide_msg.as_ptr())];
            unsafe {
                ReportEventW(
                    self.handle,
                    level.raw(),
                    0,
                    3299,
                    None,
                    0,
                    Some(&strings),
                    None,
                )
            }
            .context("ReportEventW failed")
        }
    }

    impl Drop for EventLog {
        fn drop(&mut self) {
            let _ = unsafe { DeregisterEventSource(self.handle) };
        }
    }

    /// 确保目录存在（service_main 初始化日志目录用）。
    pub fn ensure_dir(path: &Path) -> Result<()> {
        std::fs::create_dir_all(path)
            .with_context(|| format!("Failed to create directory: {}", path.display()))
    }
}

#[cfg(windows)]
pub use win::{
    ensure_dir, register_source, unregister_source, EventLevel, EventLog, SOURCE_SUBKEY,
};

pub const EVENT_SOURCE_NAME: &str = "gdut-net";
