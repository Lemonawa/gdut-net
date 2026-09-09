//! eportal 认证（纯逻辑）：URL 构建（percent-encode）与 JSONP 回包解析。
//! 协议事实来自 lin-snow/GDUT-Login（2024 实证，大学城），常量见 ADR-0005。

use std::net::Ipv4Addr;

/// percent-encode：非 unreserved（RFC 3986）一律 %XX（大写 hex）。
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 组装 eportal 登录 GET URL。参数集与已实证脚本一致（MAC 全零可行）。
pub fn build_login_url(base: &str, user: &str, pass: &str, ip: Ipv4Addr, ac_ip: &str) -> String {
    format!(
        "{base}?callback=dr1004&login_method=1&user_account={}&user_password={}&wlan_user_ip={ip}\
&wlan_user_ipv6=&wlan_user_mac=000000000000&wlan_ac_ip={}&wlan_ac_name=\
&jsVersion=4.1.3&terminal_type=2&lang=zh-cn&v=2041",
        urlencode(user),
        urlencode(pass),
        urlencode(ac_ip),
    )
}

/// 日志/事件用脱敏：query 整体打码（内含明文密码）。
pub fn redact_query(url: &str) -> String {
    match url.split_once('?') {
        Some((head, _)) => format!("{head}?***"),
        None => url.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortalResult {
    Success,
    Failure(String),
    Malformed,
}

/// 解析 JSONP（`dr1004({...})`）或裸 JSON：`result` == "1"/1 → Success。
pub fn parse_portal_reply(body: &str) -> PortalResult {
    let inner = extract_json(body);
    let Some(v) = serde_json::from_str::<serde_json::Value>(inner).ok() else {
        return PortalResult::Malformed;
    };
    match v.get("result").map(|r| {
        r.as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| r.to_string())
    }) {
        Some(r) if r == "1" => PortalResult::Success,
        Some(_) => PortalResult::Failure(
            v.get("msg")
                .and_then(|m| m.as_str())
                .unwrap_or("no msg")
                .to_string(),
        ),
        None => PortalResult::Malformed,
    }
}

/// 取 callback(...) 括号内内容；无括号则原样（裸 JSON 兜底）。
fn extract_json(body: &str) -> &str {
    match (body.find('('), body.rfind(')')) {
        (Some(a), Some(b)) if a < b => &body[a + 1..b],
        _ => body.trim(),
    }
}
