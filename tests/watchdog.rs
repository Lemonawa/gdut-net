use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gdut_net::ipc::protocol::SessionStatus;
use gdut_net::probe::ProbeVerdict;
use gdut_net::ras::ErrKind;
use gdut_net::watchdog::*;

#[derive(Default)]
struct MockDialer {
    fail_times: u8,
    dial_calls: Arc<AtomicU32>,
    connected: bool,
}

#[async_trait::async_trait]
impl Dialer for MockDialer {
    async fn dial(&mut self) -> Result<(), DialError> {
        self.dial_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_times > 0 {
            self.fail_times -= 1;
            return Err(DialError {
                kind: ErrKind::Transient,
                code: 678,
                msg: "mock".into(),
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

fn cfg() -> WatchdogCfg {
    WatchdogCfg {
        redial_min: Duration::from_secs(1),
        redial_max: Duration::from_secs(60),
        probe_interval: Duration::from_secs(30),
        auth_fail_delay: Duration::from_secs(600),
    }
}

#[tokio::test]
async fn dials_then_reports_connected() {
    let mut wd = Watchdog::new(
        MockDialer::default(),
        MockProber(vec![ProbeVerdict::Alive]),
        cfg(),
    );
    wd.run_once().await;
    assert_eq!(wd.snapshot().status, SessionStatus::Connected);
}

#[tokio::test]
async fn transient_fail_enters_backoff() {
    let d = MockDialer {
        fail_times: 2,
        ..Default::default()
    };
    let mut wd = Watchdog::new(d, MockProber(vec![ProbeVerdict::Alive]), cfg());
    wd.run_once().await;
    assert_eq!(wd.snapshot().status, SessionStatus::Backoff);
    assert_eq!(wd.snapshot().redial_attempts, 1);
}

#[tokio::test]
async fn double_probe_fail_drops() {
    let mut wd = Watchdog::new(
        MockDialer::default(),
        MockProber(vec![ProbeVerdict::LinkDown, ProbeVerdict::LinkDown]),
        cfg(),
    );
    wd.run_once().await; // dial → Connected
    wd.run_once().await; // probe fail #1（复核，不 drop）
    assert_eq!(wd.snapshot().status, SessionStatus::Connected);
    wd.run_once().await; // probe fail #2 → drop → redial
    assert_eq!(wd.dial_calls(), 2);
}

#[tokio::test]
async fn auth_fail_enters_slow_path() {
    struct AuthDialer;
    #[async_trait::async_trait]
    impl Dialer for AuthDialer {
        async fn dial(&mut self) -> Result<(), DialError> {
            Err(DialError {
                kind: ErrKind::Auth,
                code: 691,
                msg: "denied".into(),
            })
        }
        async fn hangup(&mut self) {}
        fn is_connected(&mut self) -> bool {
            false
        }
    }
    let mut wd = Watchdog::new(AuthDialer, MockProber(vec![ProbeVerdict::Alive]), cfg());
    let d = wd.run_once().await;
    assert_eq!(wd.snapshot().status, SessionStatus::AuthFail);
    assert_eq!(d, Duration::from_secs(600));
}

#[tokio::test]
async fn request_redial_dials_immediately_in_connected() {
    let dialer = MockDialer::default();
    let mut wd = Watchdog::new(dialer, MockProber(vec![ProbeVerdict::Alive]), cfg());
    wd.run_once().await; // dial → Connected
    assert_eq!(wd.dial_calls(), 1);

    wd.request_redial();
    wd.run_once().await; // 顶部消费标志 → hangup（会话仍活）→ do_dial
    assert_eq!(wd.dial_calls(), 2);
    assert_eq!(wd.snapshot().status, SessionStatus::Connected);
    // 标志一次性：再 run_once 回到探测相位，不再拨号
    wd.run_once().await;
    assert_eq!(wd.dial_calls(), 2);
}

#[tokio::test]
async fn request_redial_in_connected_hangups_live_session_first() {
    let dialer = MockDialer::default();
    let mut wd = Watchdog::new(dialer, MockProber(vec![ProbeVerdict::Alive]), cfg());
    wd.run_once().await; // dial → Connected（会话活）
    wd.request_redial();
    wd.run_once().await;
    // 语义修正：活会话必须先挂断再拨，否则二次 RasDial 失败入 Backoff 死循环
    assert_eq!(wd.snapshot().status, SessionStatus::Connected);
    assert_eq!(wd.snapshot().last_drop_reason.as_deref(), Some("手动重拨"));
    // do_dial 成功后 since 重置为新会话起点（record_drop 清理 → 成功再赋值）
    assert!(wd.snapshot().since_unix.is_some());
}

#[tokio::test]
async fn request_redial_in_backoff_preserves_attempts() {
    let d = MockDialer {
        fail_times: 1,
        ..Default::default()
    };
    let mut wd = Watchdog::new(d, MockProber(vec![ProbeVerdict::Alive]), cfg());
    wd.run_once().await; // 失败 → Backoff, attempts=1
    assert_eq!(wd.snapshot().status, SessionStatus::Backoff);

    wd.request_redial();
    wd.run_once().await; // 直接 do_dial 成功 → Connected（无活会话，不 hangup 路径）
    assert_eq!(wd.snapshot().status, SessionStatus::Connected);
    // attempts 不因手动重拨清零（稳定 ≥300s 才重置）
    assert_eq!(wd.snapshot().redial_attempts, 1);
}

#[tokio::test]
async fn request_redial_without_flag_keeps_probing() {
    let dialer = MockDialer::default();
    let mut wd = Watchdog::new(dialer, MockProber(vec![ProbeVerdict::Alive]), cfg());
    wd.run_once().await; // dial → Connected
    assert_eq!(wd.dial_calls(), 1);
    wd.run_once().await; // 正常探测，不再拨号
    assert_eq!(wd.dial_calls(), 1);
}
