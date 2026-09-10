//! 安装器入口（Windows）：参数解析 → 自提权 → silent 或 GUI。
//! 常量与安装布局见 spec §4；工作流在 work.rs，页面在 ui.rs。

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_ICONWARNING, MESSAGEBOX_STYLE};

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
    let gui_mode = matches!(
        mode,
        Mode::Gui | Mode::Repair | Mode::Uninstall | Mode::StartService
    );
    if !is_admin() {
        // silent 模式：错误留给脚本（stderr + 非零退出码），绝不弹框。
        let wait = matches!(mode, Mode::SilentInstall | Mode::SilentUninstall);
        return match elevate_self_and_maybe_wait(&raw, wait) {
            Ok(()) => Ok(()),
            Err(e) => {
                if gui_mode {
                    show_elevation_failure(&e);
                }
                Err(e)
            }
        };
    }
    match mode {
        Mode::Gui | Mode::Repair | Mode::Uninstall | Mode::StartService => match ui::run(args) {
            Ok(()) => Ok(()),
            Err(e) => {
                // 发布版是 windows 子系统，从资源管理器启动没有 stderr；
                // GUI 启动失败必须落一个原生弹窗，否则窗口直接消失。
                message_box(
                    &format!("安装程序运行失败，已退出。\n\n{e:#}"),
                    MB_ICONERROR,
                );
                Err(e)
            }
        },
        Mode::SilentInstall | Mode::SilentUninstall => {
            crate::logging::init_cli_logging();
            crate::setup::silent::run(&args, mode)
        }
    }
}

/// UAC 被用户拒绝的标记错误：GUI 弹窗据此改用警告文案/图标。
#[derive(Debug)]
struct UacDeclined;

impl std::fmt::Display for UacDeclined {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Elevation was cancelled (UAC declined)")
    }
}

impl std::error::Error for UacDeclined {}

/// 提权失败的 GUI 弹窗：取消 UAC 单独文案 + 警告图标；其他失败附 anyhow 错误文本。
fn show_elevation_failure(e: &anyhow::Error) {
    if e.downcast_ref::<UacDeclined>().is_some() {
        message_box(
            "已取消管理员授权（UAC），安装程序没有启动。",
            MB_ICONWARNING,
        );
    } else {
        message_box(
            &format!("安装程序无法请求管理员权限。\n\n{e:#}"),
            MB_ICONERROR,
        );
    }
}

/// 原生弹窗：egui 还没加载时用（原生控件用系统字体渲染中文，不会豆腐）。
fn message_box(text: &str, icon: MESSAGEBOX_STYLE) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK};
    let text_w = wide(text);
    let title_w = wide("GDUT Net 安装程序");
    let _ = unsafe {
        MessageBoxW(
            None,
            PCWSTR(text_w.as_ptr()),
            PCWSTR(title_w.as_ptr()),
            MB_OK | icon,
        )
    };
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
    // windows 0.62：ShellExecuteExW 返回 Result<()>（内部包 BOOL），用户拒绝 UAC 时为 ERROR_CANCELLED；
    // 用标记错误让 GUI 弹窗区分"取消"与其他失败。
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        if e.code() == windows::Win32::Foundation::ERROR_CANCELLED.to_hresult() {
            return Err(anyhow::Error::new(UacDeclined));
        }
        return Err(anyhow::Error::new(e).context("ShellExecuteExW (runas) failed"));
    }
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
