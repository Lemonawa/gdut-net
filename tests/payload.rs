use gdut_net::payload::{pack, unpack, validate_name, Entry, FOOTER_LEN, MAGIC};

fn entries() -> Vec<(String, Vec<u8>)> {
    vec![
        ("gdut-net.exe".to_string(), b"MZ fake exe".to_vec()),
        ("说明.txt".to_string(), "中文说明".as_bytes().to_vec()),
        ("status.bat".to_string(), b"@echo off\r\n".to_vec()),
    ]
}

#[test]
fn round_trip_keeps_setup_prefix_and_data() {
    let setup = b"MZ this is the setup exe".to_vec();
    let packed = pack(&setup, &entries()).unwrap();
    assert_eq!(&packed[..setup.len()], &setup[..]);
    assert!(packed.len() > setup.len() + FOOTER_LEN);
    let got = unpack(&packed).unwrap().expect("payload present");
    assert_eq!(got.len(), 3);
    assert_eq!(got[0].data, b"MZ fake exe");
    assert_eq!(got[1].name, "说明.txt");
    assert_eq!(got[2].data, b"@echo off\r\n");
}

#[test]
fn unpack_without_magic_returns_none() {
    assert!(unpack(b"just a plain exe").unwrap().is_none());
    assert!(unpack(&[0u8; 8]).unwrap().is_none());
}

#[test]
fn corrupted_data_fails_checksum() {
    let setup = b"MZ setup".to_vec();
    let mut packed = pack(&setup, &entries()).unwrap();
    let victim = setup.len(); // first data byte of entry 0
    packed[victim] ^= 0xff;
    let err = unpack(&packed).unwrap_err();
    assert!(format!("{err:#}").contains("checksum"), "got: {err:#}");
}

#[test]
fn magic_in_footer_but_truncated_toc_is_error() {
    let setup = b"MZ setup".to_vec();
    let mut packed = pack(&setup, &entries()).unwrap();
    packed.truncate(setup.len() + 4); // magic bytes survive? no: crop from end
                                      // rebuild: keep magic at end manually
    let mut fake = setup.clone();
    fake.extend_from_slice(MAGIC);
    fake.extend_from_slice(&[0u8; FOOTER_LEN - 8]);
    assert!(unpack(&fake).is_err());
    let _ = packed;
}

#[test]
fn validate_name_rejects_separators_and_dotdot() {
    for bad in ["", "a/b", "a\\b", "..", "../x", "C:evil"] {
        assert!(validate_name(bad).is_err(), "accepted {bad:?}");
    }
    for good in ["status.bat", "说明.txt", "a-b_c.d"] {
        assert!(validate_name(good).is_ok(), "rejected {good:?}");
    }
}

#[test]
fn duplicate_names_rejected() {
    let e = vec![
        ("a.txt".to_string(), vec![1]),
        ("a.txt".to_string(), vec![2]),
    ];
    assert!(pack(b"MZ", &e).is_err());
}

#[test]
fn empty_entries_are_allowed() {
    let packed = pack(b"MZ setup bytes", &[]).unwrap();
    let got = unpack(&packed).unwrap().expect("payload present");
    assert!(got.is_empty());
    let e: Vec<Entry> = got;
    assert!(e.is_empty());
}
