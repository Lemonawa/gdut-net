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

#[cfg(windows)]
mod win {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    use std::time::Duration;

    use socket2::{Domain, Protocol, Socket, Type};
    use tokio::task::spawn_blocking;

    // 8s：真机实测（2026-09-10）绑源 SYN 偶发被丢（30s 关联年龄下仍 t+34s
    // 失败 / t+40s 成功），Windows SYN 重传 1s/2s/4s 三连，3s 会拦腰截断。
    const TIMEOUT: Duration = Duration::from_secs(8);
    const MAX_RESPONSE: u64 = 64 * 1024;
    /// 与已实证脚本一致的 UA（requests 默认值）；设备计数按 MAC+UA（CONTEXT.md），别乱换。
    const PORTAL_UA: &str = "python-requests/2.31.0";

    fn parse_status(line: &str) -> Option<u16> {
        line.split_ascii_whitespace().nth(1)?.parse().ok()
    }

    fn portal_get_blocking(src_ip: Ipv4Addr, url: &str) -> Option<(u16, String)> {
        let rest = url.strip_prefix("http://")?;
        let (host, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let addr: SocketAddr = format!("{host}:80").parse().ok().or_else(|| {
            // host 形如 "10.0.3.2:801"
            host.parse::<SocketAddr>().ok()
        })?;
        let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).ok()?;
        socket.bind(&SocketAddr::from((src_ip, 0)).into()).ok()?;
        socket.set_read_timeout(Some(TIMEOUT)).ok()?;
        socket.set_write_timeout(Some(TIMEOUT)).ok()?;
        socket.connect_timeout(&addr.into(), TIMEOUT).ok()?;
        let mut stream = TcpStream::from(socket);
        let req = format!(
            "GET {path} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: {PORTAL_UA}\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).ok()?;
        let mut buf = Vec::new();
        stream.take(MAX_RESPONSE).read_to_end(&mut buf).ok()?;
        let text = String::from_utf8_lossy(&buf);
        let status = parse_status(text.split("\r\n").next()?)?;
        let body = text
            .split_once("\r\n\r\n")
            .map(|(_, b)| b.to_string())
            .unwrap_or_default();
        Some((status, body))
    }

    pub async fn portal_get(src_ip: Ipv4Addr, url: &str) -> Option<(u16, String)> {
        let url = url.to_string();
        spawn_blocking(move || portal_get_blocking(src_ip, &url))
            .await
            .unwrap_or(None)
    }
}

#[cfg(windows)]
pub use win::portal_get;
