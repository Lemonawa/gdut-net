use gdut_net::win32::wide;

#[test]
fn wide_appends_nul() {
    assert_eq!(wide(""), vec![0]);
    assert_eq!(wide("A"), vec![0x41, 0]);
    assert_eq!(wide("中"), vec![0x4E2D, 0]);
    assert_eq!(wide("ok").last(), Some(&0));
}
