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
    let frames = dec.feed(&bytes).unwrap();
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
    assert_eq!(dec.feed(&all).unwrap().len(), 1);
    assert_eq!(dec.feed(&b[b.len() - 1..]).unwrap().len(), 1);
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

#[test]
fn decoder_accepts_limit_and_resets_for_next_frame() {
    let mut dec = FrameDecoder::default();
    let body = vec![b'x'; MAX_FRAME_BYTES - 1];
    // Byte-sized chunks exercise incremental scanning without rescanning old data.
    for byte in &body {
        assert!(dec.feed(std::slice::from_ref(byte)).unwrap().is_empty());
    }
    let frames = dec.feed(b"\nnext\npartial").unwrap();
    assert_eq!(frames[0].len(), MAX_FRAME_BYTES);
    assert_eq!(frames[1], b"next\n");
    assert_eq!(dec.feed(b"\n").unwrap()[0], b"partial\n");
}

#[test]
fn decoder_rejects_oversized_partial_and_stays_rejected() {
    let mut dec = FrameDecoder::default();
    assert!(dec.feed(&vec![b'x'; MAX_FRAME_BYTES]).unwrap().is_empty());
    assert_eq!(dec.feed(b"x"), Err(FrameTooLarge));
    assert_eq!(dec.feed(b"\n"), Err(FrameTooLarge));
}

#[test]
fn decoder_rejects_oversized_complete_frame() {
    let mut input = vec![b'x'; MAX_FRAME_BYTES];
    input.push(b'\n');
    assert_eq!(FrameDecoder::default().feed(&input), Err(FrameTooLarge));
}
