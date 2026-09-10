use gdut_net::wireless::portal::*;
use std::net::Ipv4Addr;

#[test]
fn urlencode_keeps_unreserved_and_encodes_specials() {
    assert_eq!(urlencode("abcXYZ019-._~"), "abcXYZ019-._~");
    assert_eq!(urlencode("p@ss+w&d"), "p%40ss%2Bw%26d");
    assert_eq!(urlencode("密码"), "%E5%AF%86%E7%A0%81");
}

#[test]
fn login_url_contains_eportal_fields() {
    let url = build_login_url(
        "http://10.0.3.2:801/eportal/portal/login",
        "3126006414",
        "p@ss",
        Ipv4Addr::new(10, 43, 199, 166),
        "172.16.254.2",
    );
    assert!(url.starts_with("http://10.0.3.2:801/eportal/portal/login?"));
    assert!(url.contains("callback=dr1004"));
    assert!(url.contains("login_method=1"));
    assert!(url.contains("user_account=3126006414"));
    assert!(url.contains("user_password=p%40ss"));
    assert!(url.contains("wlan_user_ip=10.43.199.166"));
    assert!(url.contains("wlan_user_mac=000000000000"));
    assert!(url.contains("wlan_ac_ip=172.16.254.2"));
    assert!(url.contains("jsVersion=4.1.3"));
}

#[test]
fn jsonp_success_and_failure() {
    assert_eq!(
        parse_portal_reply(r#"dr1004({"result":"1","msg":"Login is successful"})"#),
        PortalResult::Success
    );
    assert_eq!(
        parse_portal_reply(r#"dr1004({"result":"0","msg":"E2620: already online"})"#),
        PortalResult::Failure("E2620: already online".into())
    );
}

#[test]
fn jsonp_malformed_and_plain_json() {
    assert!(matches!(
        parse_portal_reply("<html>login page</html>"),
        PortalResult::Malformed
    ));
    // 无 callback 包裹的裸 JSON 也要认（服务器行为兜底）
    assert_eq!(
        parse_portal_reply(r#"{"result":"1"}"#),
        PortalResult::Success
    );
}

#[test]
fn redact_query_strips_credentials() {
    let url = build_login_url(
        "http://10.0.3.2:801/eportal/portal/login",
        "u",
        "secret",
        Ipv4Addr::LOCALHOST,
        "172.16.254.2",
    );
    let red = redact_query(&url);
    assert!(red.starts_with("http://10.0.3.2:801/eportal/portal/login?"));
    assert!(red.contains("***"));
    assert!(!red.contains("secret"));
}

#[test]
fn already_online_is_success_not_failure() {
    // 真机 2026-09-10 实包：重连时 AC 已记录该 IP 在线，回包 result:0 + ret_code:2。
    let body = r#"dr1004({"result":0,"msg":"IP: 10.43.199.166 已经在线，","ret_code":2})"#;
    assert_eq!(parse_portal_reply(body), PortalResult::AlreadyOnline);

    // ret_code 以字符串形式回时也要认。
    let body = r#"dr1004({"result":0,"msg":"already online","ret_code":"2"})"#;
    assert_eq!(parse_portal_reply(body), PortalResult::AlreadyOnline);

    // ret_code:1（密码错误）仍必须是失败，别把真错误吞成成功。
    let body = r#"dr1004({"result":0,"msg":"密码错误","ret_code":1})"#;
    assert_eq!(
        parse_portal_reply(body),
        PortalResult::Failure("密码错误".into())
    );
}
