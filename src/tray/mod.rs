//! 托盘常驻图标（tray-icon）+ 按需 egui 状态面板。
//!
//! 事件循环模型：**主线程裸 win32 消息泵**（GetMessageW /
//! MsgWaitForMultipleObjects），tray-icon 在 Windows 要求创建图标的
//! 线程跑 win32 事件循环，主线程恰好满足且无需引入 winit 依赖。
//!
//! 线程模型：所有 MenuItem 操作（含 set_text）都在主泵线程完成——
//! muda 的 MenuItem 内含 Rc，不可跨线程。后台 IPC 线程只经 std mpsc
//! 发送"status text to display"，泵线程每拍取来应用到菜单项。
//!
//! PipeClient 的 async 方法由每次调用自建的极小 current_thread runtime
//! 驱动——托盘线程没有全局 tokio executor，不能假设 runtime 存在。

mod panel;

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
    REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::System::Threading::INFINITE;

use crate::ipc::client::PipeClient;
use crate::ipc::protocol::{Command, StateSnapshot};

/// 后台线程 connect 失败后的重试间隔。
const CONNECT_RETRY: Duration = Duration::from_secs(3);

/// 共享快照缓存：None = 尚未收到（服务未运行/刚断开）。
pub(crate) type SharedSnapshot = Arc<Mutex<Option<StateSnapshot>>>;

/// 32x32 RGBA 占位图标：主题青色方块（代码生成，无需二进制图片资源）。
fn tray_icon_rgba() -> Vec<u8> {
    const S: usize = 32;
    let mut rgba = Vec::with_capacity(S * S * 4);
    let inner = 2..S - 2;
    for y in 0..S {
        for x in 0..S {
            let (r, g, b, a) = if inner.contains(&x) && inner.contains(&y) {
                (0x30, 0x9c, 0xdc, 0xff) // 主题青
            } else {
                (0x30, 0x9c, 0xdc, 0x00) // 2px 透明留白，托盘里不顶边
            };
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    rgba
}

/// 注册 AUMID（HKCU\Software\Classes\AppUserModelId\gdut-net，默认值
/// DisplayName=GDUT Net）。
///
/// tauri-winrt-notification 以 app_id "gdut-net" 弹 toast（Task 13）；
/// 未注册 AUMID 时部分（域策略受限的）系统会静默丢弃通知。写 HKCU 无需
/// 管理员权限，失败只记日志不阻断托盘启动（最坏情况回到不可见 toast）。
fn register_aumid() {
    const SUBKEY: &str = r"Software\Classes\AppUserModelId\gdut-net";
    let wide = |s: &str| {
        s.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let subkey = wide(SUBKEY);
    let display = wide("GDUT Net");

    let mut hkey = HKEY::default();
    let ret = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
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
        log::warn!(
            "Failed to register AUMID (toast may not show): error {}",
            ret.0
        );
        return;
    }
    // 默认值（值名为 null 的 REG_SZ）即通知中心显示的来源名。
    // u16 按小端摊平成字节；"GDUT Net\0" 为 9 个 u16 = 18 字节，偶数
    // 长度保证不截断码元。
    let bytes: Vec<u8> = display.iter().flat_map(|w| w.to_le_bytes()).collect();
    let ret = unsafe { RegSetValueExW(hkey, PCWSTR::null(), None, REG_SZ, Some(&bytes)) };
    let closed = unsafe { RegCloseKey(hkey) };
    if ret != ERROR_SUCCESS || closed != ERROR_SUCCESS {
        log::warn!("Failed to write AUMID DisplayName: error {}", ret.0);
    }
}

pub fn register_autostart() -> Result<()> {
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let exe = std::env::current_exe().context("Failed to get exe path")?;
    let value = format!("\"{}\" tray", exe.display());
    let subkey_w: Vec<u16> = RUN_KEY.encode_utf16().chain(std::iter::once(0)).collect();
    let name_w: Vec<u16> = "gdut-net-tray"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let value_w: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let value_bytes: Vec<u8> = value_w.iter().flat_map(|c| c.to_le_bytes()).collect();
    let mut hkey = HKEY::default();
    let ret = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
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
        anyhow::bail!("RegCreateKeyExW Run failed: {}", ret.0);
    }
    let ret = unsafe {
        RegSetValueExW(
            hkey,
            PCWSTR(name_w.as_ptr()),
            None,
            REG_SZ,
            Some(&value_bytes),
        )
    };
    let _ = unsafe { RegCloseKey(hkey) };
    if ret != ERROR_SUCCESS {
        anyhow::bail!("RegSetValueExW failed: {}", ret.0);
    }
    Ok(())
}

pub fn unregister_autostart() -> Result<()> {
    use windows::Win32::System::Registry::{RegDeleteValueW, RegOpenKeyExW, KEY_SET_VALUE};
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let subkey_w: Vec<u16> = RUN_KEY.encode_utf16().chain(std::iter::once(0)).collect();
    let name_w: Vec<u16> = "gdut-net-tray"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut hkey = HKEY::default();
    let ret = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            None,
            KEY_SET_VALUE,
            &mut hkey,
        )
    };
    if ret != ERROR_SUCCESS {
        return Ok(());
    }
    let _ = unsafe { RegDeleteValueW(hkey, PCWSTR(name_w.as_ptr())) };
    let _ = unsafe { RegCloseKey(hkey) };
    Ok(())
}

/// 托盘主体：主线程建菜单/图标 → 起 IPC 线程 → 跑 win32 消息泵。
pub fn run_tray() -> Result<()> {
    register_aumid();

    let snapshot: SharedSnapshot = Arc::new(Mutex::new(None));

    // 菜单在主线程创建；后台线程只经通道送状态文本。
    let status_item = MenuItem::new("Status: Disconnected", false, None);
    let redial_item = MenuItem::new("Redial now", true, None);
    let panel_item = MenuItem::new("Details", true, None);
    let quit_item = MenuItem::new("Exit", true, None);

    let menu = Menu::new();
    menu.append_items(&[&status_item, &redial_item, &panel_item, &quit_item])
        .context("Failed to build tray menu")?;

    // tray-icon 要求：创建图标与跑事件循环必须在同一线程（Windows 上是
    // win32 消息循环），主线程天然满足。
    let icon = tray_icon::Icon::from_rgba(tray_icon_rgba(), 32, 32)
        .context("Failed to build tray icon")?;
    let _tray = tray_icon::TrayIconBuilder::new()
        .with_tooltip("gdut-net — GDUT Wired Client")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(true)
        .build()
        .map_err(|e| anyhow!("Failed to create tray icon: {e}"))?;

    // IPC 线程 → 泵线程：状态文本；面板点击重拨也汇聚到泵线程统一发，
    // 避免两处并发建 PipeClient。
    let (status_tx, status_rx) = mpsc::channel::<String>();
    let (panel_redial_tx, panel_redial_rx) = mpsc::channel::<()>();
    {
        let snapshot = Arc::clone(&snapshot);
        let status_tx = status_tx.clone();
        std::thread::Builder::new()
            .name("gdut-net-tray-ipc".into())
            .spawn(move || ipc_loop(snapshot, status_tx))
            .context("Failed to start tray IPC thread")?;
    }

    let menu_rx = MenuEvent::receiver();
    let status_text = |state: Option<&StateSnapshot>| match state {
        None => "Status: Disconnected".to_string(),
        Some(s) => format!(
            "Status: {}{}",
            s.status_text(),
            s.ip.as_deref()
                .map(|ip| format!(" · IP {ip}"))
                .unwrap_or_default()
        ),
    };

    loop {
        // 限时泵：排空 win32 消息后让主线程周期醒来，处理菜单事件通道与
        // 跨线程通道（均可能无对应 win32 消息可排）。
        if pump_once(Some(Duration::from_millis(200)))? {
            // 还有积压消息：先不碰通道，下一拍继续排空。
            continue;
        }

        while let Ok(event) = menu_rx.try_recv() {
            if event.id == redial_item.id() {
                send_redial();
            } else if event.id == panel_item.id() {
                panel::show(Arc::clone(&snapshot), panel_redial_tx.clone());
            } else if event.id == quit_item.id() {
                std::process::exit(0);
            }
        }
        while panel_redial_rx.try_recv().is_ok() {
            send_redial();
        }
        // IPC 线程送来的最新文本（只保留最后一条即可）。
        let mut latest = None;
        while let Ok(text) = status_rx.try_recv() {
            latest = Some(text);
        }
        if let Some(text) = latest {
            status_item.set_text(text);
        }
        // 快照缓存兜底刷新（文本通道丢消息时也能收敛）。
        if let Ok(guard) = snapshot.lock() {
            let want = status_text(guard.as_ref());
            if status_item.text() != want {
                status_item.set_text(want);
            }
        }
    }
}

/// 发送 Redial 命令；失败记日志（服务大概率已停止，IPC 线程会 toast）。
fn send_redial() {
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(anyhow::Error::from)
        .and_then(|rt| {
            rt.block_on(async {
                let mut c = PipeClient::connect()?;
                c.send_cmd(Command::Redial).await
            })
        });
    if let Err(e) = result {
        log::warn!("Failed to send redial command: {e:#}");
    }
}

/// 后台线程主体：循环连接 IPC → 收快照更新缓存并送状态文本 → 断开则
/// toast 后重试。所有 MenuItem 操作由泵线程完成，本线程不碰 muda。
fn ipc_loop(snapshot: SharedSnapshot, status_tx: mpsc::Sender<String>) {
    let push_text = |snapshot: &SharedSnapshot| {
        let text = snapshot.lock().ok().map(|g| match g.as_ref() {
            None => "Status: Disconnected".to_string(),
            Some(s) => format!(
                "Status: {}{}",
                s.status_text(),
                s.ip.as_deref()
                    .map(|ip| format!(" · IP {ip}"))
                    .unwrap_or_default()
            ),
        });
        if let Some(text) = text {
            let _ = status_tx.send(text);
        }
    };

    // 托盘线程无全局 runtime，NamedPipeClient::open 要求 Handle::current()
    // 必须在 runtime 上下文内（tokio-1.53 named_pipe.rs:1005）。整条 IPC
    // 回路复用同一个 current_thread runtime，避免每次建 runtime 且让
    // PipeClient::connect 的 std::thread::sleep 不阻塞全局。
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to create tray IPC runtime");

    loop {
        let mut client = match rt.block_on(async { PipeClient::connect() }) {
            Ok(c) => c,
            Err(e) => {
                log::debug!("Tray failed to connect to service (retrying): {e:#}");
                std::thread::sleep(CONNECT_RETRY);
                continue;
            }
        };

        loop {
            let state = match rt.block_on(client.next_state()) {
                Ok(s) => s,
                Err(e) => {
                    log::debug!("Tray status stream disconnected (service stopped?): {e:#}");
                    break;
                }
            };
            if let Ok(mut guard) = snapshot.lock() {
                *guard = Some(state);
            }
            push_text(&snapshot);
        }

        // 服务断开：通知用户、清缓存与文本，回到重试连接循环。
        if let Err(e) = crate::notify::toast("gdut-net", "gdut-net service stopped") {
            log::warn!("Failed to toast service stopped: {e}");
        }
        if let Ok(mut guard) = snapshot.lock() {
            *guard = None;
        }
        push_text(&snapshot);
        std::thread::sleep(CONNECT_RETRY);
    }
}

/// 跑一拍 win32 消息泵。返回 true 表示处理了至少一条消息。
///
/// 有消息时排空队列并立即返回；无消息时按 `timeout`：
/// - `None`：`GetMessageW` 无限阻塞等下一条；
/// - 有值：`MsgWaitForMultipleObjects(QS_ALLINPUT)` 等到超时或有新消息，
///   让主线程有机会周期醒来处理跨线程通道。
fn pump_once(timeout: Option<Duration>) -> Result<bool> {
    use windows::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage,
        MSG, PM_REMOVE, QS_ALLINPUT,
    };

    let mut processed = false;
    loop {
        let mut msg = MSG::default();
        let has_msg = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool();
        if !has_msg {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        processed = true;
    }
    if processed {
        return Ok(true);
    }
    match timeout {
        None => {
            let mut msg = MSG::default();
            // 0 = WM_QUIT，-1 = 错误；托盘场景两者都应终止泵。
            let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
            if ret.0 <= 0 {
                return Err(anyhow!(
                    "Win32 message pump exited (GetMessageW = {})",
                    ret.0
                ));
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            Ok(true)
        }
        Some(d) => {
            let timeout_ms = u32::try_from(d.as_millis()).unwrap_or(INFINITE - 1);
            let wake = unsafe { MsgWaitForMultipleObjects(None, false, timeout_ms, QS_ALLINPUT) };
            debug_assert!(wake == WAIT_OBJECT_0 || wake == WAIT_TIMEOUT);
            Ok(false)
        }
    }
}
