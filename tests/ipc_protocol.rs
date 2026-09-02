use gdut_net::ipc::protocol::*;

#[test]
fn state_msg_roundtrip() {
    let snap = StateSnapshot {
        status: SessionStatus::Connected,
        since_unix: Some(1756500000),
        ip: Some("10.30.132.167".into()),
        last_drop_reason: None,
        redial_attempts: 0,
        heartbeat: HeartbeatStatus::Off,
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
    };
    assert_eq!(snap.status_text(), "Connected");
    assert_eq!(snap.uptime_text(), "—");
    assert_eq!(snap.heartbeat_text(), "Off");

    snap.heartbeat = HeartbeatStatus::Error("seed 校验失败".into());
    assert_eq!(snap.heartbeat_text(), "Error (seed 校验失败)");

    snap.status = SessionStatus::Backoff;
    assert_eq!(snap.status_text(), "Backoff (retrying)");
}
