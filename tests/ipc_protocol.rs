use gdut_net::ipc::protocol::*;
use std::collections::VecDeque;

#[test]
fn state_msg_roundtrip() {
    let snap = StateSnapshot {
        status: SessionStatus::Connected,
        since_unix: Some(1756500000),
        ip: Some("10.30.132.167".into()),
        last_drop_reason: None,
        redial_attempts: 0,
        heartbeat: HeartbeatStatus::Off,
        mode: NetMode::WiredExclusive,
        wireless: WirelessSnapshot::default(),
        events: VecDeque::new(),
    };
    let bytes = encode_frame(&ServerMsg::State {
        state: snap.clone(),
    });
    assert!(bytes.ends_with(b"\n"));
    let mut dec = FrameDecoder::default();
    let frames = dec.feed(&bytes);
    assert_eq!(frames.len(), 1);
    let msg: ServerMsg = serde_json::from_slice(&frames[0]).unwrap();
    assert_eq!(msg, ServerMsg::State { state: snap });
}

#[test]
fn split_partial_frames() {
    let a = encode_frame(&ClientMsg::Cmd { c: Command::Redial });
    let b = encode_frame(&ClientMsg::Cmd { c: Command::Redial });
    let mut dec = FrameDecoder::default();
    let mut all = a.clone();
    all.extend_from_slice(&b[..b.len() - 1]);
    assert_eq!(dec.feed(&all).len(), 1);
    assert_eq!(dec.feed(&b[b.len() - 1..]).len(), 1);
}

#[test]
fn heartbeat_error_status_serializes() {
    let snap = StateSnapshot {
        status: SessionStatus::Connected,
        since_unix: None,
        ip: None,
        last_drop_reason: None,
        redial_attempts: 0,
        heartbeat: HeartbeatStatus::Error("bind 61440 被占用".into()),
        mode: NetMode::WiredExclusive,
        wireless: WirelessSnapshot::default(),
        events: VecDeque::new(),
    };
    let bytes = encode_frame(&ServerMsg::State { state: snap });
    assert!(String::from_utf8_lossy(&bytes).contains("bind 61440"));
}

#[test]
fn format_uptime_segments() {
    assert_eq!(format_uptime(0), "0:00:00");
    assert_eq!(format_uptime(59), "0:00:59");
    assert_eq!(format_uptime(60), "0:01:00");
    assert_eq!(format_uptime(3661), "1:01:01");
    assert_eq!(format_uptime(360_000), "100:00:00");
}

#[test]
fn snapshot_texts() {
    let mut snap = StateSnapshot {
        status: SessionStatus::Connected,
        since_unix: None,
        ip: Some("10.30.1.2".into()),
        last_drop_reason: None,
        redial_attempts: 0,
        heartbeat: HeartbeatStatus::Off,
        mode: NetMode::WiredExclusive,
        wireless: WirelessSnapshot::default(),
        events: VecDeque::new(),
    };
    assert_eq!(snap.status_text(), "Connected");
    assert_eq!(snap.uptime_text(), "—");
    assert_eq!(snap.heartbeat_text(), "Off");

    snap.heartbeat = HeartbeatStatus::Error("seed 校验失败".into());
    assert_eq!(snap.heartbeat_text(), "Error (seed 校验失败)");

    snap.status = SessionStatus::Backoff;
    assert_eq!(snap.status_text(), "Backoff (retrying)");
}

#[test]
fn set_mode_command_serializes() {
    let bytes = encode_frame(&ClientMsg::Cmd {
        c: Command::SetMode {
            mode: NetMode::WiredPlusStandby,
        },
    });
    let msg: ClientMsg = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        msg,
        ClientMsg::Cmd {
            c: Command::SetMode {
                mode: NetMode::WiredPlusStandby
            }
        }
    );
}

#[test]
fn old_snapshot_json_parses_into_new_struct() {
    // 旧版服务发出的快照（无 mode/wireless/events 字段）必须能被新托盘解析。
    let legacy = br#"{"status":"connected","since_unix":1756500000,"ip":"10.30.1.2","last_drop_reason":null,"redial_attempts":0,"heartbeat":"off"}"#;
    let snap: StateSnapshot = serde_json::from_slice(legacy).unwrap();
    assert_eq!(snap.mode, NetMode::WiredExclusive);
    assert_eq!(snap.wireless.phase, WPhase::Off);
    assert!(snap.events.is_empty());
}

#[test]
fn event_log_caps_and_formats() {
    let mut log = EventLog::new();
    for i in 0..25 {
        log.push(1_757_000_000 + i, &format!("event {i}"));
    }
    assert_eq!(log.ring().len(), 20);
    assert!(log.ring().back().unwrap().ends_with("event 24"));
    assert!(log.ring().front().unwrap().contains("event 5"));
    assert!(log.ring().front().unwrap().starts_with('[')); // "[HH:MM:SS] ..."
}
