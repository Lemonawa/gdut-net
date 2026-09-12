//! Task 8 harness：真 `Watchdog` + 脚本化世界 + `FakeClock` 驱动 `Supervisor`。
//!
//! 时间线以毫秒推进（测试总时长 < 1s 真实时间）；Main 车道效果由 harness 内联执行并
//! 把结果事件回灌（等价 Task 9 壳的同步执行），Wireless 车道效果记录待测。

use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gdut_net::config::Config;
use gdut_net::ipc::protocol::{
    Command, HeartbeatStatus, NetMode, SessionStatus, WPhase, WirelessSnapshot,
};
use gdut_net::probe::ProbeVerdict;
use gdut_net::ras::ErrKind;
use gdut_net::supervisor::{
    Clock, Effect, Event, Lane, PortalAttempt, Reaction, Supervisor, ToastKey, WatchdogStep,
    WirelessSample, WlanSample,
};
use gdut_net::watchdog::{DialError, Dialer, Prober, SessionView, Watchdog, WatchdogCfg};

// ---------------------------------------------------------------------------
// FakeClock
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct FakeClock {
    ms: Arc<AtomicU64>,
    wall: Arc<AtomicU64>,
}

impl FakeClock {
    fn new() -> Self {
        Self {
            ms: Arc::new(AtomicU64::new(0)),
            wall: Arc::new(AtomicU64::new(1_700_000_000)),
        }
    }

    fn now(&self) -> u64 {
        self.ms.load(Ordering::SeqCst)
    }

    fn advance(&self, ms: u64) {
        self.ms.fetch_add(ms, Ordering::SeqCst);
        self.wall.fetch_add(ms / 1000, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.ms.load(Ordering::SeqCst)
    }

    fn wall_secs(&self) -> u64 {
        self.wall.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// Mock 网络世界
// ---------------------------------------------------------------------------

struct MockDialer {
    calls: Arc<AtomicU32>,
    fail: Arc<AtomicBool>,
    connected: bool,
}

#[async_trait::async_trait]
impl Dialer for MockDialer {
    async fn dial(&mut self) -> Result<(), DialError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err(DialError {
                kind: ErrKind::Transient,
                code: 678,
                msg: "mock dial failure".into(),
            });
        }
        self.connected = true;
        Ok(())
    }

    async fn hangup(&mut self) {
        self.connected = false;
    }

    fn is_connected(&mut self) -> bool {
        self.connected
    }
}

struct MockProber(Vec<ProbeVerdict>);

#[async_trait::async_trait]
impl Prober for MockProber {
    async fn probe(&mut self) -> ProbeVerdict {
        if self.0.len() > 1 {
            self.0.remove(0)
        } else {
            self.0[0]
        }
    }
}

#[derive(Clone)]
struct World {
    link: Option<bool>,
    wireless: WirelessSample,
    ppp_ip: Option<String>,
}

impl Default for World {
    fn default() -> Self {
        Self {
            link: Some(true),
            wireless: WirelessSample::default(),
            ppp_ip: Some("10.30.0.2".to_string()),
        }
    }
}

fn wlan_sample(ip: &str, gateway: Option<&str>, ifindex: u32) -> WirelessSample {
    WirelessSample {
        associated: true,
        wlan: Some(WlanSample {
            ipv4: ip.parse().unwrap(),
            gateway: gateway.map(|g| g.parse().unwrap()),
            ifindex,
        }),
    }
}

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

fn wired_cfg() -> Config {
    let mut cfg = Config::default();
    cfg.account.student_id = "student".into();
    cfg.wireless.enabled = false;
    cfg
}

fn wireless_cfg(mode: NetMode) -> Config {
    let mut cfg = Config::default();
    cfg.account.student_id = "student".into();
    cfg.wireless.enabled = true;
    cfg.wireless.mode = mode;
    cfg
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Harness {
    sup: Supervisor,
    clock: FakeClock,
    wd: Watchdog,
    world: World,
    effects: Vec<Effect>,
    wireless_out: Vec<Effect>,
    notifies: Vec<(ToastKey, String, String)>,
    persisted: Vec<NetMode>,
    wake_at: Option<u64>,
    dial_calls: Arc<AtomicU32>,
    auto_sample_wireless: bool,
}

impl Harness {
    fn new(cfg: Config, failing: bool) -> Self {
        Self::with_probe(cfg, failing, vec![ProbeVerdict::Alive])
    }

    fn with_probe(cfg: Config, failing: bool, verdicts: Vec<ProbeVerdict>) -> Self {
        let clock = FakeClock::new();
        let dial_calls = Arc::new(AtomicU32::new(0));
        let dialer = MockDialer {
            calls: dial_calls.clone(),
            fail: Arc::new(AtomicBool::new(failing)),
            connected: false,
        };
        let wd = Watchdog::new(
            dialer,
            MockProber(verdicts),
            WatchdogCfg {
                redial_min: Duration::from_secs(1),
                redial_max: Duration::from_secs(300),
                probe_interval: Duration::from_secs(cfg.dial.probe_interval_secs),
                auth_fail_delay: Duration::from_secs(600),
            },
        );
        let sup = Supervisor::new(cfg, clock.clone());
        Self {
            sup,
            clock,
            wd,
            world: World::default(),
            effects: Vec::new(),
            wireless_out: Vec::new(),
            notifies: Vec::new(),
            persisted: Vec::new(),
            wake_at: None,
            dial_calls,
            auto_sample_wireless: true,
        }
    }

    /// 喂一个事件并递归处理 Main 车道效果（结果事件 FIFO 回灌）。
    async fn push(&mut self, event: Event) -> Vec<Reaction> {
        let mut queue = VecDeque::from([event]);
        let mut reactions = Vec::new();
        while let Some(ev) = queue.pop_front() {
            let r = self.sup.on(ev);
            self.wake_at = r.wake_at;
            for effect in r.effects.clone() {
                self.effects.push(effect.clone());
                match effect.lane() {
                    Lane::Main => self.exec_main(&effect, &mut queue).await,
                    Lane::Wireless => self.wireless_out.push(effect),
                }
            }
            reactions.push(r);
        }
        reactions
    }

    async fn exec_main(&mut self, effect: &Effect, queue: &mut VecDeque<Event>) {
        match effect {
            Effect::SampleLink => queue.push_back(Event::LinkSampled {
                up: self.world.link,
                ppp_ip: self.world.ppp_ip.clone(),
            }),
            Effect::SampleWireless => {
                if self.auto_sample_wireless {
                    queue.push_back(Event::WirelessSampled(self.world.wireless));
                } else {
                    // 测试显式回灌采样结果（I11 缺失结果场景）。
                    self.wireless_out.push(Effect::SampleWireless);
                }
            }
            Effect::SetWatchdogLink(up) => self.wd.set_eth_link(*up),
            Effect::RequestRedial => self.wd.request_redial(),
            Effect::StepWatchdog => {
                let delay = self.wd.run_once().await;
                queue.push_back(Event::WatchdogStepped(WatchdogStep {
                    delay,
                    session: self.wd.view(),
                    ppp_ip: self.world.ppp_ip.clone(),
                }));
            }
            Effect::Hangup => self.wd.shutdown().await,
            Effect::PersistMode(mode) => self.persisted.push(*mode),
            Effect::Notify { key, title, body } => {
                self.notifies.push((*key, title.clone(), body.clone()));
            }
            other => unreachable!("wireless effect on Main lane: {other:?}"),
        }
    }

    fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    fn take_wireless(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.wireless_out)
    }

    fn take_notifies(&mut self) -> Vec<(ToastKey, String, String)> {
        std::mem::take(&mut self.notifies)
    }

    fn ring(&self) -> Vec<String> {
        self.sup.snapshot().events.iter().cloned().collect()
    }

    fn ring_has(&self, needle: &str) -> bool {
        self.ring().iter().any(|line| line.contains(needle))
    }

    /// 推进 FakeClock 到最近一次 `wake_at` 并触发 `Wake`。
    async fn advance_to_wake(&mut self) -> Vec<Reaction> {
        let target = self.wake_at.expect("harness: no scheduled wake");
        let now = self.clock.now();
        if target > now {
            self.clock.advance(target - now);
        }
        self.push(Event::Wake).await
    }

    /// 启动并完成首个有线拍（拨号成功 → Connected）。
    async fn start_and_connect(&mut self) {
        self.push(Event::Started).await;
        self.advance_to_wake().await;
    }

    /// 按绝对账本连续跑到 `target` 毫秒（每到点即 Wake）；防止不前进时死循环。
    async fn run_until(&mut self, target: u64) {
        for _ in 0..10_000 {
            match self.wake_at {
                Some(next) if next <= target => {
                    self.advance_to_wake().await;
                }
                _ => return,
            }
        }
        panic!("run_until made no progress");
    }
}

/// 过滤 metric 效果，便于断言核心动作序列。
fn without_metric(effects: Vec<Effect>) -> Vec<Effect> {
    effects
        .into_iter()
        .filter(|e| !matches!(e, Effect::SuppressMetric { .. } | Effect::ReleaseMetric))
        .collect()
}

/// 仅取 metric 效果（顺序保留）。
fn metric_effects(effects: &[Effect]) -> Vec<Effect> {
    effects
        .iter()
        .filter(|e| matches!(e, Effect::SuppressMetric { .. } | Effect::ReleaseMetric))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Step 2 场景：有线调度最早三例
// ---------------------------------------------------------------------------

#[tokio::test]
async fn started_then_first_wake_dials() {
    let mut h = Harness::new(wired_cfg(), false);
    h.push(Event::Started).await;
    assert_eq!(h.take_effects(), vec![], "Started only arms the first beat");
    assert_eq!(
        h.wake_at,
        Some(h.clock.now()),
        "Started schedules the first beat immediately"
    );

    let reactions = h.push(Event::Wake).await;
    assert_eq!(
        h.take_effects(),
        vec![
            Effect::SampleLink,
            Effect::SetWatchdogLink(Some(true)),
            Effect::StepWatchdog,
        ],
        "I3/I5: fresh link sample, then gate, then step"
    );
    assert_eq!(h.dial_calls.load(Ordering::SeqCst), 1);
    let last = reactions.last().expect("reactions");
    assert!(
        matches!(last.snapshot.as_ref(), Some(s) if s.status == SessionStatus::Connected),
        "step result must publish the wired snapshot"
    );
}

#[tokio::test]
async fn cable_out_never_dials() {
    // 拨号失败让 watchdog 停在拨号路径（do_dial）：拔线安全闸正是 `do_dial`
    // 内部的 `eth_link == Some(false)` 分支（置 Backoff/Ethernet link down + 5s）。
    let mut h = Harness::new(wired_cfg(), true);
    h.push(Event::Started).await;
    h.advance_to_wake().await; // t=0：拨号失败 → Backoff，delay=1s
    assert_eq!(h.dial_calls.load(Ordering::SeqCst), 1);
    h.take_effects();

    // 到点前拔线：步进照常执行，先以新鲜采样喂 watchdog 门控（R1）。
    h.world.link = Some(false);
    h.advance_to_wake().await; // t=1s：deadline 到点
    assert_eq!(
        h.take_effects(),
        vec![
            Effect::SampleLink,
            Effect::SetWatchdogLink(Some(false)),
            Effect::StepWatchdog,
        ],
        "R1: cable-out does not exempt the scheduled step"
    );
    assert_eq!(
        h.dial_calls.load(Ordering::SeqCst),
        1,
        "R1: gated step never dials"
    );
    let snap = h.sup.snapshot();
    assert_eq!(snap.status, SessionStatus::Backoff);
    assert_eq!(
        snap.last_drop_reason.as_deref(),
        Some("Ethernet link down"),
        "session state honestly reflects the gated step"
    );

    // R2：拔线轮询保持 2s（`wake_at=+2s`，纯轮询拍只有 SampleLink）；
    // gated 步进自排 5s（LINK_DOWN_RETRY）——t=1s 的步进 → t=6s 下一步进。
    assert_eq!(h.wake_at, Some(h.clock.now() + 2_000));
    h.advance_to_wake().await; // t=3s：纯轮询
    assert_eq!(h.take_effects(), vec![Effect::SampleLink]);
    h.advance_to_wake().await; // t=5s：纯轮询
    assert_eq!(h.take_effects(), vec![Effect::SampleLink]);
    h.advance_to_wake().await; // t=6s：gated 步进到点（1s + 5s）
    assert_eq!(h.clock.now(), 6_000);
    assert_eq!(
        h.take_effects(),
        vec![Effect::SampleLink, Effect::StepWatchdog]
    );
    assert_eq!(h.dial_calls.load(Ordering::SeqCst), 1);
    assert_eq!(h.sup.snapshot().status, SessionStatus::Backoff);
    assert_eq!(h.wake_at, Some(h.clock.now() + 2_000));
}

#[tokio::test]
async fn cable_out_connected_session_steps_to_gated_backoff() {
    // 会话在线时拔线（concern 1 回归）：到点步进走探测路径，双探测失败后
    // hangup → do_dial 门控 → Backoff/Ethernet link down；全程不新增拨号。
    let mut h = Harness::with_probe(
        wired_cfg(),
        false,
        vec![ProbeVerdict::LinkDown, ProbeVerdict::LinkDown],
    );
    h.start_and_connect().await; // t=0：Connected，deadline=30s
    assert_eq!(h.dial_calls.load(Ordering::SeqCst), 1);
    assert_eq!(h.sup.snapshot().status, SessionStatus::Connected);
    h.take_effects();

    h.world.link = Some(false);
    h.run_until(30_000).await; // 拔线后到点：第一次探测异常（复核 7.5s）
    assert!(h
        .take_effects()
        .iter()
        .any(|e| matches!(e, Effect::StepWatchdog)));
    assert_eq!(
        h.sup.snapshot().status,
        SessionStatus::Connected,
        "single anomaly only rechecks (watchdog semantics)"
    );
    assert_eq!(h.dial_calls.load(Ordering::SeqCst), 1);

    // 复核窗口内的 2s 轮询拍（32s/34s/36s）不步进。
    for _ in 0..3 {
        h.advance_to_wake().await;
        assert!(
            !h.take_effects()
                .iter()
                .any(|e| matches!(e, Effect::StepWatchdog)),
            "no step before the recheck deadline"
        );
    }
    h.advance_to_wake().await; // 37.5s：第二次探测失败 → gated drop
    assert_eq!(h.clock.now(), 37_500);
    assert!(h
        .take_effects()
        .iter()
        .any(|e| matches!(e, Effect::StepWatchdog)));
    assert_eq!(
        h.dial_calls.load(Ordering::SeqCst),
        1,
        "R1: gated do_dial never touches the port"
    );
    let snap = h.sup.snapshot();
    assert_eq!(snap.status, SessionStatus::Backoff);
    assert_eq!(
        snap.last_drop_reason.as_deref(),
        Some("Ethernet link down"),
        "wired state transitions instead of staying Connected"
    );
}

#[tokio::test]
async fn link_restore_redials_immediately() {
    let mut h = Harness::new(wired_cfg(), false);
    h.start_and_connect().await;
    h.take_effects();

    // 拔线一拍。
    h.clock.advance(2_000);
    h.world.link = Some(false);
    h.push(Event::Wake).await;
    h.take_effects();

    // 插线：false→true 边沿在同一反应链里 RequestRedial + StepWatchdog。
    h.world.link = Some(true);
    h.advance_to_wake().await;
    assert_eq!(
        h.take_effects(),
        vec![
            Effect::SampleLink,
            Effect::SetWatchdogLink(Some(true)),
            Effect::RequestRedial,
            Effect::StepWatchdog,
        ],
        "I3/I5: link restore is a legitimate StepWatchdog trigger"
    );
    assert_eq!(h.dial_calls.load(Ordering::SeqCst), 2);
    assert!(h.ring_has("Ethernet link restored, redialing"));
}

// ---------------------------------------------------------------------------
// Step 3-6 场景：命令 / 快照 / 通知 / 停止 / 护栏
// ---------------------------------------------------------------------------

#[tokio::test]
async fn set_mode_persists_rings_and_broadcasts() {
    let mut h = Harness::new(wired_cfg(), false);
    h.start_and_connect().await;
    h.take_effects();

    let reactions = h
        .push(Event::Command(Command::SetMode {
            mode: NetMode::WiredPlusStandby,
        }))
        .await;
    assert_eq!(
        h.take_effects(),
        vec![Effect::PersistMode(NetMode::WiredPlusStandby)]
    );
    assert_eq!(h.persisted, vec![NetMode::WiredPlusStandby]);
    assert!(h.ring_has("Mode switched to wired-plus-standby"));
    assert_eq!(h.sup.snapshot().mode, NetMode::WiredPlusStandby);
    assert!(
        reactions.last().unwrap().snapshot.is_some(),
        "mode + event ring change must publish"
    );
}

#[tokio::test]
async fn snapshot_publishes_only_on_change() {
    let mut h = Harness::new(wired_cfg(), false);
    h.push(Event::Started).await;
    h.take_effects();

    // 心跳值未变：无快照（I2 变化门控）。
    let reactions = h.push(Event::Heartbeat(HeartbeatStatus::Off)).await;
    assert!(reactions.last().unwrap().snapshot.is_none());

    // 心跳变化：发布，且与 snapshot() 同值（同一组装函数）。
    let reactions = h.push(Event::Heartbeat(HeartbeatStatus::Running)).await;
    let published = reactions
        .last()
        .unwrap()
        .snapshot
        .clone()
        .expect("heartbeat transition publishes");
    assert_eq!(published, h.sup.snapshot());
    assert_eq!(published.heartbeat, HeartbeatStatus::Running);

    // to/from Error 也必须产生快照（选型收窄③）。
    let reactions = h
        .push(Event::Heartbeat(HeartbeatStatus::Error("x".into())))
        .await;
    assert!(reactions.last().unwrap().snapshot.is_some());
    let reactions = h.push(Event::Heartbeat(HeartbeatStatus::Off)).await;
    assert!(reactions.last().unwrap().snapshot.is_some());
}

#[tokio::test]
async fn notify_throttle_only_on_delivered() {
    let mut h = Harness::new(wired_cfg(), false);
    h.push(Event::Started).await;

    h.push(Event::Heartbeat(HeartbeatStatus::Error("e1".into())))
        .await;
    let notifies = h.take_notifies();
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].0, ToastKey::HeartbeatError);
    assert_eq!(notifies[0].1, "gdut-net heartbeat error");
    assert!(notifies[0].2.contains("e1"));

    // 在飞：不重发。
    h.push(Event::Heartbeat(HeartbeatStatus::Error("e2".into())))
        .await;
    assert!(h.take_notifies().is_empty());

    // 投递失败：窗口不开启，下次仍发。
    h.push(Event::NotifyResult {
        key: ToastKey::HeartbeatError,
        delivered: false,
    })
    .await;
    h.push(Event::Heartbeat(HeartbeatStatus::Error("e3".into())))
        .await;
    assert_eq!(
        h.take_notifies().len(),
        1,
        "failed delivery must not open the 30min window"
    );

    // 投递成功：30 分钟窗口开启。
    h.push(Event::NotifyResult {
        key: ToastKey::HeartbeatError,
        delivered: true,
    })
    .await;
    h.clock.advance(29 * 60 * 1000);
    h.push(Event::Heartbeat(HeartbeatStatus::Error("e4".into())))
        .await;
    assert!(h.take_notifies().is_empty(), "throttled inside 30min");

    h.clock.advance(2 * 60 * 1000);
    h.push(Event::Heartbeat(HeartbeatStatus::Error("e5".into())))
        .await;
    assert_eq!(h.take_notifies().len(), 1, "window expired after 30min");
}

#[tokio::test]
async fn redial_failing_toast_and_link_down_exemption() {
    let mut h = Harness::new(wired_cfg(), true);
    h.push(Event::Started).await;

    // 拨号连续失败：第一次到点的 WatchdogStepped 跨过 10 分钟阈值时弹一次。
    h.run_until(900_000).await;
    let notifies = h.take_notifies();
    assert_eq!(notifies.len(), 1, "exactly one RedialFailing toast");
    assert_eq!(notifies[0].0, ToastKey::RedialFailing);
    assert_eq!(notifies[0].1, "gdut-net network error");
    assert!(notifies[0].2.contains("Redial failed for"));
    assert!(notifies[0].2.contains("check network or credentials"));

    h.push(Event::NotifyResult {
        key: ToastKey::RedialFailing,
        delivered: true,
    })
    .await;

    // 拔线暂停豁免：10 分钟阈值虽已越过，但拔线期间不弹（keep 现状）。
    h.world.link = Some(false);
    h.run_until(1_000_000).await;
    h.take_notifies();
    let dials_before = h.dial_calls.load(Ordering::SeqCst);
    h.push(Event::Command(Command::Redial)).await;
    assert_eq!(
        h.dial_calls.load(Ordering::SeqCst),
        dials_before,
        "watchdog link gate: manual redial while cable out never touches the port"
    );
    assert!(
        h.take_notifies().is_empty(),
        "cable-down pause must not toast"
    );
}

#[tokio::test]
async fn late_and_duplicate_results_are_inert() {
    let mut h = Harness::new(wireless_cfg(NetMode::WiredPlusStandby), false);
    h.start_and_connect().await; // Started 的自动无线采样让 brain 进入 Joining
    h.take_effects();
    h.take_wireless();

    // Associate 结果（在飞）无副作用；重复结果与无在飞请求的结果全部 no-op（I9）。
    h.push(Event::AssociateFinished(Ok(()))).await;
    h.push(Event::AssociateFinished(Err("late".into()))).await;
    h.push(Event::LinkSampled {
        up: Some(false),
        ppp_ip: None,
    })
    .await;
    h.push(Event::WirelessSampled(wlan_sample(
        "10.1.1.5",
        Some("10.1.1.1"),
        15,
    )))
    .await;
    h.push(Event::PortalFinished(PortalAttempt::Replied {
        status: 200,
        body: r#"dr1004({"result":"1"})"#.into(),
    }))
    .await;
    h.push(Event::WlanProbeFinished(ProbeVerdict::Kicked)).await;
    assert_eq!(h.take_effects(), vec![]);
    assert_eq!(h.take_wireless(), vec![]);
    assert!(
        !h.ring_has("portal login success") && !h.ring_has("Wireless probe: Kicked"),
        "duplicate results must not touch the event ring"
    );

    // 迟到的 LinkSampled { up: Some(false) } 没有翻转已知链路态。
    h.advance_to_wake().await;
    assert!(
        !h.take_effects()
            .iter()
            .any(|e| matches!(e, Effect::SetWatchdogLink(Some(false)))),
        "late LinkSampled must not flip the known link state"
    );
}

#[tokio::test]
async fn link_sample_refreshes_ppp_ip() {
    // 拨号成功瞬间 PPP 适配器可能尚不可枚举：WatchdogStepped.ppp_ip = None。
    let mut h = Harness::new(wired_cfg(), false);
    h.world.ppp_ip = None;
    h.start_and_connect().await;
    assert_eq!(h.sup.snapshot().status, SessionStatus::Connected);
    assert_eq!(h.sup.snapshot().ip, None);

    // 下一笔链路采样（2s 轮询拍）同拍读到会话 IP：快照立即刷新，
    // 不再等下一个探测周期（~30s；旧 runtime 每次推快照都重读）。
    h.world.ppp_ip = Some("10.30.1.2".to_string());
    let reactions = h.advance_to_wake().await;
    assert_eq!(h.sup.snapshot().ip.as_deref(), Some("10.30.1.2"));
    let published = reactions
        .iter()
        .filter_map(|r| r.snapshot.as_ref())
        .next_back()
        .expect("ppp_ip refresh must publish");
    assert_eq!(published.ip.as_deref(), Some("10.30.1.2"));
}

#[tokio::test]
async fn missing_sample_result_never_double_samples() {
    let mut h = Harness::new(wireless_cfg(NetMode::WiredPlusStandby), false);
    h.auto_sample_wireless = false; // 采样结果由测试显式回灌
    h.push(Event::Started).await;
    let started = h.take_wireless();
    assert_eq!(
        started,
        vec![Effect::CleanupStaleRoutes, Effect::SampleWireless]
    );

    // 采样在飞：连续 Wake 绝不重发 SampleWireless（I11 绝不双发）。
    h.advance_to_wake().await;
    assert!(
        !h.take_wireless()
            .iter()
            .any(|e| matches!(e, Effect::SampleWireless)),
        "no double sample while one is in flight"
    );
    h.advance_to_wake().await;
    assert!(
        !h.take_wireless()
            .iter()
            .any(|e| matches!(e, Effect::SampleWireless)),
        "retry wakes must not double-send"
    );

    // 迟到结果仍被接受并驱动 brain。
    h.push(Event::WirelessSampled(wlan_sample(
        "10.1.1.5",
        Some("10.1.1.1"),
        15,
    )))
    .await;
    assert!(
        h.take_wireless()
            .iter()
            .any(|e| matches!(e, Effect::Associate(_))),
        "late result still drives the brain"
    );

    // 护栏清空后恢复采样。
    h.advance_to_wake().await;
    assert!(
        h.take_wireless()
            .iter()
            .any(|e| matches!(e, Effect::SampleWireless)),
        "tick resumes after the late result"
    );
}

#[tokio::test]
async fn started_and_stop_are_once() {
    let mut h = Harness::new(wired_cfg(), false);
    // Started 之前的事件：全部惰性（I10：Started 最先）。
    h.push(Event::Wake).await;
    h.push(Event::Stop).await;
    assert_eq!(h.take_effects(), vec![]);
    assert_eq!(h.wake_at, None);

    h.push(Event::Started).await;
    h.take_effects();
    // 第二次 Started 幂等。
    h.push(Event::Started).await;
    assert_eq!(h.take_effects(), vec![]);

    let reactions = h.push(Event::Stop).await;
    assert_eq!(h.take_effects(), vec![Effect::Hangup]);
    assert_eq!(
        reactions.last().unwrap().wake_at,
        None,
        "Stop clears the timer"
    );

    // Stop 之后任意事件惰性。
    h.push(Event::Stop).await;
    h.push(Event::Wake).await;
    h.push(Event::Command(Command::Redial)).await;
    h.push(Event::LinkSampled {
        up: Some(true),
        ppp_ip: None,
    })
    .await;
    assert_eq!(h.take_effects(), vec![]);
    assert_eq!(h.wake_at, None);
}

#[tokio::test]
async fn event_storms_never_fragment_backoff() {
    let mut h = Harness::new(wired_cfg(), true);
    h.push(Event::Started).await;
    // 两次失败步进（0s、1s）后取一个绝对 deadline。
    h.run_until(1_000).await;
    let armed = h.wake_at.expect("timer");
    for _ in 0..50 {
        h.push(Event::Heartbeat(HeartbeatStatus::Running)).await;
        h.push(Event::WirelessSampled(WirelessSample::default()))
            .await;
        h.push(Event::Wake).await; // 提前 Wake：只重判定到点，不改 deadline
    }
    assert_eq!(
        h.wake_at,
        Some(armed),
        "event frequency must not move any deadline (I7)"
    );

    // 79 次重拨事故回归：7 分钟内拨号次数 = 指数退避排程，不是事件数。
    h.take_effects();
    let mut steps = 0usize;
    while let Some(next) = h.wake_at {
        if next > 420_000 {
            break;
        }
        let now = h.clock.now();
        if next > now {
            h.clock.advance(next - now);
        }
        for _ in 0..5 {
            h.push(Event::Heartbeat(HeartbeatStatus::Running)).await;
            h.push(Event::WirelessSampled(WirelessSample::default()))
                .await;
        }
        assert!(
            h.take_effects()
                .iter()
                .all(|e| !matches!(e, Effect::StepWatchdog)),
            "heartbeat/wireless events must never trigger StepWatchdog (I3)"
        );
        let reactions = h.push(Event::Wake).await;
        for r in &reactions {
            steps += r
                .effects
                .iter()
                .filter(|e| matches!(e, Effect::StepWatchdog))
                .count();
        }
        h.take_effects();
    }
    assert_eq!(
        steps, 7,
        "steps at 3s,7s,15s,31s,63s,127s,255s after the storm"
    );
    assert_eq!(
        h.dial_calls.load(Ordering::SeqCst),
        9,
        "0s/1s steps + 7 storm-timeline steps; event storms never dial"
    );
}

async fn purity_script() -> Vec<Reaction> {
    let mut h = Harness::new(wired_cfg(), true);
    let mut all = Vec::new();
    all.extend(h.push(Event::Started).await);
    all.extend(h.advance_to_wake().await);
    h.world.link = Some(false);
    h.clock.advance(2_000);
    all.extend(h.push(Event::Wake).await);
    h.world.link = Some(true);
    all.extend(h.advance_to_wake().await);
    all.extend(
        h.push(Event::Heartbeat(HeartbeatStatus::Error("x".into())))
            .await,
    );
    all.extend(h.push(Event::Command(Command::Redial)).await);
    all.extend(h.advance_to_wake().await);
    all.extend(h.push(Event::Stop).await);
    all
}

#[tokio::test]
async fn same_input_sequence_yields_same_reactions() {
    assert_eq!(
        purity_script().await,
        purity_script().await,
        "I1: identical state+events+clock ⇒ identical reactions"
    );
}

#[tokio::test]
async fn arbitrary_event_sequences_do_not_panic() {
    let mut h = Harness::new(wireless_cfg(NetMode::WiredExclusive), false);
    // Started 之前：惰性。
    h.push(Event::Wake).await;
    h.push(Event::Stop).await;
    assert_eq!(h.take_effects(), vec![]);

    h.push(Event::Started).await;
    let junk = vec![
        Event::Wake,
        Event::LinkSampled {
            up: None,
            ppp_ip: None,
        },
        Event::LinkSampled {
            up: Some(false),
            ppp_ip: None,
        },
        Event::Wake,
        Event::Heartbeat(HeartbeatStatus::Off),
        Event::Command(Command::Redial),
        Event::Command(Command::SetMode {
            mode: NetMode::WiredPlusStandby,
        }),
        Event::NotifyResult {
            key: ToastKey::RedialFailing,
            delivered: true,
        },
        Event::WirelessDied,
        Event::Started,
        Event::WatchdogStepped(WatchdogStep {
            delay: Duration::from_secs(1),
            session: SessionView {
                status: SessionStatus::Backoff,
                since_unix: None,
                last_drop_reason: Some("junk".into()),
                redial_attempts: 7,
            },
            ppp_ip: None,
        }),
        Event::Stop,
    ];
    for _ in 0..3 {
        for ev in &junk {
            h.push(ev.clone()).await;
        }
    }
    assert_eq!(h.wake_at, None, "Stop is terminal");
    let _ = h.sup.snapshot();
}

#[tokio::test]
async fn manual_redial_steps_immediately() {
    let mut h = Harness::new(wired_cfg(), false);
    h.start_and_connect().await;
    h.take_effects();
    assert_eq!(h.dial_calls.load(Ordering::SeqCst), 1);

    // I3(b)：Command::Redial 是合法 StepWatchdog 触发点。
    h.push(Event::Command(Command::Redial)).await;
    assert_eq!(
        h.take_effects(),
        vec![Effect::RequestRedial, Effect::StepWatchdog]
    );
    assert_eq!(h.dial_calls.load(Ordering::SeqCst), 2);
}

#[test]
fn every_effect_has_exactly_one_lane() {
    let main = vec![
        Effect::StepWatchdog,
        Effect::RequestRedial,
        Effect::SetWatchdogLink(Some(true)),
        Effect::SetWatchdogLink(None),
        Effect::Hangup,
        Effect::SampleLink,
        Effect::SampleWireless,
        Effect::PersistMode(NetMode::WiredExclusive),
        Effect::Notify {
            key: ToastKey::HeartbeatError,
            title: "t".into(),
            body: "b".into(),
        },
    ];
    for effect in main {
        assert_eq!(effect.lane(), Lane::Main, "{effect:?}");
    }
    let wireless = vec![
        Effect::CleanupStaleRoutes,
        Effect::Associate("p".into()),
        Effect::Disassociate,
        Effect::EnsureRoutes {
            dests: vec![Ipv4Addr::new(10, 0, 3, 2)],
            gateway: Ipv4Addr::new(10, 1, 1, 1),
            ifindex: 15,
        },
        Effect::SuppressMetric {
            ifindex: 15,
            target: 100,
        },
        Effect::ReleaseMetric,
        Effect::TeardownRoutes,
        Effect::Settle(Duration::from_secs(3)),
        Effect::PortalAuth {
            src_ip: Ipv4Addr::new(10, 1, 1, 5),
            timeout: Duration::from_secs(20),
        },
        Effect::WlanProbe {
            src_ip: Ipv4Addr::new(10, 1, 1, 5),
            gateway: None,
            url: "http://223.5.5.5/".into(),
        },
    ];
    for effect in wireless {
        assert_eq!(effect.lane(), Lane::Wireless, "{effect:?}");
    }
}

// ---------------------------------------------------------------------------
// 无线编排场景（I6）
// ---------------------------------------------------------------------------

const PORTAL_SUCCESS: &str = r#"dr1004({"result":"1"})"#;

#[tokio::test]
async fn verdict_cleared_after_auth_then_probe_now() {
    let mut h = Harness::new(wireless_cfg(NetMode::WiredPlusStandby), false);
    h.world.wireless = wlan_sample("10.1.1.5", Some("10.1.1.1"), 15);
    h.push(Event::Started).await; // 自动采样 → Associate
    assert!(without_metric(h.take_wireless()).contains(&Effect::Associate("gdut".into())));
    h.push(Event::AssociateFinished(Ok(()))).await;
    h.advance_to_wake().await; // t=0: 拨号 → Connected
    h.take_wireless();

    // 关联成功 + IP：Joining → Authing → PortalAuth（EnsureRoutes 先于 Settle/PortalAuth）。
    h.run_until(2_000).await;
    let effects = without_metric(h.take_wireless());
    assert!(
        matches!(
            effects.as_slice(),
            [
                Effect::EnsureRoutes { .. },
                Effect::Settle(_),
                Effect::PortalAuth { .. }
            ]
        ),
        "got {effects:?}"
    );

    h.push(Event::PortalFinished(PortalAttempt::Replied {
        status: 200,
        body: PORTAL_SUCCESS.into(),
    }))
    .await;
    assert_eq!(h.sup.snapshot().wireless.phase, WPhase::Online);

    // Online 且无缓存 verdict → 立即 ProbeNow。
    h.run_until(4_000).await;
    let effects = without_metric(h.take_wireless());
    assert!(
        matches!(effects.as_slice(), [Effect::WlanProbe { .. }]),
        "first probe after auth, got {effects:?}"
    );

    // Kicked → 下一次决策是合法重认证。
    h.push(Event::WlanProbeFinished(ProbeVerdict::Kicked)).await;
    assert!(h.ring_has("Wireless probe: Kicked"));
    h.run_until(6_000).await;
    let effects = without_metric(h.take_wireless());
    assert!(
        matches!(
            effects.as_slice(),
            [
                Effect::EnsureRoutes { .. },
                Effect::Settle(_),
                Effect::PortalAuth { .. }
            ]
        ),
        "kicked re-auth, got {effects:?}"
    );

    // 认证后 verdict 必须清空：下一拍回到 ProbeNow，而不是无限重认证（Critical 回归）。
    h.push(Event::PortalFinished(PortalAttempt::Replied {
        status: 200,
        body: PORTAL_SUCCESS.into(),
    }))
    .await;
    h.run_until(8_000).await;
    let effects = without_metric(h.take_wireless());
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::WlanProbe { .. })),
        "probe after the auth attempt, got {effects:?}"
    );
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::PortalAuth { .. })),
        "stale Kicked must not survive the auth attempt: {effects:?}"
    );
    assert_eq!(
        h.ring()
            .iter()
            .filter(|l| l.contains("portal login success"))
            .count(),
        2
    );
}

#[tokio::test]
async fn ensure_routes_only_on_portal_auth() {
    let mut h = Harness::new(wireless_cfg(NetMode::WiredPlusStandby), false);
    h.world.wireless = wlan_sample("10.1.1.5", Some("10.1.1.1"), 15);
    h.push(Event::Started).await;
    let first = without_metric(h.take_wireless());
    assert!(first.iter().any(|e| matches!(e, Effect::Associate(_))));
    assert!(
        !first
            .iter()
            .any(|e| matches!(e, Effect::EnsureRoutes { .. })),
        "Associate must not ensure routes: {first:?}"
    );
    h.advance_to_wake().await;
    h.take_wireless();
    h.push(Event::AssociateFinished(Ok(()))).await;

    // wlan IP 有、网关缺失：不产生 EnsureRoutes/Settle/PortalAuth（on_auth(false) 回报）。
    h.world.wireless = wlan_sample("10.1.1.5", None, 15);
    h.run_until(2_000).await;
    assert!(
        h.take_wireless().is_empty(),
        "PortalAuth without gateway must not ensure routes"
    );

    // Error 退避后重新关联，再以齐备网关认证：EnsureRoutes + Settle + PortalAuth。
    h.run_until(8_000).await;
    assert!(without_metric(h.take_wireless())
        .iter()
        .any(|e| matches!(e, Effect::Associate(_))));
    h.push(Event::AssociateFinished(Ok(()))).await;
    h.world.wireless = wlan_sample("10.1.1.5", Some("10.1.1.1"), 15);
    h.run_until(10_000).await;
    assert_eq!(
        without_metric(h.take_wireless()),
        vec![
            Effect::EnsureRoutes {
                dests: vec![Ipv4Addr::new(10, 0, 3, 2), Ipv4Addr::new(223, 5, 5, 5)],
                gateway: Ipv4Addr::new(10, 1, 1, 1),
                ifindex: 15,
            },
            Effect::Settle(Duration::from_secs(3)),
            Effect::PortalAuth {
                src_ip: Ipv4Addr::new(10, 1, 1, 5),
                timeout: Duration::from_secs(20),
            },
        ]
    );
}

#[tokio::test]
async fn takeover_debounce_and_release_after() {
    let mut h = Harness::new(wireless_cfg(NetMode::WiredExclusive), false);
    h.push(Event::Started).await;
    h.advance_to_wake().await; // t=0: 拨号 → Connected
    h.take_wireless();

    // 有线健康：保持 Off，不接管。
    h.run_until(6_000).await;
    assert!(!h
        .take_wireless()
        .iter()
        .any(|e| matches!(e, Effect::Associate(_))));

    // 拔线：8000 首次判不健康；8s 去抖后 16000 才接管。
    h.world.link = Some(false);
    h.run_until(15_998).await;
    assert!(
        !h.take_wireless()
            .iter()
            .any(|e| matches!(e, Effect::Associate(_))),
        "takeover must wait takeover_after=8s"
    );
    h.run_until(16_000).await;
    assert!(
        h.take_wireless()
            .iter()
            .any(|e| matches!(e, Effect::Associate(_))),
        "takeover at unhealthy_since+8s"
    );

    // 插线恢复：有线健康 10s 后让位（release_after=10s）。
    h.world.link = Some(true);
    h.run_until(27_998).await;
    assert!(
        !h.take_wireless()
            .iter()
            .any(|e| matches!(e, Effect::Disassociate)),
        "release must wait release_after=10s"
    );
    h.run_until(28_000).await;
    let effects = h.take_wireless();
    assert!(
        effects.iter().any(|e| matches!(e, Effect::Disassociate)),
        "release at healthy_since+10s: {effects:?}"
    );
    assert!(h.ring_has("Wireless: releasing (wired healthy)"));
}

#[tokio::test]
async fn metric_suppress_release_by_mode_and_wired_health() {
    let mut h = Harness::new(wireless_cfg(NetMode::WiredPlusStandby), false);
    h.world.wireless = wlan_sample("10.1.1.5", Some("10.1.1.1"), 15);
    h.push(Event::Started).await;
    assert_eq!(
        h.take_wireless(),
        vec![
            Effect::CleanupStaleRoutes,
            Effect::Associate("gdut".into()),
            Effect::SuppressMetric {
                ifindex: 15,
                target: 100
            },
        ],
        "standby suppresses WLAN metric"
    );
    h.advance_to_wake().await; // t=0: 拨号 → Connected
    h.take_wireless();

    // 每拍重发（final-review fix）：条件成立的连续两拍都发 SuppressMetric，
    // RouteGuard 内部去重；瞬时 SetIpInterfaceEntry 失败在下一拍自愈。
    h.run_until(2_000).await;
    assert_eq!(
        metric_effects(&h.take_wireless()),
        vec![Effect::SuppressMetric {
            ifindex: 15,
            target: 100
        }],
        "standby tick 1"
    );
    h.run_until(4_000).await;
    assert_eq!(
        metric_effects(&h.take_wireless()),
        vec![Effect::SuppressMetric {
            ifindex: 15,
            target: 100
        }],
        "standby tick 2: retried, not one-shot"
    );

    // 切 exclusive 且有线健康 → 连续两拍都发 ReleaseMetric（不再发 Suppress）。
    h.push(Event::Command(Command::SetMode {
        mode: NetMode::WiredExclusive,
    }))
    .await;
    h.run_until(6_000).await;
    assert_eq!(
        metric_effects(&h.take_wireless()),
        vec![Effect::ReleaseMetric],
        "exclusive + wired healthy tick 1"
    );
    h.run_until(8_000).await;
    assert_eq!(
        metric_effects(&h.take_wireless()),
        vec![Effect::ReleaseMetric],
        "exclusive + wired healthy tick 2: retried, not one-shot"
    );

    // 拔线（有线不健康）→ 恢复每拍压制。
    h.world.link = Some(false);
    h.run_until(10_000).await;
    assert_eq!(
        metric_effects(&h.take_wireless()),
        vec![Effect::SuppressMetric {
            ifindex: 15,
            target: 100
        }],
        "exclusive + unhealthy suppresses metric again"
    );

    // 插线恢复健康 → 每拍再释放（拔线期间链路轮询 2s，恢复检测在下一个轮询拍）。
    h.world.link = Some(true);
    h.run_until(12_000).await;
    assert_eq!(
        metric_effects(&h.take_wireless()),
        vec![Effect::ReleaseMetric],
        "wired healthy again releases metric"
    );
    h.run_until(14_000).await;
    assert_eq!(
        metric_effects(&h.take_wireless()),
        vec![Effect::ReleaseMetric],
        "and keeps releasing every tick"
    );
}

#[tokio::test]
async fn metric_zero_target_still_emits_suppress_for_the_adapter_noop() {
    // 决策在核心、幂等在 adapter：target==0 照发（RouteGuard 内部 no-op）。
    let mut cfg = wireless_cfg(NetMode::WiredPlusStandby);
    cfg.wireless.standby_metric = 0;
    let mut h = Harness::new(cfg, false);
    h.world.wireless = wlan_sample("10.1.1.5", Some("10.1.1.1"), 15);
    h.push(Event::Started).await;
    assert!(
        h.take_wireless().contains(&Effect::SuppressMetric {
            ifindex: 15,
            target: 0
        }),
        "core makes the decision; the adapter no-ops on target 0"
    );
}

#[tokio::test]
async fn teardown_on_release_and_stop() {
    let mut h = Harness::new(wireless_cfg(NetMode::WiredPlusStandby), false);
    h.world.wireless = wlan_sample("10.1.1.5", Some("10.1.1.1"), 15);
    h.push(Event::Started).await;
    h.take_wireless();
    h.advance_to_wake().await; // t=0: 拨号 → Connected
    h.push(Event::AssociateFinished(Ok(()))).await;
    h.run_until(2_000).await; // PortalAuth 三连
    h.take_wireless();
    h.push(Event::PortalFinished(PortalAttempt::Replied {
        status: 200,
        body: PORTAL_SUCCESS.into(),
    }))
    .await;
    assert_eq!(h.sup.snapshot().wireless.phase, WPhase::Online);

    // 切 exclusive 且有线健康：10s 后让位 → TeardownRoutes + Disassociate。
    h.push(Event::Command(Command::SetMode {
        mode: NetMode::WiredExclusive,
    }))
    .await;
    h.run_until(13_998).await;
    assert!(!h
        .take_wireless()
        .iter()
        .any(|e| matches!(e, Effect::TeardownRoutes)));
    h.run_until(14_000).await;
    assert_eq!(
        without_metric(h.take_wireless()),
        vec![Effect::TeardownRoutes, Effect::Disassociate]
    );
    assert!(h.ring_has("Wireless: releasing (wired healthy)"));

    // Stop：Main Hangup + Wireless teardown/disassociate（自包含铁律，不被取消）。
    h.take_effects();
    let reactions = h.push(Event::Stop).await;
    assert_eq!(
        h.take_effects(),
        vec![Effect::Hangup, Effect::TeardownRoutes, Effect::Disassociate]
    );
    assert!(h.ring_has("Wireless: manager stopped"));
    assert_eq!(reactions.last().unwrap().wake_at, None);
}

#[tokio::test]
async fn join_timeout_restarts_association() {
    let mut h = Harness::new(wireless_cfg(NetMode::WiredPlusStandby), false);
    h.push(Event::Started).await;
    h.take_wireless();
    h.advance_to_wake().await; // t=0: 拨号
    h.push(Event::AssociateFinished(Ok(()))).await; // join_since = 0

    h.run_until(60_000).await;
    assert!(
        !h.ring_has("Wireless: join timeout"),
        "exactly 60s is not over the timeout"
    );
    h.run_until(62_000).await;
    assert!(h.ring_has("Wireless: join timeout, restarting"));
    assert!(
        !h.take_wireless()
            .iter()
            .any(|e| matches!(e, Effect::Associate(_))),
        "restart happens after the tick's decision"
    );

    h.run_until(64_000).await;
    assert!(
        h.take_wireless()
            .iter()
            .any(|e| matches!(e, Effect::Associate(_))),
        "brain.restart → next tick reassociates"
    );
}

#[tokio::test]
async fn wireless_died_degrades_to_default_snapshot() {
    let mut h = Harness::new(wireless_cfg(NetMode::WiredPlusStandby), false);
    h.world.wireless = wlan_sample("10.1.1.5", Some("10.1.1.1"), 15);
    h.push(Event::Started).await;
    h.take_wireless();
    h.advance_to_wake().await;
    h.push(Event::AssociateFinished(Ok(()))).await;
    h.run_until(2_000).await;
    h.push(Event::PortalFinished(PortalAttempt::Replied {
        status: 200,
        body: PORTAL_SUCCESS.into(),
    }))
    .await;
    assert_eq!(h.sup.snapshot().wireless.phase, WPhase::Online);
    h.take_wireless();

    h.push(Event::WirelessDied).await;
    assert!(h.ring_has("Wireless: manager stopped"));
    assert_eq!(h.sup.snapshot().wireless, WirelessSnapshot::default());

    // 无线 lane 死后：节拍停止，不再产生无线效果。
    h.run_until(10_000).await;
    assert!(h.take_wireless().is_empty());
}

/// 事件环消息去 `[HH:MM:SS] ` 前缀后去重收集（冻结文案逐字断言）。
fn collect_ring(h: &Harness, out: &mut Vec<String>) {
    for line in h.ring() {
        let msg = match line.split_once("] ") {
            Some((_, m)) => m.to_string(),
            None => line,
        };
        if !out.contains(&msg) {
            out.push(msg);
        }
    }
}

#[tokio::test]
async fn event_ring_strings_are_frozen_byte_for_byte() {
    let mut seen: Vec<String> = Vec::new();

    // 有线侧：启动 / 切模式 / 插线即拨。
    let mut h = Harness::new(wired_cfg(), false);
    h.push(Event::Started).await;
    collect_ring(&h, &mut seen);
    h.push(Event::Command(Command::SetMode {
        mode: NetMode::WiredPlusStandby,
    }))
    .await;
    collect_ring(&h, &mut seen);
    h.advance_to_wake().await; // t=0: 拨号
    h.world.link = Some(false);
    h.clock.advance(2_000);
    h.push(Event::Wake).await; // 拔线采样
    h.world.link = Some(true);
    h.advance_to_wake().await; // 插线边沿
    collect_ring(&h, &mut seen);

    // 无线侧：关联 / join 超时 / 认证失败与成功 / 探测 / 让位 / manager 退出。
    let mut w = Harness::new(wireless_cfg(NetMode::WiredPlusStandby), false);
    w.push(Event::Started).await;
    collect_ring(&w, &mut seen);
    w.push(Event::AssociateFinished(Ok(()))).await;
    w.advance_to_wake().await; // t=0: 拨号
    w.run_until(62_000).await; // join 超时（60s 后首个到点拍）
    collect_ring(&w, &mut seen);

    w.world.wireless = wlan_sample("10.1.1.5", Some("10.1.1.1"), 15);
    w.run_until(64_000).await; // 重新关联
    w.push(Event::AssociateFinished(Ok(()))).await;
    w.run_until(66_000).await; // PortalAuth
    w.push(Event::PortalFinished(PortalAttempt::Failed(
        "bad password".into(),
    )))
    .await;
    collect_ring(&w, &mut seen);

    w.run_until(72_000).await; // Error 退避后重新关联
    w.push(Event::AssociateFinished(Ok(()))).await;
    w.run_until(76_000).await; // PortalAuth
    w.push(Event::PortalFinished(PortalAttempt::Replied {
        status: 200,
        body: PORTAL_SUCCESS.into(),
    }))
    .await;
    w.run_until(78_000).await; // ProbeNow
    w.push(Event::WlanProbeFinished(ProbeVerdict::Kicked)).await;
    collect_ring(&w, &mut seen);

    w.push(Event::Command(Command::SetMode {
        mode: NetMode::WiredExclusive,
    }))
    .await;
    w.run_until(90_000).await; // 有线健康 10s → 让位
    collect_ring(&w, &mut seen);
    w.push(Event::WirelessDied).await;
    collect_ring(&w, &mut seen);

    for expected in [
        "Service started, mode wired-plus-standby",
        "Mode switched to wired-plus-standby",
        "Ethernet link restored, redialing",
        "Wireless: associating to campus SSID",
        "Wireless: join timeout, restarting",
        "Wireless: portal login failed: bad password",
        "Wireless: portal login success",
        "Wireless probe: Kicked",
        "Wireless: releasing (wired healthy)",
        "Wireless: manager stopped",
    ] {
        assert!(
            seen.iter().any(|m| m == expected),
            "missing frozen ring line {expected:?}; seen {seen:?}"
        );
    }
}
