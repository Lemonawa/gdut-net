//! 服务组合核心：`Event -> Reaction` 的纯同步 reducer（守护 + 无线接管 + 事件环 + 快照）。
//!
//! cfg-free：不依赖 `windows::`/tokio/真实时间（时间只经 [`Clock`] 读取）。效果由壳
//! （Task 9 的 `runtime.rs`）执行，结果以 [`Event`] 回灌。设计见 W8 选型 B
//! （Supervisor + Main/Wireless 双车道）：`Reaction.snapshot` 是全服务唯一快照发布
//! 出口，`Reaction.wake_at` 是唯一睡眠依据（绝对单调毫秒）。
//!
//! 不变量索引（I1–I11，完整文本见 Task 8 brief；每条在 `tests/supervisor.rs` 有断言）：
//! 纯粹性 / 快照单出口 / 步进三触发点 / 双车道 / 链路门控 / 无线生命周期 / 绝对唤醒 /
//! 通知节流 / 结果护栏 / 生命周期 / 降级。
//!
//! 执行器义务（Task 9 壳）：Main 车道按序内联执行并回灌结果；Wireless 车道交 worker
//! 串行执行；快照仅在 `Reaction.snapshot` 为 `Some` 时经唯一发布点推送；睡眠用 `wake_at`。

use std::net::Ipv4Addr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::ipc::protocol::{
    Command, EventLog, HeartbeatStatus, NetMode, SessionStatus, StateSnapshot, WPhase,
    WirelessSnapshot,
};
use crate::probe::ProbeVerdict;
use crate::watchdog::{SessionView, LINK_DOWN_RETRY};
use crate::wireless::{portal, Action, Brain, World, JOIN_TIMEOUT_SECS};

/// 已知链路可用时的链路轮询间隔（既有 2s 节拍）。
const LINK_POLL_MS: u64 = 2_000;
/// 无线世界采样节拍（既有 manager 2s 拍）。
const WIRELESS_TICK_MS: u64 = 2_000;
/// 采样结果缺席时的重试窗口：只推账本，绝不重发在飞效果（I11）。
const SAMPLE_RETRY_MS: u64 = 2_000;
/// eportal 单次认证墙钟上限（真机：绑源 SYN 偶发被丢 + SYN 重传，20s 兜底）。
const PORTAL_AUTH_TIMEOUT: Duration = Duration::from_secs(20);
/// /32 路由写入后的数据面生效等待（真机实测 2–8s 竞态窗口）。
const ROUTE_SETTLE: Duration = Duration::from_secs(3);
/// 同一原因 Toast 的最小间隔。
const NOTIFY_THROTTLE_MS: u64 = 30 * 60 * 1000;
/// 连续重拨失败累计多久后弹 Toast。
const REDIAL_FAILING_TOAST_AFTER_MS: u64 = 10 * 60 * 1000;

/// 进程内单调时间源：core 唯一的时间依赖。
pub trait Clock: Send {
    fn now_ms(&self) -> u64;
    fn wall_secs(&self) -> u64;
}

/// 生产时钟：单调毫秒自构造起算；墙钟为 UNIX 秒。
pub struct SystemClock {
    start: std::time::Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            start: std::time::Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn wall_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

/// 无线采样（cfg-free；壳侧从 `adapter::AdapterInfo` 映射，禁止 core 依赖 Windows 半边）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WlanSample {
    pub ipv4: Ipv4Addr,
    pub gateway: Option<Ipv4Addr>,
    pub ifindex: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WirelessSample {
    pub associated: bool,
    pub wlan: Option<WlanSample>,
}

/// eportal 认证结果（壳侧执行 `portal_get` 后回灌）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortalAttempt {
    Replied { status: u16, body: String },
    NoReply,
    TimedOut,
    Failed(String),
}

/// `Effect::StepWatchdog` 的结果：延迟 + 会话事实 + PPP IP（一次采样的组合快照）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchdogStep {
    pub delay: Duration,
    pub session: SessionView,
    pub ppp_ip: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToastKey {
    HeartbeatError,
    RedialFailing,
}

impl ToastKey {
    fn as_str(self) -> &'static str {
        match self {
            ToastKey::HeartbeatError => "heartbeat_error",
            ToastKey::RedialFailing => "redial_failing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// 装配完成后恰好一次。
    Started,
    /// 执行器睡到 `Reaction.wake_at` 后触发。
    Wake,
    /// IPC Redial / SetMode。
    Command(Command),
    /// 2s 链路采样结果。
    LinkSampled(Option<bool>),
    /// `Effect::StepWatchdog` 的结果。
    WatchdogStepped(WatchdogStep),
    /// 2s 无线采样结果。
    WirelessSampled(WirelessSample),
    AssociateFinished(Result<(), String>),
    PortalFinished(PortalAttempt),
    WlanProbeFinished(ProbeVerdict),
    Heartbeat(HeartbeatStatus),
    NotifyResult {
        key: ToastKey,
        delivered: bool,
    },
    /// 无线 lane worker panic/退出。
    WirelessDied,
    Stop,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    // 有线守护（Main lane 内联执行）
    StepWatchdog,
    RequestRedial,
    SetWatchdogLink(Option<bool>),
    Hangup,
    // 采样
    SampleLink,
    SampleWireless,
    // 无线接管（Wireless lane worker 串行执行）
    CleanupStaleRoutes,
    Associate(String),
    Disassociate,
    EnsureRoutes {
        dests: Vec<Ipv4Addr>,
        gateway: Ipv4Addr,
        ifindex: u32,
    },
    SuppressMetric {
        ifindex: u32,
        target: u32,
    },
    ReleaseMetric,
    TeardownRoutes,
    /// /32 路由生效等待（3s）。
    Settle(Duration),
    /// URL 由壳侧构建（含密码，绝不进 Effect/Debug/日志）。
    PortalAuth {
        src_ip: Ipv4Addr,
        timeout: Duration,
    },
    WlanProbe {
        src_ip: Ipv4Addr,
        gateway: Option<Ipv4Addr>,
        url: String,
    },
    // 钩子
    PersistMode(NetMode),
    Notify {
        key: ToastKey,
        title: String,
        body: String,
    },
}

impl Effect {
    /// 纯分类函数：每条效果恰好一条车道（Main = 事件处理内联，Wireless = worker 串行）。
    pub fn lane(&self) -> Lane {
        match self {
            Effect::StepWatchdog
            | Effect::RequestRedial
            | Effect::SetWatchdogLink(_)
            | Effect::Hangup
            | Effect::SampleLink
            | Effect::SampleWireless
            | Effect::PersistMode(_)
            | Effect::Notify { .. } => Lane::Main,
            Effect::CleanupStaleRoutes
            | Effect::Associate(_)
            | Effect::Disassociate
            | Effect::EnsureRoutes { .. }
            | Effect::SuppressMetric { .. }
            | Effect::ReleaseMetric
            | Effect::TeardownRoutes
            | Effect::Settle(_)
            | Effect::PortalAuth { .. }
            | Effect::WlanProbe { .. } => Lane::Wireless,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Main,
    Wireless,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reaction {
    pub effects: Vec<Effect>,
    /// Some ⇔ 组合快照变化（全服务唯一发布出口）。
    pub snapshot: Option<StateSnapshot>,
    /// 绝对单调毫秒；None = 无定时器。
    pub wake_at: Option<u64>,
}

/// 服务总控（守护 + 无线接管 + 事件环 + 快照的组合决策点）。
/// 不 derive Debug/Clone（内部持有密码明文）。
pub struct Supervisor {
    clock: Box<dyn Clock>,
    cfg: Config,
    password: String,
    // 生命周期
    started: bool,
    stopped: bool,
    // 模式 / 事件环 / 快照账本
    mode: NetMode,
    events: EventLog,
    last_snapshot: Option<StateSnapshot>,
    // 有线守护
    eth_link: Option<bool>,
    link_sample_seen: bool,
    sample_in_flight: bool,
    step_pending: bool,
    watchdog_deadline: Option<u64>,
    link_poll_at: u64,
    session: SessionView,
    ppp_ip: Option<String>,
    heartbeat: HeartbeatStatus,
    failing_since_ms: Option<u64>,
    // 通知节流
    notify_in_flight: Vec<ToastKey>,
    notify_delivered_at: Vec<(ToastKey, u64)>,
    // 无线接管
    brain: Option<Brain>,
    wireless_died: bool,
    wireless_stopped_reported: bool,
    wireless_sample_in_flight: bool,
    wireless_tick_at: u64,
    wlan: Option<WlanSample>,
    wireless_ip: Option<String>,
    verdict: Option<ProbeVerdict>,
    associate_in_flight: bool,
    auth_in_flight: bool,
    probe_in_flight: bool,
    join_since_ms: Option<u64>,
    metric_suppressed_ifindex: Option<u32>,
    portal_ip: Option<Ipv4Addr>,
    probe_ip: Option<Ipv4Addr>,
}

impl Supervisor {
    pub fn new(cfg: Config, password: String, clock: impl Clock + 'static) -> Self {
        let mode = cfg.wireless.mode;
        let mut sup = Self {
            clock: Box::new(clock),
            cfg,
            password,
            started: false,
            stopped: false,
            mode,
            events: EventLog::new(),
            last_snapshot: None,
            eth_link: None,
            link_sample_seen: false,
            sample_in_flight: false,
            step_pending: false,
            watchdog_deadline: None,
            link_poll_at: u64::MAX,
            session: SessionView {
                status: SessionStatus::Idle,
                since_unix: None,
                last_drop_reason: None,
                redial_attempts: 0,
            },
            ppp_ip: None,
            heartbeat: HeartbeatStatus::Off,
            failing_since_ms: None,
            notify_in_flight: Vec::new(),
            notify_delivered_at: Vec::new(),
            brain: None,
            wireless_died: false,
            wireless_stopped_reported: false,
            wireless_sample_in_flight: false,
            wireless_tick_at: u64::MAX,
            wlan: None,
            wireless_ip: None,
            verdict: None,
            associate_in_flight: false,
            auth_in_flight: false,
            probe_in_flight: false,
            join_since_ms: None,
            metric_suppressed_ifindex: None,
            portal_ip: None,
            probe_ip: None,
        };
        if sup.cfg.wireless.enabled {
            match (
                crate::probe::parse_http_probe_target(&sup.cfg.wireless.portal_url),
                sup.cfg.wireless.probe_host.parse::<Ipv4Addr>(),
            ) {
                (Some((portal_ip, _)), Ok(probe_ip)) => {
                    log::info!(
                        "Wireless manager started (mode {:?}, portal {portal_ip}, probe {probe_ip})",
                        sup.mode
                    );
                    sup.brain = Some(Brain::new(
                        sup.mode,
                        sup.cfg.wireless.takeover_after_secs,
                        sup.cfg.wireless.release_after_secs,
                        sup.cfg.dial.probe_interval_secs,
                    ));
                    sup.portal_ip = Some(portal_ip);
                    sup.probe_ip = Some(probe_ip);
                }
                _ => {
                    log::error!("Wireless: portal_url/probe_host invalid, manager disabled");
                }
            }
        }
        sup
    }

    /// 明文密码唯一所有者（Task 9 的无线 worker 构建 portal URL 用；绝不日志/Debug）。
    pub fn password(&self) -> &str {
        &self.password
    }

    /// 纯同步 reducer：任意事件序不 panic、不返回 Err。
    pub fn on(&mut self, event: Event) -> Reaction {
        // 时间只取一次：同一 (状态, 事件, 时钟) 必得同一 Reaction（I1）。
        let now = self.clock.now_ms();
        let wall = self.clock.wall_secs();
        let mut effects = Vec::new();
        let mut handled = false;

        if !self.started {
            if matches!(event, Event::Started) {
                self.handle_started(now, wall, &mut effects);
                handled = true;
            }
        } else if !self.stopped {
            handled = true;
            match event {
                Event::Started => {} // 幂等：Started 恰好一次
                Event::Wake => self.handle_wake(now, &mut effects),
                Event::Command(cmd) => self.handle_command(cmd, wall, &mut effects),
                Event::LinkSampled(up) => self.handle_link_sampled(up, now, wall, &mut effects),
                Event::WatchdogStepped(step) => {
                    self.handle_watchdog_stepped(step, now, &mut effects)
                }
                Event::WirelessSampled(sample) => {
                    self.handle_wireless_sampled(sample, now, wall, &mut effects)
                }
                Event::AssociateFinished(res) => self.handle_associate_finished(res, now),
                Event::PortalFinished(attempt) => self.handle_portal_finished(attempt, now, wall),
                Event::WlanProbeFinished(verdict) => self.handle_probe_finished(verdict, wall),
                Event::Heartbeat(status) => self.handle_heartbeat(status, now, &mut effects),
                Event::NotifyResult { key, delivered } => {
                    self.handle_notify_result(key, delivered, now)
                }
                Event::WirelessDied => self.handle_wireless_died(wall),
                Event::Stop => self.handle_stop(wall, &mut effects),
            }
        }

        let snapshot = if handled { self.publish() } else { None };
        let wake_at = if self.started && !self.stopped {
            self.next_wake(now)
        } else {
            None
        };
        Reaction {
            effects,
            snapshot,
            wake_at,
        }
    }

    /// 组合快照（watch 通道初值；与 `Reaction.snapshot` 同一组装函数）。
    pub fn snapshot(&self) -> StateSnapshot {
        self.compose()
    }

    /// 快照单出口 + 变化门控：Some ⇔ 组合快照与上次已发布值不同。
    fn publish(&mut self) -> Option<StateSnapshot> {
        let snap = self.compose();
        if self.last_snapshot.as_ref() == Some(&snap) {
            None
        } else {
            self.last_snapshot = Some(snap.clone());
            Some(snap)
        }
    }

    /// 绝对唤醒账本：watchdog deadline / 链路轮询 / 无线节拍 的最小值。
    fn next_wake(&self, now: u64) -> Option<u64> {
        let mut next = self.link_poll_at;
        if let Some(deadline) = self.watchdog_deadline {
            next = next.min(deadline);
        }
        if self.brain.is_some() {
            next = next.min(self.wireless_tick_at);
        }
        Some(next.max(now))
    }

    /// 已知链路态下的轮询间隔：拔线用 watchdog 的 5s 节奏。
    fn link_poll_interval_ms(&self) -> u64 {
        if self.eth_link == Some(false) {
            duration_ms(LINK_DOWN_RETRY)
        } else {
            LINK_POLL_MS
        }
    }

    fn handle_started(&mut self, now: u64, wall: u64, effects: &mut Vec<Effect>) {
        self.started = true;
        self.events.push(
            wall,
            &format!("Service started, mode {}", mode_text(self.mode)),
        );
        // 首拍立即执行状态机（旧 runtime 的 wake_at=now）。
        self.watchdog_deadline = Some(now);
        self.link_poll_at = now;
        if self.brain.is_some() {
            // 旧 manager 启动序列：清残留路由 → 首拍采样。
            effects.push(Effect::CleanupStaleRoutes);
            effects.push(Effect::SampleWireless);
            self.wireless_sample_in_flight = true;
            self.wireless_tick_at = now + WIRELESS_TICK_MS;
        }
    }

    fn handle_wake(&mut self, now: u64, effects: &mut Vec<Effect>) {
        // (a) 到点的 watchdog deadline：先做一笔新鲜链路采样再步进（I5），
        // 拔线期间作废（待 false→true 边沿）。
        if self.watchdog_deadline.is_some_and(|d| d <= now) {
            self.watchdog_deadline = None;
            self.step_pending = self.eth_link != Some(false);
        }
        if self.step_pending || self.link_poll_at <= now {
            if !self.sample_in_flight {
                effects.push(Effect::SampleLink);
                self.sample_in_flight = true;
                self.link_poll_at = now + self.link_poll_interval_ms();
            } else if self.link_poll_at <= now {
                // 结果缺席：只推账本，绝不重发在飞效果（I11）。
                self.link_poll_at = now + SAMPLE_RETRY_MS;
            }
        }
        // (b) 无线 2s 绝对节拍（在飞时同样只推账本）。
        if self.brain.is_some() && self.wireless_tick_at <= now {
            if !self.wireless_sample_in_flight {
                effects.push(Effect::SampleWireless);
                self.wireless_sample_in_flight = true;
            }
            self.wireless_tick_at = now + WIRELESS_TICK_MS;
        }
    }

    fn handle_command(&mut self, cmd: Command, wall: u64, effects: &mut Vec<Effect>) {
        match cmd {
            Command::Redial => {
                log::info!("IPC command: manual redial");
                effects.push(Effect::RequestRedial);
                effects.push(Effect::StepWatchdog);
                self.watchdog_deadline = None;
            }
            Command::SetMode { mode } => {
                log::info!("IPC command: set mode {}", mode_text(mode));
                self.mode = mode;
                self.cfg.wireless.mode = mode;
                if let Some(brain) = self.brain.as_mut() {
                    brain.set_mode(mode);
                }
                effects.push(Effect::PersistMode(mode));
                self.events
                    .push(wall, &format!("Mode switched to {}", mode_text(mode)));
            }
        }
    }

    fn handle_link_sampled(
        &mut self,
        up: Option<bool>,
        now: u64,
        wall: u64,
        effects: &mut Vec<Effect>,
    ) {
        if !self.sample_in_flight {
            return; // I9：重复/迟到结果 no-op
        }
        self.sample_in_flight = false;
        let old = self.eth_link;
        // I5：已知链路态只接受 Some(_) 覆盖；首次采样（含 None）例外。
        let next = if self.link_sample_seen {
            up.or(old)
        } else {
            up
        };
        self.link_sample_seen = true;
        let changed = next != old;
        if changed {
            self.eth_link = next;
            log::info!("Ethernet link state: {:?}", next);
            if old.is_none() && next == Some(false) {
                log::info!("Ethernet link down at startup, dial paused");
            }
            effects.push(Effect::SetWatchdogLink(next));
            if next == Some(false) {
                // 拔线：停步进（I5），等链路恢复边沿；5s 轮询。
                self.watchdog_deadline = None;
                self.step_pending = false;
                self.link_poll_at = now + duration_ms(LINK_DOWN_RETRY);
            } else if old == Some(false) {
                // 插线即拨（I3(c)）。
                log::info!("Ethernet link restored, redialing immediately");
                self.events.push(wall, "Ethernet link restored, redialing");
                effects.push(Effect::RequestRedial);
                effects.push(Effect::StepWatchdog);
                self.watchdog_deadline = None;
                self.step_pending = false;
                self.link_poll_at = now + LINK_POLL_MS;
            } else {
                self.link_poll_at = now + LINK_POLL_MS;
            }
        } else {
            self.link_poll_at = now + self.link_poll_interval_ms();
        }

        // 若这笔采样服务于到点的 watchdog deadline：链路可用则步进（I5）。
        if self.step_pending {
            if self.eth_link == Some(false) {
                self.step_pending = false;
                self.watchdog_deadline = None;
                self.link_poll_at = now + duration_ms(LINK_DOWN_RETRY);
            } else {
                effects.push(Effect::StepWatchdog);
                self.step_pending = false;
            }
        }
    }

    fn handle_watchdog_stepped(&mut self, step: WatchdogStep, now: u64, effects: &mut Vec<Effect>) {
        self.session = step.session;
        self.ppp_ip = step.ppp_ip;
        self.step_pending = false;
        self.watchdog_deadline = Some(now.saturating_add(duration_ms(step.delay)));

        match self.session.status {
            SessionStatus::Backoff | SessionStatus::AuthFail => {
                let since = *self.failing_since_ms.get_or_insert(now);
                // 拔线暂停不是失败：此刻 WLAN 可能正在承载，弹凭据提示误导。
                let link_paused = self.eth_link == Some(false)
                    || self.session.last_drop_reason.as_deref() == Some("Ethernet link down");
                if now.saturating_sub(since) >= REDIAL_FAILING_TOAST_AFTER_MS && !link_paused {
                    let body = format!(
                        "Redial failed for {} minutes ({} attempts), check network or credentials",
                        now.saturating_sub(since) / 60_000,
                        self.session.redial_attempts
                    );
                    self.maybe_notify(
                        ToastKey::RedialFailing,
                        "gdut-net network error",
                        body,
                        now,
                        effects,
                    );
                }
            }
            SessionStatus::Connected => {
                if self.failing_since_ms.take().is_some() {
                    log::info!("Redial succeeded, session restored");
                }
            }
            SessionStatus::Idle | SessionStatus::Dialing => {}
        }
    }

    fn handle_wireless_sampled(
        &mut self,
        sample: WirelessSample,
        now: u64,
        wall: u64,
        effects: &mut Vec<Effect>,
    ) {
        if !self.wireless_sample_in_flight {
            return; // I9：重复/迟到结果 no-op
        }
        self.wireless_sample_in_flight = false;
        if self.brain.is_none() {
            return;
        }
        let WirelessSample { associated, wlan } = sample;
        self.wlan = wlan;
        self.wireless_ip = wlan.map(|a| a.ipv4.to_string());

        let now_secs = now / 1000;
        let world = World {
            now: now_secs,
            eth_link_up: self.eth_link.unwrap_or(false),
            wired_connected: self.session.status == SessionStatus::Connected,
            wlan_associated: associated,
            wlan_ip: wlan.is_some(),
            probe: self.verdict,
        };
        let action = {
            let brain = self.brain.as_mut().expect("checked above");
            brain.set_mode(self.mode);
            brain.decide(&world)
        };
        if self.handle_action(action, now_secs, wall, effects) {
            self.update_metric(effects);
            // Joining 60s 超时 → 复位重关联（IP/DHCP 迟迟不来的自愈）。
            let joining = self
                .brain
                .as_ref()
                .is_some_and(|b| b.phase() == WPhase::Joining);
            if joining
                && self
                    .join_since_ms
                    .is_some_and(|t| now.saturating_sub(t) > JOIN_TIMEOUT_SECS * 1000)
            {
                self.events.push(wall, "Wireless: join timeout, restarting");
                if let Some(brain) = self.brain.as_mut() {
                    brain.restart();
                }
                self.join_since_ms = None;
            }
        }
    }

    /// 处理 Brain 决策；返回 false = 旧 manager 的 `continue`（跳过 metric/超时）。
    fn handle_action(
        &mut self,
        action: Action,
        now_secs: u64,
        wall: u64,
        effects: &mut Vec<Effect>,
    ) -> bool {
        match action {
            Action::None => true,
            Action::Associate => {
                if self.associate_in_flight {
                    return true;
                }
                self.events
                    .push(wall, "Wireless: associating to campus SSID");
                effects.push(Effect::Associate(self.cfg.wireless.profile.clone()));
                self.associate_in_flight = true;
                true
            }
            Action::Disassociate => {
                self.events
                    .push(wall, "Wireless: releasing (wired healthy)");
                effects.push(Effect::TeardownRoutes);
                effects.push(Effect::Disassociate);
                self.verdict = None;
                self.join_since_ms = None;
                true
            }
            Action::PortalAuth => {
                if self.auth_in_flight {
                    return true;
                }
                let Some(wlan) = self.wlan else {
                    if let Some(brain) = self.brain.as_mut() {
                        brain.on_auth(false, "wlan ip lost", now_secs);
                    }
                    self.verdict = None;
                    return false;
                };
                let Some(gateway) = wlan.gateway else {
                    if let Some(brain) = self.brain.as_mut() {
                        brain.on_auth(false, "wlan gateway missing", now_secs);
                    }
                    self.verdict = None;
                    return false;
                };
                let (Some(portal_ip), Some(probe_ip)) = (self.portal_ip, self.probe_ip) else {
                    return true;
                };
                // I6：EnsureRoutes 只在 PortalAuth 且 wlan IP+网关齐备时、Settle 之前。
                effects.push(Effect::EnsureRoutes {
                    dests: vec![portal_ip, probe_ip],
                    gateway,
                    ifindex: wlan.ifindex,
                });
                effects.push(Effect::Settle(ROUTE_SETTLE));
                effects.push(Effect::PortalAuth {
                    src_ip: wlan.ipv4,
                    timeout: PORTAL_AUTH_TIMEOUT,
                });
                self.auth_in_flight = true;
                true
            }
            Action::ProbeNow => {
                if self.probe_in_flight {
                    return true;
                }
                // I6：verdict 只由 WlanProbeFinished 设置。Online 相里 wlan_ip 必然齐备
                // （缺 IP 会先走 start_join），此分支仅为防御，不写 verdict。
                let Some(wlan) = self.wlan else {
                    return false;
                };
                let url = format!("http://{}/", self.cfg.wireless.probe_host);
                effects.push(Effect::WlanProbe {
                    src_ip: wlan.ipv4,
                    gateway: wlan.gateway,
                    url,
                });
                self.probe_in_flight = true;
                true
            }
        }
    }

    /// metric 压制账本：standby 或 exclusive 且有线不健康时压制；有线健康时释放。
    fn update_metric(&mut self, effects: &mut Vec<Effect>) {
        let Some(wlan) = self.wlan else {
            return;
        };
        let wired_healthy =
            self.eth_link == Some(true) && self.session.status == SessionStatus::Connected;
        let takeover_active = self.mode == NetMode::WiredExclusive && !wired_healthy;
        let target = self.cfg.wireless.standby_metric;
        if (self.mode == NetMode::WiredPlusStandby || takeover_active)
            && target != 0
            && self.metric_suppressed_ifindex != Some(wlan.ifindex)
        {
            effects.push(Effect::SuppressMetric {
                ifindex: wlan.ifindex,
                target,
            });
            self.metric_suppressed_ifindex = Some(wlan.ifindex);
        }
        if self.mode == NetMode::WiredExclusive
            && wired_healthy
            && self.metric_suppressed_ifindex.take().is_some()
        {
            effects.push(Effect::ReleaseMetric);
        }
    }

    fn handle_associate_finished(&mut self, res: Result<(), String>, now: u64) {
        if !self.associate_in_flight {
            return; // I9
        }
        self.associate_in_flight = false;
        if let Err(e) = &res {
            log::warn!("Wireless associate failed (will retry): {e}");
        }
        // 旧 manager：关联调用返回后开始计 join 超时（成功/失败都计）。
        self.join_since_ms = Some(now);
    }

    fn handle_portal_finished(&mut self, attempt: PortalAttempt, now: u64, wall: u64) {
        if !self.auth_in_flight {
            return; // I9
        }
        self.auth_in_flight = false;
        let (ok, msg) = portal_outcome(&attempt);
        if ok {
            self.events.push(wall, "Wireless: portal login success");
        } else {
            self.events
                .push(wall, &format!("Wireless: portal login failed: {msg}"));
        }
        if let Some(brain) = self.brain.as_mut() {
            brain.on_auth(ok, &msg, now / 1000);
        }
        // I6：认证尝试即让缓存探测结论失效（无限重认证 Critical 回归点）。
        self.verdict = None;
    }

    fn handle_probe_finished(&mut self, verdict: ProbeVerdict, wall: u64) {
        if !self.probe_in_flight {
            return; // I9
        }
        self.probe_in_flight = false;
        self.verdict = Some(verdict);
        if verdict != ProbeVerdict::Alive {
            self.events
                .push(wall, &format!("Wireless probe: {verdict:?}"));
        }
    }

    fn handle_heartbeat(&mut self, status: HeartbeatStatus, now: u64, effects: &mut Vec<Effect>) {
        if let HeartbeatStatus::Error(e) = &status {
            self.maybe_notify(
                ToastKey::HeartbeatError,
                "gdut-net heartbeat error",
                format!("Compatibility heartbeat error: {e}"),
                now,
                effects,
            );
        }
        self.heartbeat = status;
    }

    fn handle_notify_result(&mut self, key: ToastKey, delivered: bool, now: u64) {
        let Some(pos) = self.notify_in_flight.iter().position(|k| *k == key) else {
            return; // 迟到的投递结果：no-op
        };
        self.notify_in_flight.remove(pos);
        if delivered {
            // 窗口只在成功投递后开启（失败不占用节流窗口）。
            self.notify_delivered_at.retain(|(k, _)| *k != key);
            self.notify_delivered_at.push((key, now));
        }
    }

    /// 通知节流：per-key 30 分钟窗口；在飞不重发；窗口只在投递成功后开启（I8）。
    fn maybe_notify(
        &mut self,
        key: ToastKey,
        title: &str,
        body: String,
        now: u64,
        effects: &mut Vec<Effect>,
    ) {
        if self.notify_in_flight.contains(&key) {
            return;
        }
        if let Some(&(_, at)) = self.notify_delivered_at.iter().find(|(k, _)| *k == key) {
            if now.saturating_sub(at) < NOTIFY_THROTTLE_MS {
                return;
            }
        }
        log::info!("Toast [{}]: {title} — {body}", key.as_str());
        self.notify_in_flight.push(key);
        effects.push(Effect::Notify {
            key,
            title: title.to_string(),
            body,
        });
    }

    fn handle_wireless_died(&mut self, wall: u64) {
        if !self.cfg.wireless.enabled {
            return;
        }
        if !self.wireless_stopped_reported {
            self.events.push(wall, "Wireless: manager stopped");
            self.wireless_stopped_reported = true;
        }
        self.brain = None;
        self.wireless_died = true;
        self.wlan = None;
        self.wireless_ip = None;
        self.verdict = None;
        self.wireless_sample_in_flight = false;
        self.associate_in_flight = false;
        self.auth_in_flight = false;
        self.probe_in_flight = false;
        self.join_since_ms = None;
        // worker 退出时 RouteGuard::drop 已回滚；核心只清账本。
        self.metric_suppressed_ifindex = None;
    }

    fn handle_stop(&mut self, wall: u64, effects: &mut Vec<Effect>) {
        self.stopped = true;
        // 自包含铁律：Stop 反应的效果必须可执行（Hangup 是 Main 不可取消）。
        effects.push(Effect::Hangup);
        if self.cfg.wireless.enabled && !self.wireless_died && self.brain.is_some() {
            effects.push(Effect::TeardownRoutes);
            effects.push(Effect::Disassociate);
            if !self.wireless_stopped_reported {
                self.events.push(wall, "Wireless: manager stopped");
                self.wireless_stopped_reported = true;
            }
        }
    }

    fn compose(&self) -> StateSnapshot {
        let wireless = match &self.brain {
            Some(b) => {
                let mut w = b.snapshot();
                w.ip = self.wireless_ip.clone();
                w
            }
            None => WirelessSnapshot::default(),
        };
        StateSnapshot {
            status: self.session.status,
            since_unix: self.session.since_unix,
            ip: self.ppp_ip.clone(),
            last_drop_reason: self.session.last_drop_reason.clone(),
            redial_attempts: self.session.redial_attempts,
            heartbeat: self.heartbeat.clone(),
            mode: self.mode,
            wireless,
            events: self.events.ring().clone(),
        }
    }
}

/// 模式英文短名（事件环与日志用；字符串与旧 runtime 逐字一致）。
fn mode_text(m: NetMode) -> &'static str {
    match m {
        NetMode::WiredExclusive => "wired-exclusive",
        NetMode::WiredPlusStandby => "wired-plus-standby",
    }
}

/// `PortalAttempt` → (ok, msg)：与旧 manager 的判定逐字一致。
fn portal_outcome(attempt: &PortalAttempt) -> (bool, String) {
    match attempt {
        PortalAttempt::Replied { status, body } => match portal::parse_portal_reply(body) {
            portal::PortalResult::Success => (true, "login ok".to_string()),
            portal::PortalResult::AlreadyOnline => (true, "already online".to_string()),
            portal::PortalResult::Failure(m) => (false, m),
            portal::PortalResult::Malformed => {
                (false, format!("unparseable reply (HTTP {status})"))
            }
        },
        PortalAttempt::NoReply => (false, "no reply".to_string()),
        PortalAttempt::TimedOut => (false, "portal auth timeout".to_string()),
        PortalAttempt::Failed(m) => (false, m.clone()),
    }
}

/// `Duration` → 毫秒（饱和）。
fn duration_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}
