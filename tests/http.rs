use gdut_net::http::{is_auth_redirect, parse_status_code, parse_url, read_acceptable, ReadPolicy};

#[test]
fn parse_url_splits_host_port_path() {
    let t = parse_url("http://10.0.3.2:801/eportal/portal/login").unwrap();
    assert_eq!(t.host, "10.0.3.2");
    assert_eq!(t.port, 801);
    assert_eq!(t.path, "/eportal/portal/login");
    assert_eq!(t.host_header, "10.0.3.2:801");

    let t = parse_url("http://223.5.5.5").unwrap();
    assert_eq!(
        (
            t.host.as_str(),
            t.port,
            t.path.as_str(),
            t.host_header.as_str()
        ),
        ("223.5.5.5", 80, "/", "223.5.5.5")
    );

    assert!(parse_url("https://10.0.3.2/").is_none());
    assert!(parse_url("http://").is_none());
    assert!(parse_url("http://10.0.3.2:99999/").is_none());
}

#[test]
fn status_line_tolerance() {
    assert_eq!(parse_status_code("HTTP/1.0 302 Found"), Some(302));
    assert_eq!(parse_status_code("HTTP/1.1 200 OK"), Some(200));
    assert_eq!(parse_status_code("HTTP/2 204"), Some(204));
    assert_eq!(parse_status_code("garbage"), None);
    assert_eq!(parse_status_code(""), None);
}

#[test]
fn auth_redirect_keywords() {
    assert!(is_auth_redirect("http://1.1.1.1/wlanacip?x=1"));
    assert!(is_auth_redirect("http://1.1.1.1/nexturl=..."));
    assert!(is_auth_redirect("http://portal.gdut.edu.cn/"));
    assert!(!is_auth_redirect("http://www.example.com/"));
}

#[test]
fn read_policy_decides_partial_reads() {
    // Complete（portal）：任何读错误都算失败，哪怕已有字节。
    assert!(!read_acceptable(ReadPolicy::Complete, false, true));
    assert!(!read_acceptable(ReadPolicy::Complete, false, false));
    assert!(read_acceptable(ReadPolicy::Complete, true, false));
    assert!(read_acceptable(ReadPolicy::Complete, true, true));
    // AcceptPartial（probe）：已有字节时接受读错误（旧 `!read_ok && buf.is_empty()` 语义）。
    assert!(read_acceptable(ReadPolicy::AcceptPartial, false, true));
    assert!(!read_acceptable(ReadPolicy::AcceptPartial, false, false));
    assert!(read_acceptable(ReadPolicy::AcceptPartial, true, false));
    assert!(read_acceptable(ReadPolicy::AcceptPartial, true, true));
}
