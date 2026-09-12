use gdut_net::ipc::protocol::{
    HeartbeatStatus, NetMode, SessionStatus, StateSnapshot, WPhase, WirelessSnapshot,
};
use gdut_net::status::{session_en, status_line_zh, view, Light, Primary};

fn snap() -> StateSnapshot {
    StateSnapshot {
        status: SessionStatus::Connected,
        since_unix: None,
        ip: None,
        last_drop_reason: None,
        redial_attempts: 0,
        heartbeat: HeartbeatStatus::Off,
        mode: NetMode::WiredExclusive,
        wireless: WirelessSnapshot::default(),
        events: Default::default(),
    }
}

#[test]
fn no_snapshot_is_service_down() {
    let v = view(None);
    assert_eq!((v.primary, v.light), (Primary::ServiceDown, Light::Off));
    assert_eq!(v.primary.word_zh(), "服务未运行");
    assert_eq!(v.primary.egress_zh(), "—");
    assert_eq!(status_line_zh(None), "服务未运行");
}

#[test]
fn wired_connected_beats_wireless_online() {
    let mut s = snap();
    s.wireless.phase = WPhase::Online;
    let v = view(Some(&s));
    assert_eq!((v.primary, v.light), (Primary::Connected, Light::Wired));
    assert_eq!(v.primary.word_zh(), "已连接");
    assert_eq!(v.primary.egress_zh(), "有线");
}

#[test]
fn wireless_online_when_wired_not_connected() {
    let mut s = snap();
    s.status = SessionStatus::Backoff;
    s.wireless.phase = WPhase::Online;
    let v = view(Some(&s));
    assert_eq!(
        (v.primary, v.light),
        (Primary::WirelessOnline, Light::Wireless)
    );
    assert_eq!(v.primary.word_zh(), "无线接管");
    assert_eq!(v.primary.egress_zh(), "无线");
}

#[test]
fn in_progress_and_failure_map_to_busy_light() {
    for (st, primary, word) in [
        (SessionStatus::Dialing, Primary::Dialing, "拨号中"),
        (SessionStatus::Backoff, Primary::Backoff, "重拨中"),
        (SessionStatus::AuthFail, Primary::AuthFail, "认证失败"),
    ] {
        let mut s = snap();
        s.status = st;
        let v = view(Some(&s));
        assert_eq!((v.primary, v.light), (primary, Light::Busy));
        assert_eq!(v.primary.word_zh(), word);
        assert_eq!(v.primary.egress_zh(), "—");
    }
    let mut s = snap();
    s.status = SessionStatus::Idle;
    let v = view(Some(&s));
    assert_eq!((v.primary, v.light), (Primary::Idle, Light::Off));
}

#[test]
fn status_line_composes_wired_and_wifi() {
    let mut s = snap();
    assert_eq!(status_line_zh(Some(&s)), "有线：已连接 · WiFi：关闭");
    s.wireless.phase = WPhase::Online;
    assert_eq!(status_line_zh(Some(&s)), "有线：已连接 · WiFi：已接管");
}

#[test]
fn session_en_strings_are_frozen() {
    assert_eq!(session_en(SessionStatus::Idle), "Idle");
    assert_eq!(session_en(SessionStatus::Dialing), "Dialing");
    assert_eq!(session_en(SessionStatus::Connected), "Connected");
    assert_eq!(session_en(SessionStatus::Backoff), "Backoff (retrying)");
    assert_eq!(session_en(SessionStatus::AuthFail), "Auth failed");
}

#[test]
fn wireless_and_heartbeat_en_strings() {
    use gdut_net::status::{heartbeat_en, mode_en, wireless_en};
    let mut w = WirelessSnapshot::default();
    assert_eq!(wireless_en(&w), "Off");
    w.phase = WPhase::Online;
    w.ip = Some("10.0.3.7".into());
    assert_eq!(wireless_en(&w), "Online 10.0.3.7");
    w.ip = None;
    w.phase = WPhase::Error;
    w.last_error = Some("portal timeout".into());
    assert_eq!(wireless_en(&w), "Error (portal timeout)");
    assert_eq!(heartbeat_en(&HeartbeatStatus::Off), "Off");
    assert_eq!(
        heartbeat_en(&HeartbeatStatus::Error("seed 校验失败".into())),
        "Error (seed 校验失败)"
    );
    assert_eq!(
        mode_en(NetMode::WiredExclusive),
        "Wired only (auto wireless takeover)"
    );
    assert_eq!(
        mode_en(NetMode::WiredPlusStandby),
        "Wired + wireless standby"
    );
    assert_eq!(gdut_net::status::wphase_zh(WPhase::Joining), "连接中");
}
