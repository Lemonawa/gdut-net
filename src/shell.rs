//! 安装态 shell 集成（Windows）：开始菜单快捷方式（IShellLink COM）、
//! "应用和功能"卸载项（HKLM）、卸载后延迟删除安装目录（cmd 助手）。
//!
//! 不依赖 `crate::setup`（避免模块环）：`START_MENU_FOLDER` 在此定义，
//! `setup/mod.rs` 反向 re-export。所有移除函数幂等：键/目录缺失不算错误。

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::shell_shortcuts::SHORTCUTS;

pub const START_MENU_FOLDER: &str = "GDUT Net";
const UNINSTALL_SUBKEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\gdut-net";

/// 开始菜单公共程序目录（ProgramData 全局，非用户级）。
fn start_menu_dir() -> Result<PathBuf> {
    let base = std::env::var_os("ProgramData").context("ProgramData is not set")?;
    Ok(PathBuf::from(base)
        .join(r"Microsoft\Windows\Start Menu\Programs")
        .join(START_MENU_FOLDER))
}

/// 安装 shell 集成：逐个建快捷方式（目标缺失即失败）后写卸载键。
pub fn install_shell_integration(install_dir: &Path, version: &str) -> Result<()> {
    let dir = start_menu_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    for s in SHORTCUTS {
        let target = install_dir.join(s.target);
        if !target.exists() {
            bail!("Shortcut target missing: {}", target.display());
        }
        create_shortcut(
            &dir.join(format!("{}.lnk", s.name)),
            &target,
            s.args,
            s.run_as_admin,
            s.name,
        )?;
    }
    write_uninstall_key(install_dir, version)
}

/// 移除开始菜单目录与卸载键；失败仅告警（卸载链不因 shell 集成卡住）。
pub fn remove_shell_integration() -> Result<()> {
    if let Ok(dir) = start_menu_dir() {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => log::info!("Removed Start Menu folder {}", dir.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("Failed to remove {}: {e}", dir.display()),
        }
    }
    if let Err(e) = delete_uninstall_key() {
        log::warn!("Failed to remove uninstall registry key: {e:#}");
    }
    Ok(())
}

/// 延迟删除安装目录：启动后直接退出的 setup 进程无法删掉自己所在的目录，
/// 交给 cmd 后台重试（CREATE_NO_WINDOW | DETACHED_PROCESS）：单条 cmd 最多
/// 90 次，每次 `rmdir` 后若目录已消失即退出，否则约 1s 后再试。
/// GUI 路径的 setup 窗口会长时间占用安装目录里的 exe，重试窗口覆盖它；
/// silent 路径进程立刻退出，最初几次尝试即可删掉。
///
/// `current_dir(temp)`：开始菜单快捷方式把工作目录设为安装目录，助手若带
/// 这个 CWD 就无法删除自己所在的目录（Windows 目录占用），先挪到 temp。
/// 用 `raw_arg` 原样把脚本交给 cmd：`.arg()` 会按 CRT 规则把内嵌引号转义成
/// `\"`，而 cmd.exe 不认这种转义——安装路径含空格，引号必须原样到达。
pub fn schedule_install_dir_removal(dir: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt as _;
    let dir = dir.to_path_buf();
    let script = format!(
        "@echo off & for /l %i in (1,1,90) do (rmdir /s /q \"{0}\" 2>nul & if not exist \"{0}\" exit /b & ping -n 2 127.0.0.1 >nul)",
        dir.display()
    );
    std::process::Command::new("cmd")
        .arg("/c")
        .raw_arg(&script)
        .current_dir(std::env::temp_dir())
        .creation_flags(0x0800_0000 | 0x0000_0008) // CREATE_NO_WINDOW | DETACHED_PROCESS
        .spawn()
        .context("Failed to spawn delayed directory removal")?;
    Ok(())
}

/// 已安装版本（HKLM 卸载键 DisplayVersion）；未安装或读取失败为 None。
pub fn installed_version() -> Option<String> {
    read_uninstall_string("DisplayVersion")
}

// ---- COM / registry 胶水 ----

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 建单个 .lnk：CoInitializeEx/CoUninitialize 严格配对，body 失败也先反初始化再上抛。
fn create_shortcut(
    lnk: &Path,
    target: &Path,
    args: &str,
    run_as_admin: bool,
    description: &str,
) -> Result<()> {
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

    // S_FALSE（已初始化）与 S_OK 均可；RPC_E_CHANGED_MODE 等其他失败退出。
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if hr.is_err() {
        bail!("CoInitializeEx failed: {hr:?}");
    }
    let result = create_shortcut_body(lnk, target, args, run_as_admin, description);
    unsafe { CoUninitialize() };
    result.with_context(|| format!("Failed to create shortcut {}", lnk.display()))
}

fn create_shortcut_body(
    lnk: &Path,
    target: &Path,
    args: &str,
    run_as_admin: bool,
    description: &str,
) -> Result<()> {
    use windows::core::{Interface as _, PCWSTR};
    use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::{IShellLinkDataList, IShellLinkW, ShellLink, SLDF_RUNAS_USER};

    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let target_w = wide(&target.to_string_lossy());
        let args_w = wide(args);
        let desc_w = wide(description);
        link.SetPath(PCWSTR(target_w.as_ptr()))?;
        link.SetArguments(PCWSTR(args_w.as_ptr()))?;
        link.SetDescription(PCWSTR(desc_w.as_ptr()))?;
        if let Some(parent) = target.parent() {
            let dir_w = wide(&parent.to_string_lossy());
            link.SetWorkingDirectory(PCWSTR(dir_w.as_ptr()))?;
        }
        // 管理员快捷方式：SLDF_RUNAS_USER 让 Shell 执行时弹 UAC（campus/home/无线体检/卸载）。
        if run_as_admin {
            let dl: IShellLinkDataList = link.cast()?;
            let flags = dl.GetFlags()?;
            dl.SetFlags(flags | SLDF_RUNAS_USER.0 as u32)?;
        }
        let pf: IPersistFile = link.cast()?;
        let lnk_w = wide(&lnk.to_string_lossy());
        pf.Save(PCWSTR(lnk_w.as_ptr()), true)?;
    }
    Ok(())
}

/// 写 HKLM "应用和功能"卸载项（DisplayName/Version/Icon/UninstallString 等）。
fn write_uninstall_key(install_dir: &Path, version: &str) -> Result<()> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_WRITE,
        REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    let mut hkey = HKEY::default();
    let subkey = wide(UNINSTALL_SUBKEY);
    let ret = unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            windows::core::PCWSTR(subkey.as_ptr()),
            None,
            windows::core::PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
    };
    if ret != ERROR_SUCCESS {
        bail!("RegCreateKeyExW failed: {}", ret.0);
    }
    let set_sz = |name: &str, value: &str| -> Result<()> {
        let n = wide(name);
        let v = wide(value);
        let bytes: Vec<u8> = v.iter().flat_map(|c| c.to_le_bytes()).collect();
        let ret = unsafe {
            RegSetValueExW(
                hkey,
                windows::core::PCWSTR(n.as_ptr()),
                None,
                REG_SZ,
                Some(bytes.as_slice()),
            )
        };
        if ret != ERROR_SUCCESS {
            bail!("RegSetValueExW({name}) failed: {}", ret.0);
        }
        Ok(())
    };
    set_sz("DisplayName", "GDUT Net")?;
    set_sz("DisplayVersion", version)?;
    set_sz("InstallLocation", &install_dir.to_string_lossy())?;
    set_sz(
        "DisplayIcon",
        &install_dir.join("gdut-net.exe").to_string_lossy(),
    )?;
    set_sz(
        "UninstallString",
        &format!(
            "\"{}\" --uninstall",
            install_dir.join("gdut-net-setup.exe").display()
        ),
    )?;
    set_sz(
        "QuietUninstallString",
        &format!(
            "\"{}\" --silent --uninstall",
            install_dir.join("gdut-net-setup.exe").display()
        ),
    )?;
    let one: u32 = 1;
    for name in ["NoModify", "NoRepair"] {
        let n = wide(name);
        let bytes = one.to_le_bytes();
        let ret = unsafe {
            RegSetValueExW(
                hkey,
                windows::core::PCWSTR(n.as_ptr()),
                None,
                REG_DWORD,
                Some(bytes.as_slice()),
            )
        };
        if ret != ERROR_SUCCESS {
            bail!("RegSetValueExW({name}) failed: {}", ret.0);
        }
    }
    let closed = unsafe { RegCloseKey(hkey) };
    if closed != ERROR_SUCCESS {
        bail!("RegCloseKey failed: {}", closed.0);
    }
    Ok(())
}

/// 删除卸载键；键不存在（或父路径不存在）视为幂等成功。
fn delete_uninstall_key() -> Result<()> {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{RegDeleteTreeW, HKEY_LOCAL_MACHINE};

    let subkey = wide(UNINSTALL_SUBKEY);
    let ret = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, windows::core::PCWSTR(subkey.as_ptr())) };
    if ret != ERROR_SUCCESS && ret != ERROR_FILE_NOT_FOUND && ret != ERROR_PATH_NOT_FOUND {
        bail!("RegDeleteTreeW failed: {}", ret.0);
    }
    Ok(())
}

/// 读卸载键的 REG_SZ 值；缺失/类型不符/超长均返回 None（展示用，绝不致命）。
fn read_uninstall_string(name: &str) -> Option<String> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

    let subkey = wide(UNINSTALL_SUBKEY);
    let value = wide(name);
    let mut buf = [0u16; 256];
    let mut size = (buf.len() * 2) as u32;
    let ret = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            windows::core::PCWSTR(subkey.as_ptr()),
            windows::core::PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if ret != ERROR_SUCCESS {
        return None;
    }
    // size 含结尾 NUL 的字节数；截掉 NUL 后转字符串。
    let len = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..len.min(buf.len())]))
}
