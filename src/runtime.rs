//! 服务运行时装配：把 ras/adapter/probe/watchdog/heartbeat 接成可运行的主体。
//!
//! `start_all` 由 service_main（服务线程，非 main）调用：内部自建多线程
//! tokio runtime 并 block_on。装配链路：
//!
//! ```text
//! Config ──► crypto::unprotect(密码)
//!        ├─► RealDialer（ras::dial / RasSession）─┐
//!        ├─► RealProber（adapter 刷新 + probe_once）─┤─► Watchdog.run_once 循环
//!        ├─► watch::<StateSnapshot>（IPC status/托盘消费）
//!        ├─► wireless manager（wireless.enabled 时 spawn）：Brain 决策循环，
//!            执行 WLAN 关联 / eportal 认证 / 探测 / 路由与 metric（ADR-0005）
//!        └─► heartbeat::session::run_blocking（spawn_blocking 内运行，
//!            enabled 时；装配失败 60s 循环重试）
//! stop.cancelled() ─► 退出循环 → hangup
//! ```
//!
//! 仅 Windows 编译，以 `cargo check --target x86_64-pc-windows-msvc` 验证。

#[cfg(windows)]
mod win {
    use std::collections::HashMap;
    use std::net::Ipv4Addr;
    use std::path::PathBuf;
    use std::time::Duration;
    use std::time::Instant;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    use anyhow::{anyhow, bail, Context, Result};
    use tokio::sync::{mpsc, watch};
    use tokio::time::sleep;
    use tokio_util::sync::CancellationToken;

    use crate::adapter::{self, AdapterInfo};
    use crate::backoff::AUTH_FAIL_DELAY;
    use crate::config::Config;
    use crate::heartbeat::session;
    use crate::ipc::protocol::{
        Command, EventLog, HeartbeatStatus, NetMode, SessionStatus, StateSnapshot, WPhase,
        WirelessSnapshot,
    };
    use crate::ipc::server;
    use crate::probe::{probe_once, ProbeVerdict};
    use crate::watchdog::{DialError, Dialer, Prober, Watchdog, WatchdogCfg};
    use crate::wireless::portal::{self, PortalResult};
    use crate::wireless::routes;
    use crate::wireless::wlan;
    use crate::wireless::{Action, Brain, World, JOIN_TIMEOUT_SECS};
    use crate::{crypto, notify, ras};

    /// 心跳装配失败后的重试间隔。
    const HEARTBEAT_RETRY_DELAY: Duration = Duration::from_secs(60);
    /// 连续重拨失败多久后弹 Toast（未恢复即持续失败）。
    const REDIAL_FAILING_TOAST_AFTER: Duration = Duration::from_secs(600);
    /// 同一原因 Toast 的最小间隔。
    const NOTIFY_THROTTLE: Duration = Duration::from_secs(30 * 60);
    /// 单次 eportal 认证墙钟上限：portal_get 内部虽有 3s 级超时，但整体
    /// 兜底防挂死会话把 Brain 卡在 Authing（T4 review carry-forward）。
    const PORTAL_AUTH_TIMEOUT: Duration = Duration::from_secs(10);

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
    }

    impl RealDialer {
        fn new(cfg: &Config, pass: String) -> Self {
            Self {
                pbk: cfg.dial.pbk_path.clone(),
                entry: cfg.dial.entry_name.clone(),
                user: cfg.account.student_id.clone(),
                pass,
                session: None,
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
                    self.session = Some(s);
                    Ok(())
                }
                Err(e) => Err(e),
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
                Ok(a) => probe_once(a.ipv4, a.gateway, &self.http_url).await,
                Err(e) => {
                    log::warn!("Failed to resolve adapter before probe: {e:#}");
                    crate::probe::ProbeVerdict::LinkDown
                }
            }
        }
    }

    /// Toast 节流器：同一原因 30 分钟内不重复。
    struct Notifier {
        last: HashMap<String, Instant>,
    }

    impl Notifier {
        fn new() -> Self {
            Self {
                last: HashMap::new(),
            }
        }

        /// 节流通过则弹 Toast，成功才记录节流 Instant（失败不占用节流窗口，
        /// 下次仍会尝试）；toast 失败只记日志（通知不可用不中断服务）。
        fn fire(&mut self, key: &str, title: &str, body: &str) {
            if let Some(t) = self.last.get(key) {
                if t.elapsed() < NOTIFY_THROTTLE {
                    return;
                }
            }
            log::info!("Toast [{key}]: {title} — {body}");
            match notify::toast(title, body) {
                Ok(()) => {
                    self.last.insert(key.to_string(), Instant::now());
                }
                Err(e) => {
                    log::warn!("Toast failed (ignored): {e:#}");
                }
            }
        }
    }

    /// 当前 UNIX 秒（事件环时间戳；时钟回拨/溢出按 0 兜底）。
    fn unix_now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// 模式英文短名（事件环与日志用；完整描述见 StateSnapshot::mode_text）。
    fn mode_text(m: NetMode) -> &'static str {
        match m {
            NetMode::WiredExclusive => "wired-exclusive",
            NetMode::WiredPlusStandby => "wired-plus-standby",
        }
    }

    /// wireless manager 专属配置快照（主循环持 Config 原件做持久化）。
    struct ManagerCfg {
        profile: String,
        portal_url: String,
        wlan_ac_ip: String,
        probe_host: String,
        user: String,
        pass: String,
        takeover_after: u64,
        release_after: u64,
        probe_interval: u64,
        standby_metric: u32,
    }

    /// 无线接管执行体（ADR-0005）：2s 一拍采集 World → Brain 决策 → 执行动作。
    /// 决策正确性由 Task 4 的 Brain 纯逻辑测试背书；本函数只做采信执行。
    async fn wireless_manager(
        cfg: ManagerCfg,
        stop: CancellationToken,
        mut mode_rx: watch::Receiver<NetMode>,
        mut wired_rx: watch::Receiver<bool>,
        wl_tx: watch::Sender<WirelessSnapshot>,
        ev_tx: mpsc::Sender<String>,
    ) {
        let start = Instant::now();
        let now = || start.elapsed().as_secs();
        // config::validate 已保证二者可解析；此处仍容错返回（配置可被手改）。
        let portal_ip = crate::probe::parse_http_probe_target(&cfg.portal_url).map(|(ip, _)| ip);
        let probe_ip: Option<Ipv4Addr> = cfg.probe_host.parse().ok();
        let (portal_ip, probe_ip) = match (portal_ip, probe_ip) {
            (Some(p), Some(q)) => (p, q),
            _ => {
                log::error!("Wireless: portal_url/probe_host invalid, manager disabled");
                return;
            }
        };
        routes::cleanup_stale(&[portal_ip, probe_ip]);
        let mut brain = Brain::new(
            *mode_rx.borrow_and_update(),
            cfg.takeover_after,
            cfg.release_after,
            cfg.probe_interval,
        );
        let mut guard = routes::RouteGuard::new();
        let mut join_since: Option<u64> = None;
        let mut verdict: Option<ProbeVerdict> = None;
        let ev = |tx: &mpsc::Sender<String>, msg: &str| {
            let _ = tx.try_send(msg.to_string());
        };
        loop {
            if stop.is_cancelled() {
                break;
            }
            brain.set_mode(*mode_rx.borrow_and_update());
            let (eth_up, assoc, wlan) = tokio::task::spawn_blocking(|| {
                (
                    adapter::ethernet_link_up(),
                    wlan::associated(),
                    adapter::wlan_adapter(),
                )
            })
            .await
            .unwrap_or((false, false, None));
            let world = World {
                now: now(),
                eth_link_up: eth_up,
                wired_connected: *wired_rx.borrow_and_update(),
                wlan_associated: assoc,
                wlan_ip: wlan.is_some(),
                probe: verdict,
            };
            match brain.decide(&world) {
                Action::None => {}
                Action::Associate => {
                    ev(&ev_tx, "Wireless: associating to campus SSID");
                    let p = cfg.profile.clone();
                    if let Err(e) = tokio::task::spawn_blocking(move || wlan::associate(&p))
                        .await
                        .unwrap_or_else(|_| Err(anyhow!("join task panicked")))
                    {
                        log::warn!("Wireless associate failed (will retry): {e:#}");
                    }
                    join_since = Some(now());
                }
                Action::Disassociate => {
                    ev(&ev_tx, "Wireless: releasing (wired healthy)");
                    guard.teardown();
                    let _ = tokio::task::spawn_blocking(wlan::disassociate).await;
                    verdict = None;
                }
                Action::PortalAuth => {
                    let Some(a) = wlan.as_ref() else {
                        brain.on_auth(false, "wlan ip lost", now());
                        // 认证尝试已使缓存探测结论失效，须清：Online 相的
                        // Kicked 检查先于探测定时器，残留 Kicked 会把
                        // manager 钉进无限重认证循环（review Critical）。
                        verdict = None;
                        continue;
                    };
                    let Some(gw) = a.gateway else {
                        brain.on_auth(false, "wlan gateway missing", now());
                        verdict = None;
                        continue;
                    };
                    guard.ensure(&[portal_ip, probe_ip], gw, a.ifindex);
                    let url = portal::build_login_url(
                        &cfg.portal_url,
                        &cfg.user,
                        &cfg.pass,
                        a.ipv4,
                        &cfg.wlan_ac_ip,
                    );
                    // 日志只落脱敏 URL（内含明文密码）。
                    log::info!(
                        "Wireless: portal login from {} ({})",
                        a.ipv4,
                        portal::redact_query(&url)
                    );
                    // 挂死会话兜底：portal_get 卡死不能把 Brain 永久钉在
                    // Authing——超时按认证失败回报（T4 carry-forward）。
                    let r =
                        tokio::time::timeout(PORTAL_AUTH_TIMEOUT, portal::portal_get(a.ipv4, &url))
                            .await;
                    let (ok, msg) = match r {
                        Ok(Some((code, body))) => match portal::parse_portal_reply(&body) {
                            PortalResult::Success => (true, "login ok".to_string()),
                            PortalResult::Failure(m) => (false, m),
                            PortalResult::Malformed => {
                                (false, format!("unparseable reply (HTTP {code})"))
                            }
                        },
                        Ok(None) => (false, "no reply".to_string()),
                        Err(_) => (false, "portal auth timeout".to_string()),
                    };
                    if ok {
                        ev(&ev_tx, "Wireless: portal login success");
                    } else {
                        ev(&ev_tx, &format!("Wireless: portal login failed: {msg}"));
                    }
                    brain.on_auth(ok, &msg, now());
                    // 认证成功后 Brain 的探测定时器已复位（last_probe_at=None
                    // → 下一拍 ProbeNow），此处同步清缓存结论，让下一拍用
                    // 新鲜探测数据而非过期 Kicked 再触发一次 PortalAuth。
                    verdict = None;
                }
                Action::ProbeNow => {
                    let Some(a) = wlan.as_ref() else {
                        verdict = Some(ProbeVerdict::LinkDown);
                        continue;
                    };
                    let url = format!("http://{}/", cfg.probe_host);
                    let v = probe_once(a.ipv4, a.gateway, &url).await;
                    if v != ProbeVerdict::Alive {
                        ev(&ev_tx, &format!("Wireless probe: {v:?}"));
                    }
                    verdict = Some(v);
                }
            }
            // standby 模式幂等压制 WLAN metric；exclusive 模式还原（让位瞬间
            // 解除压制，路由去留由后续 ensure/teardown 决定）。
            let mode = *mode_rx.borrow_and_update();
            if let Some(a) = &wlan {
                if mode == NetMode::WiredPlusStandby {
                    guard.set_standby_metric(a.ifindex, cfg.standby_metric);
                }
            }
            if mode == NetMode::WiredExclusive {
                guard.release_metric();
            }
            // Joining 超时 → 复位重关联（IP/DHCP 迟迟不来的场景）。
            if brain.phase() == WPhase::Joining
                && join_since.is_some_and(|t| now() - t > JOIN_TIMEOUT_SECS)
            {
                ev(&ev_tx, "Wireless: join timeout, restarting");
                brain.restart();
                join_since = None;
            }
            let mut snap = brain.snapshot();
            snap.ip = wlan.map(|a| a.ipv4.to_string());
            let _ = wl_tx.send(snap);
            tokio::select! {
                _ = stop.cancelled() => break,
                _ = sleep(Duration::from_secs(2)) => {}
            }
        }
        // 退出收尾：删路由 + 还原 metric + 断开 WLAN（自包含铁律的第三出口）。
        guard.teardown();
        let _ = tokio::task::spawn_blocking(wlan::disassociate).await;
        let _ = wl_tx.send(WirelessSnapshot::default());
        ev(&ev_tx, "Wireless: manager stopped");
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

    async fn run(mut cfg: Config, cfg_path: PathBuf, stop: CancellationToken) -> Result<()> {
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
        let mut watchdog = Watchdog::new(dialer, prober, watchdog_cfg);

        // 状态快照通道 + IPC server：客户端（status/托盘）连上来即见当前状态。
        let (snap_tx, snap_rx) = watch::channel(watchdog.snapshot());
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Command>(16);
        server::spawn_server(snap_rx, cmd_tx, stop.clone());

        // 无线接管装配（ADR-0005）：模式广播 + 有线状态广播 + manager actor。
        // mode/事件由主循环在 SetMode/ev_rx 消费时更新；wired 由 run_once 后回填。
        let init_mode = cfg.wireless.mode;
        let (mode_tx, mode_rx) = watch::channel(init_mode);
        let (wired_tx, wired_rx) = watch::channel(false);
        let (wl_tx, mut wl_rx) = watch::channel(WirelessSnapshot::default());
        let (ev_tx, mut ev_rx) = mpsc::channel::<String>(64);
        let mut events = EventLog::new();
        events.push(
            unix_now(),
            &format!("Service started, mode {}", mode_text(init_mode)),
        );
        if cfg.wireless.enabled {
            let mcfg = ManagerCfg {
                profile: cfg.wireless.profile.clone(),
                portal_url: cfg.wireless.portal_url.clone(),
                wlan_ac_ip: cfg.wireless.wlan_ac_ip.clone(),
                probe_host: cfg.wireless.probe_host.clone(),
                user: cfg.account.student_id.clone(),
                pass: pass.clone(),
                takeover_after: cfg.wireless.takeover_after_secs,
                release_after: cfg.wireless.release_after_secs,
                probe_interval: cfg.dial.probe_interval_secs,
                standby_metric: cfg.wireless.standby_metric,
            };
            let mstop = stop.child_token();
            let ev_tx = ev_tx.clone();
            let m_wl_tx = wl_tx.clone();
            tokio::spawn(async move {
                wireless_manager(mcfg, mstop, mode_rx, wired_rx, m_wl_tx, ev_tx).await;
            });
        }

        // 快照组装：watchdog 原始快照 + PPP IP + 心跳/模式/无线/事件环富化。
        // events 经参数传入（闭包持不可变借用会与主循环的 events.push 冲突）。
        let compose = |mut snap: StateSnapshot, events: &EventLog| {
            snap.ip = adapter::ppp_adapter_ip().map(|ip| ip.to_string());
            snap.heartbeat = hb_tx.borrow().clone();
            snap.mode = *mode_tx.borrow();
            snap.wireless = wl_tx.borrow().clone();
            snap.events = events.ring().clone();
            snap
        };

        // 通知钩子状态：重拨连续失败起点 + 节流器。
        let mut notifier = Notifier::new();
        let mut failing_since: Option<Instant> = None;
        // run_once 建议的下一轮等待时长（首轮立即执行）。
        let mut next_delay = Duration::ZERO;

        loop {
            tokio::select! {
                _ = stop.cancelled() => break,
                // 外层 select 只轮询事件，绝不含 run_once——在飞的 run_once
                // 由循环体内独占 await 执行，任何命令都无法打断/drop 它
                // （结构保证，不依赖守卫 bool）。
                maybe_cmd = cmd_rx.recv() => {
                    match maybe_cmd {
                        Some(Command::Redial) => {
                            log::info!("IPC command: manual redial");
                            // 置一次性标志即返回（下一轮 run_once 顶部立即
                            // 消费）；当前 sleep 已被本分支胜出打断，不会
                            // 睡满退避时长。
                            watchdog.request_redial();
                        }
                        Some(Command::SetMode { mode }) => {
                            log::info!("IPC command: set mode {}", mode_text(mode));
                            // 最新胜（watch replace 语义）+ 落盘（失败仅警告：
                            // 模式切换不因磁盘问题被拒）。
                            let _ = mode_tx.send(mode);
                            cfg.wireless.mode = mode;
                            if let Err(e) = cfg.save(&cfg_path) {
                                log::warn!("Failed to persist mode to config (ignored): {e:#}");
                            }
                            events.push(
                                unix_now(),
                                &format!("Mode switched to {}", mode_text(mode)),
                            );
                            let _ = snap_tx.send(compose(watchdog.snapshot(), &events));
                        }
                        // IPC server 已退出（随 stop）：break 防 busy-loop。
                        None => break,
                    }
                }
                _ = sleep(next_delay) => {}
                // 钩子：心跳报 Error → Toast（节流）+ 立即推快照
                // （长 sleep 间隔下 hb 状态变化须即时可见）。
                hb_changed = hb_rx.changed() => {
                    if hb_changed.is_ok() {
                        if let HeartbeatStatus::Error(e) = hb_rx.borrow().clone() {
                            notifier.fire(
                                "heartbeat_error",
                                "gdut-net heartbeat error",
                                &format!("Compatibility heartbeat error: {e}"),
                            );
                            let _ = snap_tx.send(compose(watchdog.snapshot(), &events));
                        }
                    }
                }
                // 无线快照变化（manager 每拍/退出时推）→ 重推富化快照。
                wl_changed = wl_rx.changed() => {
                    if wl_changed.is_ok() {
                        let _ = snap_tx.send(compose(watchdog.snapshot(), &events));
                    }
                }
                // manager 事件（关联/认证/释放/超时）→ 事件环 + 重推快照。
                ev = ev_rx.recv() => {
                    if let Some(msg) = ev {
                        events.push(unix_now(), &msg);
                        let _ = snap_tx.send(compose(watchdog.snapshot(), &events));
                    }
                }
            }

            // 状态机单步：独占 await（拨号 spawn_blocking 期间不受命令
            // 分支影响），完成后再轮询事件。
            let d = watchdog.run_once().await;

            // 有线会话状态广播（manager 的 takeover/release 判定输入）。
            let _ = wired_tx.send(matches!(
                watchdog.snapshot().status,
                SessionStatus::Connected
            ));

            let snap = compose(watchdog.snapshot(), &events);
            let _ = snap_tx.send(snap.clone());

            // 钩子：连续重拨失败累计 ≥10 分钟未恢复 → Toast。
            if snap.status == SessionStatus::Backoff || snap.status == SessionStatus::AuthFail {
                let since = *failing_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= REDIAL_FAILING_TOAST_AFTER {
                    notifier.fire(
                        "redial_failing",
                        "gdut-net network error",
                        &format!(
                            "Redial failed for {} minutes ({} attempts), check network or credentials",
                            since.elapsed().as_secs() / 60,
                            snap.redial_attempts
                        ),
                    );
                }
            } else if snap.status == SessionStatus::Connected && failing_since.take().is_some() {
                log::info!("Redial succeeded, session restored");
            }
            next_delay = d;
        }

        log::info!("Stop signal received, hanging up and exiting");
        watchdog.shutdown().await;
        Ok(())
    }
}

#[cfg(windows)]
pub use win::start_all;
