//! 守护状态机：拨号 → 探测 → 掉线重拨的单一状态源。
//!
//! 平台能力通过 [`Dialer`]/[`Prober`] trait 注入，纯逻辑可在 Linux 上 TDD。
//! "掉线"以流量探测为准（不单看 RAS 状态）；`is_connected()==false`
//! 是可靠的即时掉线信号（上游语义：668 连接不存在归 Disconnected）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use crate::backoff::Backoff;
use crate::ipc::protocol::{HeartbeatStatus, SessionStatus, StateSnapshot};
use crate::probe::ProbeVerdict;
use crate::ras::ErrKind;

/// 会话稳定累计时长阈值：达到后重置退避与重拨计数。
const STABLE_RESET_AFTER: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Dialing,
    Connected,
    Backoff,
    AuthFail,
}

impl From<Phase> for SessionStatus {
    fn from(p: Phase) -> Self {
        match p {
            Phase::Idle => SessionStatus::Idle,
            Phase::Dialing => SessionStatus::Dialing,
            Phase::Connected => SessionStatus::Connected,
            Phase::Backoff => SessionStatus::Backoff,
            Phase::AuthFail => SessionStatus::AuthFail,
        }
    }
}

#[derive(Debug)]
pub struct DialError {
    pub kind: ErrKind,
    pub code: u32,
    pub msg: String,
}

#[async_trait]
pub trait Dialer: Send {
    async fn dial(&mut self) -> Result<(), DialError>;
    async fn hangup(&mut self);
    fn is_connected(&mut self) -> bool;
}

#[async_trait]
pub trait Prober: Send {
    async fn probe(&mut self) -> ProbeVerdict;
}

#[derive(Debug, Clone, Copy)]
pub struct WatchdogCfg {
    pub redial_min: Duration,
    pub redial_max: Duration,
    pub probe_interval: Duration,
    pub auth_fail_delay: Duration,
}

pub struct Watchdog {
    dialer: Box<dyn Dialer>,
    prober: Box<dyn Prober>,
    cfg: WatchdogCfg,
    phase: Phase,
    backoff: Backoff,
    attempts: u32,
    since: Option<SystemTime>,
    probe_fails: u8,
    stable_for: Duration,
    dial_calls: u32,
    last_drop_reason: Option<String>,
    /// IPC Redial 命令置位；run_once 顶部消费后立即 do_dial。
    redial_requested: AtomicBool,
}

impl Watchdog {
    pub fn new(
        dialer: impl Dialer + 'static,
        prober: impl Prober + 'static,
        cfg: WatchdogCfg,
    ) -> Self {
        let backoff = Backoff::new(cfg.redial_min, cfg.redial_max);
        Self {
            dialer: Box::new(dialer),
            prober: Box::new(prober),
            cfg,
            phase: Phase::Idle,
            backoff,
            attempts: 0,
            since: None,
            probe_fails: 0,
            stable_for: Duration::ZERO,
            dial_calls: 0,
            last_drop_reason: None,
            redial_requested: AtomicBool::new(false),
        }
    }

    pub fn snapshot(&self) -> StateSnapshot {
        StateSnapshot {
            status: self.phase.into(),
            since_unix: self
                .since
                .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()),
            ip: None,
            last_drop_reason: self.last_drop_reason.clone(),
            redial_attempts: self.attempts,
            heartbeat: HeartbeatStatus::Off,
        }
    }

    /// 测试辅助：累计拨号调用次数。
    pub fn dial_calls(&self) -> u32 {
        self.dial_calls
    }

    /// 退出收尾：挂断当前会话（若在）。
    pub async fn shutdown(&mut self) {
        self.dialer.hangup().await;
    }

    /// 请求立即重拨（IPC Redial 命令）：置一次性标志，下一次
    /// `run_once` 顶部消费并直接 `do_dial`，绕过当前相位的等待/探测。
    pub fn request_redial(&self) {
        self.redial_requested.store(true, Ordering::SeqCst);
    }

    /// 状态机单步：非 Connected 相位拨号，Connected 相位探测。
    /// 返回建议等待时长。
    pub async fn run_once(&mut self) -> Duration {
        if self.redial_requested.swap(false, Ordering::SeqCst) {
            log::info!("收到手动重拨请求，立即执行");
            // 会话仍活（RAS 层或刚建立）时直接二次 RasDial 会失败并陷入
            // Backoff 死循环：复用掉线路径——挂断 + 状态清理，再拨。
            if self.phase == Phase::Connected || self.dialer.is_connected() {
                self.record_drop("手动重拨");
                self.dialer.hangup().await;
            }
            return self.do_dial().await;
        }
        match self.phase {
            Phase::Connected => self.step_connected().await,
            _ => self.do_dial().await,
        }
    }

    async fn step_connected(&mut self) -> Duration {
        if !self.dialer.is_connected() {
            log::warn!("RAS 会话已不存在，判定掉线，立即重拨");
            self.record_drop("RAS 会话不存在");
            self.dialer.hangup().await;
            return self.do_dial().await;
        }
        let verdict = self.prober.probe().await;
        match verdict {
            ProbeVerdict::Alive => {
                self.probe_fails = 0;
                self.stable_for += self.cfg.probe_interval;
                if self.stable_for >= STABLE_RESET_AFTER && self.attempts > 0 {
                    log::info!(
                        "会话稳定 ≥{}s，重置退避与重拨计数（此前 {} 次）",
                        STABLE_RESET_AFTER.as_secs(),
                        self.attempts
                    );
                    self.backoff.reset();
                    self.attempts = 0;
                }
                self.cfg.probe_interval
            }
            bad @ (ProbeVerdict::LinkDown | ProbeVerdict::Kicked) => {
                self.probe_fails += 1;
                if self.probe_fails < 2 {
                    log::warn!(
                        "探测异常（{bad:?}），{}s 后复核",
                        (self.cfg.probe_interval / 4).as_secs()
                    );
                    self.cfg.probe_interval / 4
                } else {
                    log::warn!(
                        "探测连续 {} 次异常（{bad:?}），判定掉线，挂断并重拨",
                        self.probe_fails
                    );
                    self.record_drop(&format!("探测连续 {} 次异常（{bad:?}）", self.probe_fails));
                    self.dialer.hangup().await;
                    self.do_dial().await
                }
            }
        }
    }

    async fn do_dial(&mut self) -> Duration {
        self.phase = Phase::Dialing;
        self.dial_calls += 1;
        match self.dialer.dial().await {
            Ok(()) => {
                log::info!("拨号成功，会话建立");
                self.phase = Phase::Connected;
                self.since = Some(SystemTime::now());
                self.probe_fails = 0;
                self.stable_for = Duration::ZERO;
                self.cfg.probe_interval
            }
            Err(DialError {
                kind: ErrKind::Auth,
                code,
                msg,
            }) => {
                let delay = self.cfg.auth_fail_delay;
                log::error!("认证失败（{code}）：{msg}，{}s 后重试", delay.as_secs());
                self.phase = Phase::AuthFail;
                self.last_drop_reason = Some(format!("认证失败 {code}: {msg}"));
                delay
            }
            Err(DialError {
                kind: ErrKind::Transient,
                code,
                msg,
            }) => {
                let delay = self.backoff.next_delay();
                self.attempts = self.attempts.saturating_add(1);
                log::warn!(
                    "拨号失败（{code}）：{msg}，{}s 后重拨（第 {} 次）",
                    delay.as_secs(),
                    self.attempts
                );
                self.phase = Phase::Backoff;
                self.last_drop_reason = Some(format!("拨号失败 {code}: {msg}"));
                delay
            }
        }
    }

    fn record_drop(&mut self, reason: &str) {
        self.since = None;
        self.probe_fails = 0;
        self.stable_for = Duration::ZERO;
        self.last_drop_reason = Some(reason.to_string());
    }
}
