use gdut_net::heartbeat::spec::*;

#[test]
fn ka1_pkt1_layout() {
    assert_eq!(ka1_pkt1(1), [0x07, 1, 0x08, 0x00, 0x01, 0, 0, 0]);
}

#[test]
fn parse_ka1_resp_issue82_capture() {
    // issue #82 真实抓包（file packet 形态）
    let pkt = [
        0x07u8, 0x6f, 0x10, 0x00, 0x02, 0x03, 0x00, 0x00, 0xa3, 0xe2, 0xf3, 0x00, 0x0a, 0x1e, 0x84,
        0xa7, 0xa8, 0xa8, 0x00, 0x00, 0xe6, 0x59, 0xf1, 0x67, 0x00, 0x00, 0x00, 0x00, 0xdc, 0x02,
    ];
    let init = parse_ka1_resp(&pkt).unwrap();
    assert_eq!(init.seed, [0xa3, 0xe2, 0xf3, 0x00]);
    assert_eq!(init.host_ip, [0x0a, 0x1e, 0x84, 0xa7]);
    assert_eq!(init.flag, Some([0x00, 0x00]));
}

#[test]
fn ka1_pkt2_checksum_sha1_mode_issue82() {
    // seed=a3e2f300 → 0xa3&3=3 → SHA1；抓包校验值 9ae9cef84b020aa3
    let pkt = ka1_pkt2(
        1,
        true,
        [0x0a, 0x1e, 0x84, 0xa7],
        [0xa3, 0xe2, 0xf3, 0x00],
        [0x2a, 0x00],
    );
    assert_eq!(&pkt[0..5], &[0x07, 1, 0x60, 0x00, 0x03]);
    assert_eq!(&pkt[17..18], &[0x62]);
    assert_eq!(&pkt[20..24], &[0xa3, 0xe2, 0xf3, 0x00]);
    assert_eq!(
        &pkt[24..32],
        &[0x9a, 0xe9, 0xce, 0xf8, 0x4b, 0x02, 0x0a, 0xa3]
    );
}

#[test]
fn crypt_mode_selection() {
    assert_eq!(crypt_bytes(&[0x00, 0, 0, 0]), plain_bytes());
    assert_eq!(crypt_bytes(&[0x01, 0, 0, 0]), md5_bytes(&[0x01, 0, 0, 0]));
    assert_eq!(crypt_bytes(&[0x02, 0, 0, 0]), md4_bytes(&[0x02, 0, 0, 0]));
    assert_eq!(crypt_bytes(&[0xa3, 0, 0, 0]), sha1_bytes(&[0xa3, 0, 0, 0]));
}

#[test]
fn ka2_pkt2_layout_and_crc() {
    let pkt = ka2_pkt2(
        3,
        [0xdc, 0x02],
        0x03e9,
        [0x43, 0xe1, 0xf3, 0x00],
        [0x0a, 0x1e, 0x84, 0xa7],
    );
    assert_eq!(pkt[0], 0x07);
    assert_eq!(pkt[1], 3);
    assert_eq!(&pkt[2..4], &[0x28, 0x00]);
    assert_eq!(&pkt[4..6], &[0x0b, 0x03]);
    assert_eq!(&pkt[6..8], &[0xdc, 0x02]);
    assert_eq!(&pkt[8..10], &[0x03, 0xe9]);
    assert_eq!(&pkt[16..20], &[0x43, 0xe1, 0xf3, 0x00]);
    // CRC 自洽：清零校验位重算一致
    let mut p2 = pkt;
    p2[24..28].fill(0);
    assert_eq!(&pkt[24..28], &ka2_checksum(&p2));
}

#[test]
fn ka2_pkt1_layout() {
    let pkt = ka2_pkt1(0, [0x00, 0x00], 0x1234, [0; 4]);
    assert_eq!(&pkt[0..2], &[0x07, 0x00]);
    assert_eq!(&pkt[4..6], &[0x0b, 0x01]);
    assert_eq!(&pkt[8..10], &[0x12, 0x34]);
    assert!(pkt[16..20].iter().all(|&b| b == 0));
}

#[test]
fn file_packet_flag_learning() {
    let mut pkt = [0u8; 40];
    pkt[0] = 0x07;
    pkt[2] = 0x10;
    pkt[6] = 0xdc;
    pkt[7] = 0x02;
    assert_eq!(parse_ka2_resp(&pkt).unwrap().flag, Some([0xdc, 0x02]));
}

#[test]
fn cnt_wraps_below_128() {
    assert_eq!(next_cnt(125), 127);
    assert_eq!(next_cnt(127), 1);
}
