//! 服务运行时装配：把 ras/adapter/probe/watchdog/heartbeat 接成可运行的主体。
//!
//! `start_all` 由 service_main（服务线程，非 main）调用：内部自建多线程
//! tokio runtime 并 block_on。装配链路：
//!
//! ```text
//! Config ──► crypto::unprotect(密码)
//!        ├─► RealDialer（ras::dial / RasSession）─┐
//!        ├─► RealProber（adapter 刷新 + probe_once）─┤─► Supervisor（Task 8 纯核心）
//!        ├─► watch::<StateSnapshot>（IPC status/托盘消费；唯一发布点）
//!        ├─► 无线 lane worker（wireless.enabled 时 spawn）：串行执行
//!        │     Supervisor 的 Wireless 车道效果（路由/metric/关联/认证/探测）
//!        └─► heartbeat::session::run_blocking（spawn_blocking 内运行；
//!            enabled 时；装配失败 60s 循环重试）
//! ```
//!
//! 主循环是薄执行器：事件喂 `Supervisor::on` → Main 效果内联执行、结果事件
//! FIFO 回灌 → Wireless 效果送 worker。睡眠只依据 `Reaction.wake_at`（绝对
//! 时刻，不被事件切碎）；快照只在 `Reaction.snapshot == Some` 时经唯一
//! `snap_tx.send` 发布（I2）。停止握手：Stop 反应 → Hangup 内联（不可取消）
//! → worker 收 `Shutdown`（teardown 效果不可取消）→ 等 worker 退出。
//!
//! 仅 Windows 编译，以 `cargo check --target x86_64-pc-windows-msvc` 验证。

#[cfg(windows)]
mod win {
    use std::collections::VecDeque;
    use std::net::Ipv4Addr;
    use std::path::PathBuf;
    use std::time::Duration;

    use anyhow::{bail, Context, Result};
    use tokio::sync::{mpsc, watch};
    use tokio::time::sleep;
    use tokio_util::sync::CancellationToken;

    use crate::adapter::{self, AdapterInfo};
    use crate::backoff::AUTH_FAIL_DELAY;
    use crate::config::Config;
    use crate::heartbeat::session;
    use crate::ipc::protocol::{Command, HeartbeatStatus, StateSnapshot};
    use crate::ipc::server;
    use crate::probe;
    use crate::supervisor::{
        Clock, Effect, Event, Lane, Supervisor, SystemClock, WatchdogStep, WirelessSample,
        WlanSample,
    };
    use crate::watchdog::{DialError, Dialer, Prober, Watchdog, WatchdogCfg};
    use crate::wireless::routes::{self, RouteGuard};
    use crate::wireless::wlan;
    use crate::{crypto, notify, ras};

    /// 心跳装配失败后的重试间隔。
    const HEARTBEAT_RETRY_DELAY: Duration = Duration::from_secs(60);
    /// 停止时等待无线 worker 退出的上限（runtime.shutdown_timeout 的前置礼貌）。
    const WORKER_EXIT_TIMEOUT: Duration = Duration::from_secs(5);

    /// RasSession 句柄包装：HRASCONN 是不透明指针（*mut c_void），Win32 RAS
    /// 句柄不线程亲和，单一所有者顺序使用下跨线程移动安全。
    struct SendSession(ras::RasSession);
    unsafe impl Send for SendSession {}

    /// watchdog::Dialer 真实现：持配置与解密后的密码，会话句柄存 Option。
    struct RealDialer {
        pbk: String,
        entry: String,
        user: String,
        pass: String,
        session: Option<SendSession>,
        /// 连续 756/813（端口卡在 dialing 态）计数；达阈值重启 RasMan 清端口。
        wedge_fails: u32,
    }

    /// 连续 756/813 达该值 → 重启 RasMan（真机 2026-09-10：无载波拨号后
    /// 端口卡死，所有拨号返回 756，重试无法清除，重启系统才恢复）。
    const WEDGE_RESTART_AFTER: u32 = 3;

    impl RealDialer {
        fn new(cfg: &Config, pass: String) -> Self {
            Self {
                pbk: cfg.dial.pbk_path.clone(),
                entry: cfg.dial.entry_name.clone(),
                user: cfg.account.student_id.clone(),
                pass,
                session: None,
                wedge_fails: 0,
            }
        }
    }

    #[async_trait::async_trait]
    impl Dialer for RealDialer {
        async fn dial(&mut self) -> Result<(), DialError> {
            // RasDial 是阻塞调用（同步等待会话建立），丢进 blocking 线程。
            let pbk = self.pbk.clone();
            let entry = self.entry.clone();
            let user = self.user.clone();
            let pass = self.pass.clone();
            let res = tokio::task::spawn_blocking(move || {
                ras::dial(&pbk, &entry, &user, &pass).map(SendSession)
            })
            .await
            .map_err(|e| DialError {
                kind: ras::ErrKind::Transient,
                code: 0,
                msg: format!("Dial task join failed: {e}"),
            })
            .and_then(|r| match r {
                Ok(s) => Ok(s),
                Err(ras::RasError::Auth) => Err(DialError {
                    kind: ras::ErrKind::Auth,
                    code: 691,
                    msg: "Authentication failed (691): invalid student ID or password".to_string(),
                }),
                Err(ras::RasError::Other { code, msg }) => Err(DialError {
                    kind: ras::ErrKind::Transient,
                    code,
                    msg,
                }),
            });
            match res {
                Ok(s) => {
                    self.wedge_fails = 0;
                    self.session = Some(s);
                    Ok(())
                }
                Err(e) => {
                    if matches!(e.code, 756 | 813) {
                        self.wedge_fails += 1;
                        if self.wedge_fails >= WEDGE_RESTART_AFTER {
                            log::warn!(
                                "Dial port stuck (error {} x{}), restarting RasMan",
                                e.code,
                                self.wedge_fails
                            );
                            let r = tokio::task::spawn_blocking(ras::restart_rasman).await;
                            match r {
                                Ok(Ok(())) => log::info!("RasMan restarted, port state cleared"),
                                Ok(Err(e)) => log::warn!("RasMan restart failed: {e:#}"),
                                Err(e) => log::warn!("RasMan restart task join failed: {e}"),
                            }
                            self.wedge_fails = 0;
                        }
                    } else {
                        self.wedge_fails = 0;
                    }
                    Err(e)
                }
            }
        }

        async fn hangup(&mut self) {
            if let Some(s) = self.session.take() {
                if let Err(e) = s.0.hangup() {
                    log::warn!("Hangup failed (ignored): {e:#}");
                }
            }
        }

        fn is_connected(&mut self) -> bool {
            match self.session.as_ref() {
                Some(s) => matches!(s.0.status(), Ok(ras::ConnState::Connected)),
                None => false,
            }
        }
    }

    /// watchdog::Prober 真实现：优先绑 PPPoE 会话口探测（校园网把 DHCP 口与
    /// PPP 口隔离——物理口 ping 网关永远不通，绑物理口会误判掉线自杀循环）；
    /// 会话不存在（未拨号）时退回物理口，探链路供拨号失败诊断。
    struct RealProber {
        interface: String,
        http_url: String,
    }

    impl RealProber {
        /// 按配置解析探测适配器；interface 非空时按 FriendlyName 精确匹配。
        fn resolve_adapter(&self) -> Result<AdapterInfo> {
            let ppp = adapter::ppp_adapter();
            if let Some(a) = ppp {
                if self.interface.is_empty() || a.name == self.interface {
                    return Ok(a);
                }
            }
            let physical = adapter::physical_adapter()?;
            if self.interface.is_empty() || physical.name == self.interface {
                Ok(physical)
            } else {
                bail!(
                    "Config dial.interface={:?} does not match current physical adapter {:?}",
                    self.interface,
                    physical.name
                )
            }
        }
    }

    #[async_trait::async_trait]
    impl Prober for RealProber {
        async fn probe(&mut self) -> crate::probe::ProbeVerdict {
            match self.resolve_adapter() {
                Ok(a) => probe::probe_once(a.ipv4, a.gateway, &self.http_url).await,
                Err(e) => {
                    log::warn!("Failed to resolve adapter before probe: {e:#}");
                    crate::probe::ProbeVerdict::LinkDown
                }
            }
        }
    }

    /// 无线 lane 消息：效果串行执行；`Shutdown` 携带 Stop 反应的 teardown
    /// 效果（不可取消），执行完 worker 退出。
    enum LaneMsg {
        Effect(Effect),
        Shutdown(Vec<Effect>),
    }

    /// 无线接管执行体（ADR-0005）：串行执行 Supervisor 的无线车道效果。
    /// 决策正确性由 `Supervisor` 纯逻辑测试背书；本 worker 只做采信执行。
    struct WirelessWorker {
        portal_ip: Ipv4Addr,
        probe_ip: Ipv4Addr,
        guard: RouteGuard,
    }

    impl WirelessWorker {
        /// 解析 portal/probe 目标；与 `Supervisor::new` 同一判定（配置可被手改）。
        fn new(cfg: &Config) -> Result<Self> {
            let portal_ip =
                probe::parse_http_probe_target(&cfg.wireless.portal_url).map(|(ip, _)| ip);
            let probe_ip: Option<Ipv4Addr> = cfg.wireless.probe_host.parse().ok();
            let (Some(portal_ip), Some(probe_ip)) = (portal_ip, probe_ip) else {
                bail!("Wireless: portal_url/probe_host invalid, manager disabled");
            };
            Ok(Self {
                portal_ip,
                probe_ip,
                guard: RouteGuard::new(),
            })
        }

        async fn run(mut self, mut lane_rx: mpsc::Receiver<LaneMsg>) {
            while let Some(msg) = lane_rx.recv().await {
                match msg {
                    LaneMsg::Effect(effect) => self.run_effect(effect, true).await,
                    LaneMsg::Shutdown(effects) => {
                        for effect in effects {
                            self.run_effect(effect, false).await;
                        }
                        break;
                    }
                }
            }
            // 兜底（自包含铁律第三出口）：Guard Drop 删路由 + 还原 metric；
            // 显式 Shutdown 后为空操作。
        }

        /// 执行一条无线效果。`race_stop=true` 时长 await 与 stop 竞争（取消即
        /// 放弃结果）；`false` = 停止握手效果，必须执行完（不可取消）。
        async fn run_effect(&mut self, effect: Effect, _race_stop: bool) {
            match effect {
                Effect::CleanupStaleRoutes => {
                    routes::cleanup_stale(&[self.portal_ip, self.probe_ip]);
                }
                Effect::EnsureRoutes {
                    dests,
                    gateway,
                    ifindex,
                } => self.guard.ensure(&dests, gateway, ifindex),
                Effect::SuppressMetric { ifindex, target } => {
                    self.guard.set_standby_metric(ifindex, target);
                }
                Effect::ReleaseMetric => self.guard.release_metric(),
                Effect::TeardownRoutes => self.guard.teardown(),
                Effect::Disassociate => {
                    let _ = tokio::task::spawn_blocking(wlan::disassociate).await;
                }
                Effect::Associate(_)
                | Effect::Settle(_)
                | Effect::PortalAuth { .. }
                | Effect::WlanProbe { .. } => {
                    log::warn!("Wireless worker: effect pending implementation: {effect:?}");
                }
                other => log::warn!("Wireless worker received non-wireless effect: {other:?}"),
            }
        }
    }

    /// 壳侧执行器状态：Supervisor + watchdog + 持久化配置 + 唯一发布通道。
    struct Shell {
        core: Supervisor,
        watchdog: Watchdog,
        /// Config 副本（Supervisor 持自己的原件）；仅 SetMode 持久化用。
        cfg: Config,
        cfg_path: PathBuf,
        snap_tx: watch::Sender<StateSnapshot>,
        /// Wireless lane 发送端（worker 未启用时不会收到无线效果）。
        lane_tx: mpsc::Sender<LaneMsg>,
        /// Stop 反应收集的无线 teardown 效果（交 worker 不可取消执行）。
        shutdown_effects: Vec<Effect>,
        /// 已进入停止流程（Stop 事件已入队/处理中）。
        stopping: bool,
        /// 核心给的下一次绝对唤醒（单调毫秒）。
        wake_at: Option<u64>,
        /// 壳侧读取同一单调时钟（与核心 SystemClock 起点差可忽略）。
        clock: SystemClock,
    }

    impl Shell {
        /// Main 车道效果内联执行；返回结果事件（同轮 FIFO 回灌）。
        async fn exec_main(&mut self, effect: Effect) -> Option<Event> {
            match effect {
                Effect::StepWatchdog => {
                    let delay = self.watchdog.run_once().await;
                    let ppp_ip = adapter::ppp_adapter_ip().map(|ip| ip.to_string());
                    Some(Event::WatchdogStepped(WatchdogStep {
                        delay,
                        session: self.watchdog.view(),
                        ppp_ip,
                    }))
                }
                Effect::RequestRedial => {
                    self.watchdog.request_redial();
                    None
                }
                Effect::SetWatchdogLink(up) => {
                    self.watchdog.set_eth_link(up);
                    None
                }
                Effect::Hangup => {
                    self.watchdog.shutdown().await;
                    None
                }
                Effect::SampleLink => {
                    let up = tokio::task::spawn_blocking(adapter::ethernet_link_up)
                        .await
                        .unwrap_or(None);
                    Some(Event::LinkSampled(up))
                }
                Effect::SampleWireless => {
                    // 一次 blocking 采样：关联态 + WLAN 适配器（同旧 manager 一拍）。
                    let (associated, wlan) = tokio::task::spawn_blocking(|| {
                        (wlan::associated(), adapter::wlan_adapter())
                    })
                    .await
                    .unwrap_or((false, None));
                    Some(Event::WirelessSampled(WirelessSample {
                        associated,
                        wlan: wlan.map(|a| WlanSample {
                            ipv4: a.ipv4,
                            gateway: a.gateway,
                            ifindex: a.ifindex,
                        }),
                    }))
                }
                Effect::PersistMode(mode) => {
                    self.cfg.wireless.mode = mode;
                    if let Err(e) = self.cfg.save(&self.cfg_path) {
                        log::warn!("Failed to persist mode to config (ignored): {e:#}");
                    }
                    None
                }
                Effect::Notify { key, title, body } => {
                    let delivered = match notify::toast(&title, &body) {
                        Ok(()) => true,
                        Err(e) => {
                            log::warn!("Toast failed (ignored): {e:#}");
                            false
                        }
                    };
                    Some(Event::NotifyResult { key, delivered })
                }
                other => {
                    debug_assert!(false, "wireless effect on Main lane: {other:?}");
                    log::error!("Main executor received wireless effect: {other:?}");
                    None
                }
            }
        }

        /// 喂事件给核心：内联执行 Main 效果（结果 FIFO 回灌），Wireless 效果
        /// 转发 worker；快照只在核心判定变化时经唯一发布点推送。
        async fn drain(&mut self, queue: &mut VecDeque<Event>) {
            while let Some(event) = queue.pop_front() {
                let reaction = self.core.on(event);
                self.wake_at = reaction.wake_at;
                // 快照唯一发布点（I2）。
                if let Some(snapshot) = reaction.snapshot {
                    let _ = self.snap_tx.send(snapshot);
                }
                for effect in reaction.effects {
                    match effect.lane() {
                        Lane::Main => {
                            if let Some(result) = self.exec_main(effect).await {
                                queue.push_back(result);
                            }
                        }
                        Lane::Wireless => {
                            if self.stopping {
                                self.shutdown_effects.push(effect);
                            } else if self.lane_tx.send(LaneMsg::Effect(effect)).await.is_err() {
                                log::warn!("Wireless lane closed, effect dropped");
                            }
                        }
                    }
                }
            }
        }
    }

    /// 服务主体：建 runtime → 装配 → 循环直到 stop → hangup 收尾。
    pub fn start_all(cfg: Config, cfg_path: PathBuf, stop: CancellationToken) -> Result<()> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("Failed to create tokio runtime")?;
        let result = runtime.block_on(run(cfg, cfg_path, stop));
        // blocking 任务已由 hb_stop 保证退出；兜底防万一（如 recv 卡死在驱动层）。
        runtime.shutdown_timeout(Duration::from_secs(10));
        result
    }

    async fn run(cfg: Config, cfg_path: PathBuf, stop: CancellationToken) -> Result<()> {
        // 密码运行时解密一次；失败 bail（重输密码场景由 install 负责）。
        let pass = crypto::unprotect(&cfg.account.password_blob)
            .context("Failed to decrypt password_blob (re-run install to enter password)")?;

        // 心跳先行装配（不阻塞拨号）：bind 失败只影响兼容模式，不上抛。
        // 装配循环重试：适配器/网络未就绪时每 60s 自愈；停止后由 runtime
        // shutdown_timeout 收走本任务。
        let (hb_tx, mut hb_rx) = watch::channel(HeartbeatStatus::Off);
        if cfg.heartbeat.enabled {
            let server: Ipv4Addr = cfg.heartbeat.server.parse().with_context(|| {
                format!(
                    "Failed to parse heartbeat.server: {:?}",
                    cfg.heartbeat.server
                )
            })?;
            let port = cfg.heartbeat.port;
            let interval = Duration::from_secs(cfg.heartbeat.interval_secs);
            let hb_stop = stop.child_token();
            let hb_tx = hb_tx.clone();
            tokio::spawn(async move {
                loop {
                    if hb_stop.is_cancelled() {
                        break;
                    }
                    // 阻塞段（GAA 枚举 + run_blocking 的 std recv/sleep 循环）
                    // 整体进 blocking 线程池，不占 tokio worker。
                    let attempt = tokio::task::spawn_blocking({
                        let hb_stop = hb_stop.clone();
                        let hb_tx = hb_tx.clone();
                        move || match adapter::physical_adapter() {
                            Ok(a) => {
                                log::info!(
                                    "Heartbeat starting: server={server}:{port} src={} (physical {})",
                                    a.ipv4,
                                    a.name
                                );
                                // bind 61440 失败：报错（Rule：非静默），60s 后重试——
                                // 用户关掉官方客户端后可自愈。
                                session::run_blocking(
                                    server, port, a.ipv4, interval, hb_stop, hb_tx,
                                )
                            }
                            Err(e) => Err(format!("Heartbeat: physical adapter not found: {e:#}")),
                        }
                    })
                    .await;
                    match attempt {
                        Ok(Ok(())) => log::info!("Heartbeat session ended normally"),
                        Ok(Err(e)) => {
                            log::warn!("Heartbeat session exited (retry in 60s): {e}");
                            let _ = hb_tx.send(HeartbeatStatus::Error(e));
                        }
                        Err(e) => {
                            log::warn!("Heartbeat task join failed (retry in 60s): {e}");
                            let _ = hb_tx
                                .send(HeartbeatStatus::Error(format!("Heartbeat task error: {e}")));
                        }
                    }
                    tokio::select! {
                        _ = hb_stop.cancelled() => break,
                        _ = sleep(HEARTBEAT_RETRY_DELAY) => {}
                    }
                }
                let _ = hb_tx.send(HeartbeatStatus::Off);
            });
        }

        // watchdog 装配（真实现原位保留，核心只发效果不持有 dialer/prober）。
        let watchdog_cfg = WatchdogCfg {
            redial_min: Duration::from_secs(1),
            redial_max: Duration::from_secs(300),
            probe_interval: Duration::from_secs(cfg.dial.probe_interval_secs),
            auth_fail_delay: AUTH_FAIL_DELAY,
        };
        let dialer = RealDialer::new(&cfg, pass.clone());
        let prober = RealProber {
            interface: cfg.dial.interface.clone(),
            http_url: cfg.dial.http_probe_url.clone(),
        };
        let watchdog = Watchdog::new(dialer, prober, watchdog_cfg);

        // 组合核心 + IPC server：watch 初值即核心组合快照（客户端连上即见）。
        let core = Supervisor::new(cfg.clone(), SystemClock::new());
        let (snap_tx, snap_rx) = watch::channel(core.snapshot());
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Command>(16);
        server::spawn_server(snap_rx, cmd_tx, stop.clone());

        // 无线 lane worker（wireless.enabled 且 portal/probe 可解析时 spawn）。
        let (lane_tx, lane_rx) = mpsc::channel::<LaneMsg>(64);
        let (_wl_tx, mut wl_rx) = mpsc::channel::<Event>(64);
        let mut worker_handle = None;
        if cfg.wireless.enabled {
            match WirelessWorker::new(&cfg) {
                Ok(worker) => worker_handle = Some(tokio::spawn(worker.run(lane_rx))),
                Err(e) => log::error!("{e:#}"),
            }
        }

        let mut shell = Shell {
            core,
            watchdog,
            cfg,
            cfg_path,
            snap_tx,
            lane_tx,
            shutdown_effects: Vec::new(),
            stopping: false,
            wake_at: None,
            clock: SystemClock::new(),
        };

        // 装配完成事件：核心由此进入运行态（首拍立即调度链路采样 + 拨号）。
        let mut queue = VecDeque::new();
        queue.push_back(Event::Started);
        shell.drain(&mut queue).await;

        // 通道存活标志：watch/mpsc 关闭后必须停用分支，防 busy-loop。
        let mut hb_alive = true;
        let mut wireless_alive = worker_handle.is_some();

        loop {
            if shell.stopping {
                break;
            }
            let sleep_for = match shell.wake_at {
                Some(at) => Duration::from_millis(at.saturating_sub(shell.clock.now_ms())),
                None => Duration::from_secs(60 * 60 * 24), // 无定时器：长时间挂起
            };
            tokio::select! {
                // 顺序即优先级：stop 最先。
                _ = stop.cancelled() => {
                    log::info!("Stop signal received, hanging up and exiting");
                    shell.stopping = true;
                    queue.clear();
                    queue.push_back(Event::Stop);
                }
                maybe_cmd = cmd_rx.recv() => match maybe_cmd {
                    Some(cmd) => queue.push_back(Event::Command(cmd)),
                    // IPC server 已退出（随 stop）：走停止握手防 busy-loop。
                    None => {
                        shell.stopping = true;
                        queue.clear();
                        queue.push_back(Event::Stop);
                    }
                },
                changed = hb_rx.changed(), if hb_alive => match changed {
                    Ok(()) => queue.push_back(Event::Heartbeat(hb_rx.borrow_and_update().clone())),
                    Err(_) => hb_alive = false,
                },
                maybe_event = wl_rx.recv(), if wireless_alive => match maybe_event {
                    Some(event) => queue.push_back(event),
                    None => {
                        // worker 退出/panic：核验 join 结果并让核心降级（事件环 + 默认快照）。
                        wireless_alive = false;
                        if let Some(handle) = worker_handle.take() {
                            match handle.await {
                                Ok(()) => log::info!("Wireless worker task finished"),
                                Err(e) if e.is_panic() => {
                                    log::error!("Wireless worker PANICKED (wireless degraded): {e}")
                                }
                                Err(e) => log::warn!("Wireless worker task join error: {e}"),
                            }
                        }
                        queue.push_back(Event::WirelessDied);
                    }
                },
                _ = sleep(sleep_for) => queue.push_back(Event::Wake),
            }
            shell.drain(&mut queue).await;
        }

        // 停止握手：worker 的 teardown 效果不可取消；等 worker 退出，
        // RouteGuard::drop 是最后兜底。
        let shutdown_effects = std::mem::take(&mut shell.shutdown_effects);
        if shell
            .lane_tx
            .send(LaneMsg::Shutdown(shutdown_effects))
            .await
            .is_err()
        {
            log::warn!("Wireless lane closed before shutdown; relying on RouteGuard drop");
        }
        if let Some(handle) = worker_handle.take() {
            match tokio::time::timeout(WORKER_EXIT_TIMEOUT, handle).await {
                Ok(Ok(())) => log::info!("Wireless worker task finished"),
                Ok(Err(e)) if e.is_panic() => {
                    log::error!("Wireless worker PANICKED (wireless degraded): {e}")
                }
                Ok(Err(e)) => log::warn!("Wireless worker task join error: {e}"),
                Err(_) => log::warn!("Wireless worker did not exit within 5s, continuing shutdown"),
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
pub use win::start_all;
