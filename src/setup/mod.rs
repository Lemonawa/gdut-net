//! 安装器入口（Windows）：参数解析 → 自提权 → silent 或 GUI。
//! 常量与安装布局见 spec §4；工作流在 work.rs，页面在 ui.rs。

use std::path::PathBuf;

use anyhow::{bail, Context, Result};

pub mod silent;
pub mod ui;
// `pub mod work;` lands here in Task 8.

pub use crate::setup_args::{Mode, SetupArgs};

pub const START_MENU_FOLDER: &str = "GDUT Net";
pub const DATA_DIR: &str = r"C:\ProgramData\gdut-net";

/// 安装目录：%ProgramFiles%\gdut-net。
pub fn install_dir() -> PathBuf {
    let base = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    base.join("gdut-net")
}

pub fn is_admin() -> bool {
    unsafe { windows::Win32::UI::Shell::IsUserAnAdmin() }.as_bool()
}

pub fn entry() -> Result<()> {
    let raw: Vec<String> = std::env::args().collect();
    let args = SetupArgs::parse(raw.iter().skip(1).cloned())?;
    let mode = args.mode()?;
    if !is_admin() {
        let wait = matches!(mode, Mode::SilentInstall | Mode::SilentUninstall);
        return elevate_self_and_maybe_wait(&raw, wait);
    }
    match mode {
        Mode::Gui | Mode::Repair | Mode::Uninstall | Mode::StartService => ui::run(args),
        Mode::SilentInstall | Mode::SilentUninstall => {
            crate::logging::init_cli_logging();
            crate::setup::silent::run(&args, mode)
        }
    }
}

/// ShellExecuteW "runas" 重启自身；silent 等非 GUI 模式等待并透传退出码。
fn elevate_self_and_maybe_wait(raw: &[String], wait: bool) -> Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let exe = std::env::current_exe()?;
    let params = quote_args(&raw[1..]);
    let exe_w: Vec<u16> = exe
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let verb = wide("runas");
    let params_w = wide(&params);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(exe_w.as_ptr()),
        lpParameters: PCWSTR(params_w.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // windows 0.62：ShellExecuteExW 返回 Result<()>（内部包 BOOL），用户拒绝 UAC 时为 ERROR_CANCELLED。
    unsafe { ShellExecuteExW(&mut info) }.context("Elevation was cancelled (UAC declined?)")?;
    if wait {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{
            GetExitCodeProcess, WaitForSingleObject, INFINITE,
        };
        unsafe { WaitForSingleObject(info.hProcess, INFINITE) };
        let mut code = 1u32;
        unsafe { GetExitCodeProcess(info.hProcess, &mut code) }
            .context("GetExitCodeProcess failed")?;
        unsafe {
            let _ = CloseHandle(info.hProcess);
        }
        if code != 0 {
            bail!("Elevated setup failed with exit code {code}");
        }
    }
    Ok(())
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 参数带空格时加引号（安装路径可能含空格）。
fn quote_args(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.contains(' ') {
                format!("\"{a}\"")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
