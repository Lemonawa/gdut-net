use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Idle,
    Dialing,
    Connected,
    Backoff,
    AuthFail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeartbeatStatus {
    Off,
    Running,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub status: SessionStatus,
    pub since_unix: Option<u64>,
    pub ip: Option<String>,
    pub last_drop_reason: Option<String>,
    pub redial_attempts: u32,
    pub heartbeat: HeartbeatStatus,
}

impl StateSnapshot {
    /// 状态的中文描述（`status` 子命令与托盘共用）。
    pub fn status_text(&self) -> String {
        match self.status {
            SessionStatus::Idle => "Idle",
            SessionStatus::Dialing => "Dialing",
            SessionStatus::Connected => "Connected",
            SessionStatus::Backoff => "Backoff (retrying)",
            SessionStatus::AuthFail => "Auth failed",
        }
        .to_string()
    }

    /// 在线时长 `HH:MM:SS`（自 `since_unix` 起算）；无会话为 `—`。
    pub fn uptime_text(&self) -> String {
        self.since_unix.map_or_else(
            || "—".to_string(),
            |t| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                format_uptime(now.saturating_sub(t))
            },
        )
    }

    /// 心跳状态的中文描述。
    pub fn heartbeat_text(&self) -> String {
        match &self.heartbeat {
            HeartbeatStatus::Off => "Off".to_string(),
            HeartbeatStatus::Running => "Running".to_string(),
            HeartbeatStatus::Error(e) => format!("Error ({e})"),
        }
    }
}

/// 秒数 → `HH:MM:SS`（小时不封顶）。
pub fn format_uptime(secs: u64) -> String {
    format!("{}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    State { state: StateSnapshot },
    Ack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Command {
    Redial,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    Cmd { c: Command },
}

pub fn encode_frame<T: Serialize>(msg: &T) -> Vec<u8> {
    let mut out = serde_json::to_vec(msg).expect("serialize to JSON cannot fail");
    out.push(b'\n');
    out
}

#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn feed(&mut self, chunk: &[u8]) -> VecDeque<Vec<u8>> {
        self.buf.extend_from_slice(chunk);
        let mut frames = VecDeque::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let frame: Vec<u8> = self.buf.drain(..=pos).collect();
            frames.push_back(frame);
        }
        frames
    }
}
