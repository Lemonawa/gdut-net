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
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
    REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::System::Threading::INFINITE;

use crate::ipc::client::PipeClient;
use crate::ipc::protocol::{Command, NetMode, SessionStatus, StateSnapshot, WPhase};

/// 后台线程 connect 失败后的重试间隔。
const CONNECT_RETRY: Duration = Duration::from_secs(3);

/// 共享快照缓存：None = 尚未收到（服务未运行/刚断开）。
pub(crate) type SharedSnapshot = Arc<Mutex<Option<StateSnapshot>>>;

/// 32x32 RGBA 方块图标（代码生成，无需二进制图片资源）：2px 透明留白，
/// 托盘里不顶边。
fn icon_rgba(color: [u8; 3]) -> Vec<u8> {
    const S: usize = 32;
    let [r, g, b] = color;
    let mut rgba = Vec::with_capacity(S * S * 4);
    let inner = 2..S - 2;
    for y in 0..S {
        for x in 0..S {
            let a = if inner.contains(&x) && inner.contains(&y) {
                0xff
            } else {
                0x00
            };
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    rgba
}

/// 托盘图标语义：无线在线（蓝）/ 有线在线（绿）/ 重试中（黄）/ 掉线（灰）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IconKind {
    /// 有线 PPP 在线。
    WiredUp,
    /// 无线已接管在线。
    WirelessUp,
    /// 重拨退避/认证失败/拨号中。
    Backoff,
    /// 空闲或无快照。
    Down,
}

impl IconKind {
    /// 图标配色：绿 `2ec48a` / 蓝 `309cdc` / 黄 `e0b000` / 灰 `888888`。
    const ALL: [IconKind; 4] = [
        IconKind::WiredUp,
        IconKind::WirelessUp,
        IconKind::Backoff,
        IconKind::Down,
    ];

    fn color(self) -> [u8; 3] {
        match self {
            IconKind::WiredUp => [0x2e, 0xc4, 0x8a],
            IconKind::WirelessUp => [0x30, 0x9c, 0xdc],
            IconKind::Backoff => [0xe0, 0xb0, 0x00],
            IconKind::Down => [0x88, 0x88, 0x88],
        }
    }
}

/// 从快照推图标语义：无线 Online 优先于有线状态（接管即蓝灯）；无快照
/// 视为掉线灰灯。
fn icon_kind(s: Option<&StateSnapshot>) -> IconKind {
    match s {
        None => IconKind::Down,
        Some(s) if s.wireless.phase == WPhase::Online => IconKind::WirelessUp,
        Some(s) => match s.status {
            SessionStatus::Connected => IconKind::WiredUp,
            SessionStatus::Backoff | SessionStatus::AuthFail | SessionStatus::Dialing => {
                IconKind::Backoff
            }
            SessionStatus::Idle => IconKind::Down,
        },
    }
}

/// 按语义取预建图标（ALL 顺序与 `icons` 构建顺序一致）。
fn icon_for(icons: &[tray_icon::Icon], kind: IconKind) -> Option<&tray_icon::Icon> {
    IconKind::ALL
        .iter()
        .position(|k| *k == kind)
        .and_then(|idx| icons.get(idx))
}

/// 托盘状态行（菜单首项 + IPC 推送共用）：None = 无快照按掉线展示。
fn status_line(s: Option<&StateSnapshot>) -> String {
    match s {
        None => "Wired: Disconnected".to_string(),
        Some(s) => format!("Wired: {} · WiFi: {}", s.status_text(), s.wireless_text()),
    }
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
    let status_item = MenuItem::new("Wired: Disconnected", false, None);
    let sep1 = PredefinedMenuItem::separator();
    let mode_exclusive =
        CheckMenuItem::new("Wired only (auto wireless takeover)", true, true, None);
    let mode_standby = CheckMenuItem::new("Wired + wireless standby", true, false, None);
    let sep2 = PredefinedMenuItem::separator();
    let redial_item = MenuItem::new("Redial now", true, None);
    let panel_item = MenuItem::new("Details", true, None);
    let sep3 = PredefinedMenuItem::separator();
    let quit_item = MenuItem::new("Exit", true, None);

    let menu = Menu::new();
    menu.append_items(&[
        &status_item,
        &sep1,
        &mode_exclusive,
        &mode_standby,
        &sep2,
        &redial_item,
        &panel_item,
        &sep3,
        &quit_item,
    ])
    .context("Failed to build tray menu")?;

    // 四种状态色图标预先建好；泵线程只做 set_icon 切换（muda/tray-icon
    // 操作必须留在创建线程）。初值 Down 灰灯，与首个快照到达前的状态一致。
    let icons: Vec<tray_icon::Icon> = IconKind::ALL
        .iter()
        .map(|kind| tray_icon::Icon::from_rgba(icon_rgba(kind.color()), 32, 32))
        .collect::<Result<_, _>>()
        .context("Failed to build tray icons")?;

    // tray-icon 要求：创建图标与跑事件循环必须在同一线程（Windows 上是
    // win32 消息循环），主线程天然满足。`tray` 必须保活：drop 会移除托盘
    // 图标。
    let tray = tray_icon::TrayIconBuilder::new()
        .with_tooltip("gdut-net — Wired: Disconnected / WiFi: Off")
        .with_icon(
            icon_for(&icons, IconKind::Down)
                .cloned()
                .context("Failed to pick initial tray icon")?,
        )
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(true)
        .build()
        .map_err(|e| anyhow!("Failed to create tray icon: {e}"))?;
    let mut last_kind = IconKind::Down;

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

    loop {
        // 限时泵：排空 win32 消息后让主线程周期醒来，处理菜单事件通道与
        // 跨线程通道（均可能无对应 win32 消息可排）。
        if pump_once(Some(Duration::from_millis(200)))? {
            // 还有积压消息：先不碰通道，下一拍继续排空。
            continue;
        }

        while let Ok(event) = menu_rx.try_recv() {
            if event.id == *mode_exclusive.id() {
                send_set_mode(NetMode::WiredExclusive);
            } else if event.id == *mode_standby.id() {
                send_set_mode(NetMode::WiredPlusStandby);
            } else if event.id == *redial_item.id() {
                send_redial();
            } else if event.id == *panel_item.id() {
                panel::show(Arc::clone(&snapshot), panel_redial_tx.clone());
            } else if event.id == *quit_item.id() {
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
        // 快照缓存兜底刷新（文本通道丢消息时也能收敛）：状态行、模式勾选、
        // 图标/tooltip 变更才 set（幂等，且避免每拍 syscall 抖动）。
        if let Ok(guard) = snapshot.lock() {
            let want_status = status_line(guard.as_ref());
            if status_item.text() != want_status {
                status_item.set_text(want_status);
            }
            let mode = guard.as_ref().map_or_else(NetMode::default, |s| s.mode);
            mode_exclusive.set_checked(mode == NetMode::WiredExclusive);
            mode_standby.set_checked(mode == NetMode::WiredPlusStandby);
            let kind = icon_kind(guard.as_ref());
            if kind != last_kind {
                if let Some(icon) = icon_for(&icons, kind) {
                    if let Err(e) = tray.set_icon(Some(icon.clone())) {
                        log::warn!("Failed to update tray icon: {e}");
                    }
                }
                let tooltip = match guard.as_ref() {
                    None => "gdut-net — service not running".to_string(),
                    Some(s) => format!(
                        "gdut-net — Wired: {} / WiFi: {}",
                        s.status_text(),
                        s.wireless_text()
                    ),
                };
                tray.set_tooltip(Some(tooltip)).ok();
                last_kind = kind;
            }
        }
    }
}

/// 发送 Redial 命令；失败记日志（服务大概率已停止，IPC 线程会 toast）。
fn send_redial() {
    send_cmd_logged("redial", Command::Redial);
}

/// 发送 SetMode 命令（托盘模式勾选项）；失败记日志，实际生效以服务端
/// 下一条快照回显的 mode 为准（泵线程会重设勾选）。
fn send_set_mode(mode: NetMode) {
    send_cmd_logged("set mode", Command::SetMode { mode });
}

/// 单次命令发送：current_thread runtime + 连管道 + 发帧，失败只记日志。
fn send_cmd_logged(what: &str, cmd: Command) {
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(anyhow::Error::from)
        .and_then(|rt| {
            rt.block_on(async {
                let mut c = PipeClient::connect()?;
                c.send_cmd(cmd).await
            })
        });
    if let Err(e) = result {
        log::warn!("Failed to send {what} command: {e:#}");
    }
}

/// 后台线程主体：循环连接 IPC → 收快照更新缓存并送状态文本 → 断开则
/// toast 后重试。所有 MenuItem 操作由泵线程完成，本线程不碰 muda。
fn ipc_loop(snapshot: SharedSnapshot, status_tx: mpsc::Sender<String>) {
    let push_text = |snapshot: &SharedSnapshot| {
        let text = snapshot.lock().ok().map(|g| status_line(g.as_ref()));
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
