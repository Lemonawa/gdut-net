use gdut_net::crypto::{unwrap_blob, wrap_blob};

#[test]
fn blob_roundtrip() {
    let b = wrap_blob("aabbccdd", &[1, 2, 3, 0xff]);
    assert!(b.starts_with("GDUT1:"));
    let (e, p) = unwrap_blob(&b).unwrap();
    assert_eq!(e, "aabbccdd");
    assert_eq!(p, vec![1, 2, 3, 0xff]);
}

#[test]
fn blob_rejects_garbage() {
    assert!(unwrap_blob("nonsense").is_none());
    assert!(unwrap_blob("GDUT2:x:y").is_none());
}
