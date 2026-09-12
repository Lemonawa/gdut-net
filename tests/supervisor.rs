//! Task 8 harness：真 `Watchdog` + 脚本化世界 + `FakeClock` 驱动 `Supervisor`。
//!
//! 时间线以毫秒推进（测试总时长 < 1s 真实时间）；Main 车道效果由 harness 内联执行并
//! 把结果事件回灌（等价 Task 9 壳的同步执行），Wireless 车道效果记录待测。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gdut_net::config::Config;
use gdut_net::ipc::protocol::{Command, HeartbeatStatus, NetMode, SessionStatus};
use gdut_net::probe::ProbeVerdict;
use gdut_net::ras::ErrKind;
use gdut_net::supervisor::{
    Clock, Effect, Event, Lane, PortalAttempt, Reaction, Supervisor, ToastKey, WatchdogStep,
    WirelessSample, WlanSample,
};
use gdut_net::watchdog::{DialError, Dialer, Prober, Watchdog, WatchdogCfg};

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
    failing: Arc<AtomicBool>,
    auto_sample_wireless: bool,
}

impl Harness {
    fn new(cfg: Config, failing: bool) -> Self {
        let clock = FakeClock::new();
        let dial_calls = Arc::new(AtomicU32::new(0));
        let failing = Arc::new(AtomicBool::new(failing));
        let dialer = MockDialer {
            calls: dial_calls.clone(),
            fail: failing.clone(),
            connected: false,
        };
        let wd = Watchdog::new(
            dialer,
            MockProber(vec![ProbeVerdict::Alive]),
            WatchdogCfg {
                redial_min: Duration::from_secs(1),
                redial_max: Duration::from_secs(300),
                probe_interval: Duration::from_secs(cfg.dial.probe_interval_secs),
                auth_fail_delay: Duration::from_secs(600),
            },
        );
        let sup = Supervisor::new(cfg, "s3cret".into(), clock.clone());
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
            failing,
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
                    Lane::Wireless => {
                        if self.auto_sample_wireless && matches!(effect, Effect::SampleWireless) {
                            queue.push_back(Event::WirelessSampled(self.world.wireless));
                        } else {
                            self.wireless_out.push(effect);
                        }
                    }
                }
            }
            reactions.push(r);
        }
        reactions
    }

    async fn exec_main(&mut self, effect: &Effect, queue: &mut VecDeque<Event>) {
        match effect {
            Effect::SampleLink => queue.push_back(Event::LinkSampled(self.world.link)),
            Effect::SampleWireless => {
                queue.push_back(Event::WirelessSampled(self.world.wireless));
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
}

/// 过滤 metric 效果，便于断言核心动作序列。
fn without_metric(effects: Vec<Effect>) -> Vec<Effect> {
    effects
        .into_iter()
        .filter(|e| !matches!(e, Effect::SuppressMetric { .. } | Effect::ReleaseMetric))
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
    let mut h = Harness::new(wired_cfg(), false);
    h.start_and_connect().await;
    h.take_effects();

    // 2s 链路轮询发现拔线：只更新门控，不步进。
    h.clock.advance(2_000);
    h.world.link = Some(false);
    h.push(Event::Wake).await;
    assert_eq!(
        h.take_effects(),
        vec![Effect::SampleLink, Effect::SetWatchdogLink(Some(false))]
    );
    assert_eq!(
        h.wake_at,
        Some(h.clock.now() + 5_000),
        "cable-out polls every 5s (LINK_DOWN_RETRY)"
    );

    // 到点 Wake：不产生 StepWatchdog，拨号计数不变。
    h.advance_to_wake().await;
    let effects = h.take_effects();
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::StepWatchdog)),
        "I5: no StepWatchdog while cable is out: {effects:?}"
    );
    assert_eq!(effects, vec![Effect::SampleLink]);
    assert_eq!(h.wake_at, Some(h.clock.now() + 5_000));
    assert_eq!(
        h.dial_calls.load(Ordering::SeqCst),
        1,
        "cable out never dials"
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
