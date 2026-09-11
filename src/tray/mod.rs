//! 托盘常驻图标（tray-icon）+ 常驻 egui 状态窗口（按需显示/隐藏）。
//!
//! 事件循环模型：**主线程裸 win32 消息泵**（GetMessageW /
//! MsgWaitForMultipleObjects + 命名唤醒事件），tray-icon 在 Windows 要求
//! 创建图标的线程跑 win32 事件循环，主线程恰好满足且无需引入 winit 依赖。
//!
//! 线程模型：所有 MenuItem 操作（含 set_text）都在主泵线程完成——
//! muda 的 MenuItem 内含 Rc，不可跨线程。后台 IPC 线程只经 std mpsc
//! 发送"status text to display"，泵线程每拍取来应用到菜单项。
//!
//! PipeClient 的 async 方法由每次调用自建的极小 current_thread runtime
//! 驱动——托盘线程没有全局 tokio executor，不能假设 runtime 存在。

mod gui;

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
    REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE};

use crate::ipc::client::PipeClient;
use crate::ipc::protocol::{Command, NetMode, SessionStatus, StateSnapshot, WPhase};

use gui::{GuiShared, GuiState};

/// 后台线程 connect 失败后的重试间隔。
const CONNECT_RETRY: Duration = Duration::from_secs(3);

/// 共享快照缓存：None = 尚未收到（服务未运行/刚断开）。
pub(crate) type SharedSnapshot = Arc<Mutex<Option<StateSnapshot>>>;

/// 托盘单实例 mutex 名（内核对象；同一用户会话可见即可，无需 Global\ 前缀）。
const SINGLETON_NAME: &str = "gdut-net-tray-singleton";
/// 二次启动 → 主实例显示 GUI 的自动复位事件名。
const SHOW_EVENT_NAME: &str = "gdut-net-tray-show";

/// UTF-16 + NUL，Win32 W 接口参数。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 单实例判定结果。
enum Singleton {
    /// 首个实例：持有内核 mutex，进程退出自动释放。
    Primary(HANDLE),
    /// 已有托盘在跑：唤醒它的 GUI 后本进程退出。
    Secondary,
}

/// 单实例守卫；mutex 创建失败时降级为 Primary（无保护，不阻断托盘启动）。
fn acquire_singleton() -> Singleton {
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    let name = wide(SINGLETON_NAME);
    match unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) } {
        Ok(h) => {
            // CreateMutexW 成功但已存在同名对象：说明另一托盘持有它。
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                let _ = unsafe { CloseHandle(h) };
                Singleton::Secondary
            } else {
                Singleton::Primary(h)
            }
        }
        Err(e) => {
            log::warn!("Tray singleton mutex failed (continuing unguarded): {e}");
            Singleton::Primary(HANDLE::default())
        }
    }
}

/// 二次启动：置位命名事件，跨进程唤醒主实例显示 GUI。
///
/// 主实例未运行时事件不存在，本函数只创建后立即关闭（无副作用）。
fn signal_show() {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{CreateEventW, SetEvent};

    let name = wide(SHOW_EVENT_NAME);
    if let Ok(h) = unsafe { CreateEventW(None, false, false, PCWSTR(name.as_ptr())) } {
        let _ = unsafe { SetEvent(h) };
        let _ = unsafe { CloseHandle(h) };
    }
}

/// 从 cmd/PowerShell 启动时有控制台窗口；双击（windows 子系统）没有。
pub fn has_console() -> bool {
    let hwnd = unsafe { windows::Win32::System::Console::GetConsoleWindow() };
    !hwnd.0.is_null()
}

/// 双击 exe（无参数、无控制台）入口：已安装 → 托盘 + 弹 GUI；未安装 → 中文提示。
pub fn double_click_entry() -> Result<()> {
    match crate::service::install_state() {
        crate::service::InstallState::Installed { .. } => run_tray(true),
        crate::service::InstallState::NotInstalled => {
            message_box_install_hint();
            Ok(())
        }
    }
}

/// 未安装时的中文提示（GUI 场景用户可见，不受"控制台英文"约束）。
fn message_box_install_hint() {
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};

    let text =
        wide("gdut-net 尚未安装。\n\n请运行安装包 gdut-net-setup.exe，或从开始菜单打开安装程序。");
    let title = wide("GDUT Net");
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

/// 从泵线程显示/聚焦常驻 GUI：窗口线程活 → 唤醒；否则新建。
fn show_gui(
    gui: &GuiShared,
    snapshot: &SharedSnapshot,
    redial_tx: &mpsc::Sender<()>,
    setmode_tx: &mpsc::Sender<NetMode>,
) {
    gui::show_or_focus(
        Arc::clone(gui),
        Arc::clone(snapshot),
        redial_tx.clone(),
        setmode_tx.clone(),
    );
}

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

/// 从快照推图标语义：有线 Connected 优先绿灯（standby 有线健康时也显示有线在
/// 用，spec §8）；有线不在线才轮到无线 Online 蓝灯；无快照视为掉线灰灯。
fn icon_kind(s: Option<&StateSnapshot>) -> IconKind {
    match s {
        None => IconKind::Down,
        Some(s) if s.wireless.phase == WPhase::Online => match s.status {
            SessionStatus::Connected => IconKind::WiredUp,
            _ => IconKind::WirelessUp,
        },
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

/// 托盘状态行（菜单首项 + tooltip 共用）：None = 无快照按"服务未运行"。
///
/// 中文只覆盖 GUI 呈现（托盘/窗口）；CLI 的英文输出在
/// `ipc::protocol::StateSnapshot` 的 `*_text()`，协议层不动。
fn status_line(s: Option<&StateSnapshot>) -> String {
    let Some(s) = s else {
        return "服务未运行".to_string();
    };
    let wired = match s.status {
        SessionStatus::Connected => "已连接",
        SessionStatus::Dialing => "拨号中",
        SessionStatus::Backoff => "重拨中",
        SessionStatus::AuthFail => "认证失败",
        SessionStatus::Idle => "空闲",
    };
    let wifi = match s.wireless.phase {
        WPhase::Off => "关闭",
        WPhase::Joining => "连接中",
        WPhase::Authing => "认证中",
        WPhase::Online => "已接管",
        WPhase::Error => "错误",
    };
    format!("有线：{wired} · WiFi：{wifi}")
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

/// 注册托盘自启（HKCU Run）：值指向显式传入的托盘 exe（setup 场景 current_exe 是 setup 自己）。
pub fn register_autostart(tray_exe: &std::path::Path) -> Result<()> {
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let value = format!("\"{}\" tray", tray_exe.display());
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
///
/// `show_gui_at_start=true`（双击入口）：托盘就绪后立刻弹出 GUI。
/// 已有实例在跑时改为置位命名事件唤出它的窗口，本进程直接退出。
pub fn run_tray(show_gui_at_start: bool) -> Result<()> {
    // 先装文件日志（托盘无 stderr），单实例守卫的 warn 也得有落点；
    // 句柄活到进程退出（泵循环内不 drop），Secondary 分支安装后即退出，无害。
    let _logger = crate::logging::init_tray_logging(r"C:\ProgramData\gdut-net\logs", "tray");
    let _singleton = match acquire_singleton() {
        Singleton::Primary(h) => h,
        Singleton::Secondary => {
            signal_show();
            return Ok(());
        }
    };
    register_aumid();

    let snapshot: SharedSnapshot = Arc::new(Mutex::new(None));

    // 菜单在主线程创建；后台线程只经通道送状态文本。
    // 面孔中文（GUI 场景用户可见）；CLI 输出保持英文。
    let status_item = MenuItem::new(status_line(None), false, None);
    let sep1 = PredefinedMenuItem::separator();
    let mode_exclusive = CheckMenuItem::new("有线优先（自动无线接管）", true, true, None);
    let mode_standby = CheckMenuItem::new("有线 + 无线备用", true, false, None);
    let sep2 = PredefinedMenuItem::separator();
    let redial_item = MenuItem::new("立即重拨", true, None);
    let panel_item = MenuItem::new("打开主界面", true, None);
    let sep3 = PredefinedMenuItem::separator();
    let quit_item = MenuItem::new("退出托盘", true, None);

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
        .with_tooltip(format!("gdut-net — {}", status_line(None)))
        .with_icon(
            icon_for(&icons, IconKind::Down)
                .cloned()
                .context("Failed to pick initial tray icon")?,
        )
        .with_menu(Box::new(menu))
        // 左键留给日常 GUI；右键仍弹原生菜单。
        .with_menu_on_left_click(false)
        .build()
        .map_err(|e| anyhow!("Failed to create tray icon: {e}"))?;
    let mut last_kind = IconKind::Down;
    let mut last_tooltip = status_line(None);

    // IPC 线程 → 泵线程：状态文本；面板点击重拨也汇聚到泵线程统一发，
    // 避免两处并发建 PipeClient。
    let (status_tx, status_rx) = mpsc::channel::<String>();
    let (panel_redial_tx, panel_redial_rx) = mpsc::channel::<()>();
    let (panel_setmode_tx, panel_setmode_rx) = mpsc::channel::<NetMode>();
    {
        let snapshot = Arc::clone(&snapshot);
        let status_tx = status_tx.clone();
        std::thread::Builder::new()
            .name("gdut-net-tray-ipc".into())
            .spawn(move || ipc_loop(snapshot, status_tx))
            .context("Failed to start tray IPC thread")?;
    }

    // 命名自动复位事件：二次启动的进程 SetEvent → 本进程 MsgWait 醒来弹 GUI。
    let wake_event = {
        let name = wide(SHOW_EVENT_NAME);
        unsafe { CreateEventW(None, false, false, PCWSTR(name.as_ptr())) }
            .context("Failed to create tray show event")?
    };
    let gui: GuiShared = Arc::new(Mutex::new(GuiState::Absent));
    if show_gui_at_start {
        show_gui(&gui, &snapshot, &panel_redial_tx, &panel_setmode_tx);
    }

    let menu_rx = MenuEvent::receiver();
    let tray_rx = tray_icon::TrayIconEvent::receiver();

    loop {
        // 限时泵 + 命名事件：排空 win32 消息后让主线程周期醒来，处理菜单/
        // 托盘事件与跨线程通道（均可能无对应 win32 消息可排）。
        let pump = pump_once(Some(Duration::from_millis(200)), Some(&wake_event))?;
        if pump.processed {
            // 还有积压消息：先不碰通道，下一拍继续排空。
            continue;
        }
        if pump.woke {
            log::info!("Show-GUI request received");
            show_gui(&gui, &snapshot, &panel_redial_tx, &panel_setmode_tx);
        }

        // 左键单击托盘 → 打开日常 GUI（菜单已改为只右键弹）。
        while let Ok(ev) = tray_rx.try_recv() {
            if let tray_icon::TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Up,
                ..
            } = ev
            {
                show_gui(&gui, &snapshot, &panel_redial_tx, &panel_setmode_tx);
            }
        }
        while let Ok(event) = menu_rx.try_recv() {
            if event.id == *mode_exclusive.id() {
                send_set_mode(NetMode::WiredExclusive);
            } else if event.id == *mode_standby.id() {
                send_set_mode(NetMode::WiredPlusStandby);
            } else if event.id == *redial_item.id() {
                send_redial();
            } else if event.id == *panel_item.id() {
                show_gui(&gui, &snapshot, &panel_redial_tx, &panel_setmode_tx);
            } else if event.id == *quit_item.id() {
                std::process::exit(0);
            }
        }
        while panel_redial_rx.try_recv().is_ok() {
            send_redial();
        }
        while let Ok(mode) = panel_setmode_rx.try_recv() {
            send_set_mode(mode);
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
        // tooltip、图标，都是变了才 set（幂等，且避免每拍 syscall 抖动）。
        if let Ok(guard) = snapshot.lock() {
            let want_status = status_line(guard.as_ref());
            if status_item.text() != want_status {
                status_item.set_text(&want_status);
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
                last_kind = kind;
            }
            // tooltip 与菜单首行用同一句话；状态文本变了就更新（图标可能没变，
            // 例如 退避重拨 → 认证失败 同属 Backoff 灯）。
            if last_tooltip != want_status {
                tray.set_tooltip(Some(format!("gdut-net — {want_status}")))
                    .ok();
                last_tooltip = want_status;
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

/// 一拍泵结果：`processed` = 处理过消息（应 continue）；`woke` = 命名事件触发。
struct Pump {
    processed: bool,
    woke: bool,
}

/// 跑一拍 win32 消息泵。`wake` 须为自动复位事件句柄。
///
/// 有消息时排空队列并立即返回；无消息时按 `timeout`：
/// - `None`：`GetMessageW` 无限阻塞等下一条；
/// - 有值：`MsgWaitForMultipleObjects(QS_ALLINPUT)` 等到超时、有新消息，
///   或 `wake` 事件置位（跨进程"显示 GUI"请求）。
fn pump_once(timeout: Option<Duration>, wake: Option<&HANDLE>) -> Result<Pump> {
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
        return Ok(Pump {
            processed: true,
            woke: false,
        });
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
            Ok(Pump {
                processed: true,
                woke: false,
            })
        }
        Some(d) => {
            let timeout_ms = u32::try_from(d.as_millis()).unwrap_or(INFINITE - 1);
            let handles = wake.map(std::slice::from_ref);
            let res = unsafe { MsgWaitForMultipleObjects(handles, false, timeout_ms, QS_ALLINPUT) };
            if res == WAIT_TIMEOUT {
                return Ok(Pump {
                    processed: false,
                    woke: false,
                });
            }
            // 句柄下标 0 = wake 事件（自动复位会清除信号）；其余为输入待排空。
            Ok(Pump {
                processed: false,
                woke: res == WAIT_OBJECT_0,
            })
        }
    }
}
