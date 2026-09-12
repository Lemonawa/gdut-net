//! 快照 → 人类可读状态的唯一转换器（托盘 / 日常界面 / CLI / setup 共用）。
//!
//! 本模块拥有：读卡灯语义（Light）与主状态（Primary）的优先级判定、
//! 中文词表（托盘状态行 / 日常窗口）、
//! 英文词表（CLI `status` 输出、模式确认与 setup 等待文案）。
//! 任何状态变体只在此处新增；渲染端（egui / tray-icon / stdout）只做适配。

use crate::ipc::protocol::{
    HeartbeatStatus, NetMode, SessionStatus, StateSnapshot, WPhase, WirelessSnapshot,
};

/// 读卡灯语义：托盘图标与界面状态灯的共用判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Light {
    Wired,
    Wireless,
    Busy,
    Off,
}

/// 页级主状态（日常界面大字 / 托盘状态行的第一语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Primary {
    ServiceDown,
    Connected,
    WirelessOnline,
    Dialing,
    Backoff,
    AuthFail,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusView {
    pub primary: Primary,
    pub light: Light,
}

/// 优先级：有线已连接 > 无线已接管 > 有线进行中/失败/空闲；无快照 = 服务未运行。
pub fn view(s: Option<&StateSnapshot>) -> StatusView {
    let Some(s) = s else {
        return StatusView {
            primary: Primary::ServiceDown,
            light: Light::Off,
        };
    };
    if s.status == SessionStatus::Connected {
        return StatusView {
            primary: Primary::Connected,
            light: Light::Wired,
        };
    }
    if s.wireless.phase == WPhase::Online {
        return StatusView {
            primary: Primary::WirelessOnline,
            light: Light::Wireless,
        };
    }
    let primary = match s.status {
        SessionStatus::Dialing => Primary::Dialing,
        SessionStatus::Backoff => Primary::Backoff,
        SessionStatus::AuthFail => Primary::AuthFail,
        SessionStatus::Idle => Primary::Idle,
        SessionStatus::Connected => unreachable!("connected handled above"),
    };
    let light = match primary {
        Primary::Idle => Light::Off,
        _ => Light::Busy,
    };
    StatusView { primary, light }
}

impl Primary {
    /// 中文短语（GUI 侧）。
    pub fn word_zh(self) -> &'static str {
        match self {
            Primary::ServiceDown => "服务未运行",
            Primary::Connected => "已连接",
            Primary::WirelessOnline => "无线接管",
            Primary::Dialing => "拨号中",
            Primary::Backoff => "重拨中",
            Primary::AuthFail => "认证失败",
            Primary::Idle => "空闲",
        }
    }

    /// 出口说明（小票字段）。
    pub fn egress_zh(self) -> &'static str {
        match self {
            Primary::Connected => "有线",
            Primary::WirelessOnline => "无线",
            _ => "—",
        }
    }
}

/// 会话状态中文（托盘状态行 / setup 等待文案）。
pub fn session_zh(s: SessionStatus) -> &'static str {
    match s {
        SessionStatus::Connected => "已连接",
        SessionStatus::Dialing => "拨号中",
        SessionStatus::Backoff => "重拨中",
        SessionStatus::AuthFail => "认证失败",
        SessionStatus::Idle => "空闲",
    }
}

/// 会话状态英文（CLI `status`；字符串逐字冻结，见 tests/status.rs）。
pub fn session_en(s: SessionStatus) -> &'static str {
    match s {
        SessionStatus::Idle => "Idle",
        SessionStatus::Dialing => "Dialing",
        SessionStatus::Connected => "Connected",
        SessionStatus::Backoff => "Backoff (retrying)",
        SessionStatus::AuthFail => "Auth failed",
    }
}

/// 无线相位中文。
pub fn wphase_zh(phase: WPhase) -> &'static str {
    match phase {
        WPhase::Off => "关闭",
        WPhase::Joining => "连接中",
        WPhase::Authing => "认证中",
        WPhase::Online => "已接管",
        WPhase::Error => "错误",
    }
}

/// 无线相位英文（CLI `status`；逐字冻结）。
pub fn wphase_en(phase: WPhase) -> &'static str {
    match phase {
        WPhase::Off => "Off",
        WPhase::Joining => "Joining",
        WPhase::Authing => "Authenticating",
        WPhase::Online => "Online",
        WPhase::Error => "Error",
    }
}

/// 心跳英文（CLI `status`；逐字冻结）。
pub fn heartbeat_en(h: &HeartbeatStatus) -> String {
    match h {
        HeartbeatStatus::Off => "Off".to_string(),
        HeartbeatStatus::Running => "Running".to_string(),
        HeartbeatStatus::Error(e) => format!("Error ({e})"),
    }
}

/// 模式英文（CLI `status` 与模式确认；逐字冻结）。
pub fn mode_en(m: NetMode) -> &'static str {
    match m {
        NetMode::WiredExclusive => "Wired only (auto wireless takeover)",
        NetMode::WiredPlusStandby => "Wired + wireless standby",
    }
}

/// 无线链路英文（相位 + IP / 错误后缀；逐字冻结）。
pub fn wireless_en(w: &WirelessSnapshot) -> String {
    let phase = wphase_en(w.phase);
    match (&w.ip, &w.last_error) {
        (Some(ip), _) => format!("{phase} {ip}"),
        (None, Some(e)) => format!("{phase} ({e})"),
        _ => phase.to_string(),
    }
}

/// 托盘状态行（菜单首项 + tooltip 共用）：None = 无快照按"服务未运行"。
pub fn status_line_zh(s: Option<&StateSnapshot>) -> String {
    let Some(s) = s else {
        return "服务未运行".to_string();
    };
    format!(
        "有线：{} · WiFi：{}",
        session_zh(s.status),
        wphase_zh(s.wireless.phase)
    )
}
