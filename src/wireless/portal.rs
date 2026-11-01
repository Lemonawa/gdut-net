//! eportal authentication: URL/reply logic plus source-bound, cancellable HTTP flow.
//! 协议事实来自 lin-snow/GDUT-Login（2024 实证，大学城），常量见 ADR-0005。
//! Billing gate: campus a58.js (2026-11-01), also documented by FUjr/gdut_network.

use std::net::Ipv4Addr;
use std::time::Duration;

/// Query + optional confirmation + 4s portal settle + login, each request <= 8s.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(32);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const BILLING_SETTLE: Duration = Duration::from_secs(4);
// Device slots are keyed partly by User-Agent; retain the established value.
const PORTAL_UA: &str = "python-requests/2.31.0";

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

/// The school's a58.js uses r=0 to query, r=1 only after confirmation is required.
/// Keep the configured login origin and port; never follow a response-supplied URL.
pub fn build_operator_url(base: &str, user: &str, confirm: bool) -> Option<String> {
    let parent = base.strip_suffix("/login")?;
    Some(format!(
        "{parent}/custom/operator?callback=dr1003&account={}&r={}&jsVersion=4.1.3&lang=en",
        urlencode(user),
        u8::from(confirm)
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillingStatus {
    Ready,
    ConfirmationRequired,
    Rejected,
    Malformed,
}

pub fn parse_billing_reply(body: &str) -> BillingStatus {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(extract_json(body)) else {
        return BillingStatus::Malformed;
    };
    match v.get("code") {
        Some(code) if code.as_str() == Some("1") || code.as_u64() == Some(1) => {}
        Some(_) => return BillingStatus::Rejected,
        None => return BillingStatus::Malformed,
    }
    match v.get("result").and_then(|r| r.as_str()) {
        Some("ok") => BillingStatus::Ready,
        Some("fail") => BillingStatus::ConfirmationRequired,
        _ => BillingStatus::Malformed,
    }
}

/// GDUT authentication, including its first-Wi-Fi-login billing-cycle gate.
/// All requests bind the same WLAN source and share the caller's cancellation.
/// No passwords, account IDs, response messages, or query strings reach logs/errors.
pub async fn login(
    src_ip: Ipv4Addr,
    base: &str,
    user: &str,
    pass: &str,
    ac_ip: &str,
) -> anyhow::Result<(u16, String)> {
    tokio::time::timeout(LOGIN_TIMEOUT, login_inner(src_ip, base, user, pass, ac_ip))
        .await
        .map_err(|_| anyhow::anyhow!("Portal authentication deadline exceeded"))?
}

async fn login_inner(
    src_ip: Ipv4Addr,
    base: &str,
    user: &str,
    pass: &str,
    ac_ip: &str,
) -> anyhow::Result<(u16, String)> {
    let query = build_operator_url(base, user, false)
        .ok_or_else(|| anyhow::anyhow!("Portal login URL must end in /login"))?;
    let status = billing_request(src_ip, query).await?;
    match status {
        BillingStatus::Ready => {}
        BillingStatus::ConfirmationRequired => {
            log::info!("Wireless: billing cycle confirmation required");
            let confirm = build_operator_url(base, user, true)
                .ok_or_else(|| anyhow::anyhow!("Portal login URL must end in /login"))?;
            if billing_request(src_ip, confirm).await? != BillingStatus::Ready {
                anyhow::bail!("Billing cycle confirmation rejected; login deferred");
            }
            log::info!("Wireless: billing cycle confirmed; waiting 4s before login");
            tokio::time::sleep(BILLING_SETTLE).await;
        }
        BillingStatus::Rejected => anyhow::bail!("Billing cycle status query rejected"),
        BillingStatus::Malformed => anyhow::bail!("Malformed billing cycle status reply"),
    }
    let url = build_login_url(base, user, pass, src_ip, ac_ip);
    let response = request(src_ip, url)
        .await
        .map_err(|_| anyhow::anyhow!("Portal login request failed"))?;
    Ok((response.status, response.body))
}

async fn request(src_ip: Ipv4Addr, url: String) -> anyhow::Result<crate::http::Response> {
    crate::http::get_async(
        url,
        src_ip,
        PORTAL_UA,
        REQUEST_TIMEOUT,
        64 * 1024,
        crate::http::ReadPolicy::Complete,
    )
    .await
}

async fn billing_request(src_ip: Ipv4Addr, url: String) -> anyhow::Result<BillingStatus> {
    let reply = request(src_ip, url)
        .await
        .map_err(|_| anyhow::anyhow!("Billing cycle request failed"))?;
    if reply.status != 200 {
        anyhow::bail!("Billing cycle request returned HTTP {}", reply.status);
    }
    Ok(parse_billing_reply(&reply.body))
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
    /// 服务器判定该 IP 已在线（`result:0, ret_code:2`，msg 含"已经在线"）。
    /// 语义上是成功：会话已存在，直接用它，别再重试。
    AlreadyOnline,
    Failure(String),
    Malformed,
}

/// 解析 JSONP（`dr1004({...})`）或裸 JSON：
/// - `result == "1"/1` → Success；
/// - `result == 0` 且 `ret_code == 2`（数字或字符串）→ AlreadyOnline
///   （真机 2026-09-10：`{"result":0,"msg":"IP: x 已经在线","ret_code":2}`；
///   错误密码是 ret_code:1，可靠区分，别按 msg 文案匹配）。
pub fn parse_portal_reply(body: &str) -> PortalResult {
    let inner = extract_json(body);
    let Some(v) = serde_json::from_str::<serde_json::Value>(inner).ok() else {
        return PortalResult::Malformed;
    };
    let result = v.get("result").map(|r| {
        r.as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| r.to_string())
    });
    match result {
        Some(r) if r == "1" => PortalResult::Success,
        Some(r) if r == "0" && ret_code(&v) == Some(2) => PortalResult::AlreadyOnline,
        Some(_) => PortalResult::Failure(
            v.get("msg")
                .and_then(|m| m.as_str())
                .unwrap_or("no msg")
                .to_string(),
        ),
        None => PortalResult::Malformed,
    }
}

/// ret_code 数值（数字或数字字符串均可）。
fn ret_code(v: &serde_json::Value) -> Option<u64> {
    let r = v.get("ret_code")?;
    r.as_u64().or_else(|| r.as_str()?.parse().ok())
}

/// 取 callback(...) 括号内内容；无括号则原样（裸 JSON 兜底）。
fn extract_json(body: &str) -> &str {
    match (body.find('('), body.rfind(')')) {
        (Some(a), Some(b)) if a < b => &body[a + 1..b],
        _ => body.trim(),
    }
}
