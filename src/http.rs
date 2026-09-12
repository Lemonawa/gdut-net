//! HTTP/1.0 GET 原语的唯一实现：源地址绑定是 interface 参数。
//!
//! cfg-free：URL / 状态行 / 认证跳转的纯解析（Linux TDD）；
//! cfg(windows)：`get`（socket2 绑源、超时、字节上限）与 `get_async`。
//! 语义与既有调用点一致：HTTP/1.0、不跟随重定向、Connection: close。

/// 解析目标：`http://host[:port][/path]`。
#[derive(Debug, PartialEq, Eq)]
pub struct Target {
    /// 主机（不含端口）。
    pub host: String,
    /// 端口（无端口 = 80）。
    pub port: u16,
    /// 请求路径（无路径 = "/"）。
    pub path: String,
    /// Host 头原样：`host[:port]`（URL 写了端口才带端口）。
    pub host_header: String,
}

/// 仅接受 `http://`；主机可为 IPv4 或域名（IPv4 字面量的强校验在 probe 侧）。
pub fn parse_url(url: &str) -> Option<Target> {
    let rest = url.strip_prefix("http://")?;
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if hostport.is_empty() {
        return None;
    }
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok()?),
        None => (hostport, 80),
    };
    if host.is_empty() {
        return None;
    }
    Some(Target {
        host: host.to_string(),
        port,
        path: path.to_string(),
        host_header: hostport.to_string(),
    })
}

/// 宽容状态行解析：`HTTP/1.0 302 Found`、`HTTP/1.1 200 OK`、`HTTP/2 200` 均可。
pub fn parse_status_code(status_line: &str) -> Option<u16> {
    let mut parts = status_line.split_ascii_whitespace();
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    parts.next()?.parse().ok()
}

/// 判定 Location（已小写）是否为认证页跳转（wlanacip|nexturl|portal）。
pub fn is_auth_redirect(location_lower: &str) -> bool {
    ["wlanacip", "nexturl", "portal"]
        .iter()
        .any(|k| location_lower.contains(k))
}

/// 读结束后的取舍：Complete 政策任何读错误都算失败；AcceptPartial 在已有字节时接受
/// （探针只需要状态行 + Location；服务器/中间盒 RST 或超时不应丢弃已收到的应答）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadPolicy {
    Complete,
    AcceptPartial,
}

/// `read_ok` = read_to_end 正常到 EOF；`got_bytes` = 缓冲区已有数据。
pub fn read_acceptable(policy: ReadPolicy, read_ok: bool, got_bytes: bool) -> bool {
    read_ok || (policy == ReadPolicy::AcceptPartial && got_bytes)
}

#[cfg(windows)]
mod win {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    use std::time::Duration;

    use anyhow::{anyhow, Context, Result};
    use socket2::{Domain, Protocol, Socket, Type};

    use super::{parse_status_code, parse_url, read_acceptable, ReadPolicy};

    pub struct Request<'a> {
        pub url: &'a str,
        pub bind_ip: Ipv4Addr,
        pub user_agent: &'a str,
        pub timeout: Duration,
        pub max_bytes: u64,
        /// 读错误的取舍（见 [`ReadPolicy`]）：probe 用 AcceptPartial，portal 用 Complete。
        pub read_policy: ReadPolicy,
    }

    #[derive(Debug, Clone)]
    pub struct Response {
        pub status: u16,
        pub location_lower: String,
        pub body: String,
    }

    /// 同步 GET：socket2 绑源 IP、connect/read/write 用同一 timeout、读上限 max_bytes。
    /// 读结束时按 `req.read_policy` 取舍：AcceptPartial 在已有字节时接受读错误。
    pub fn get(req: &Request<'_>) -> Result<Response> {
        let target = parse_url(req.url).ok_or_else(|| anyhow!("Unsupported URL (http only)"))?;
        let addr: SocketAddr = format!("{}:{}", target.host, target.port)
            .parse()
            .with_context(|| format!("Bad host in URL: {}", target.host))?;
        let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
        socket.bind(&SocketAddr::from((req.bind_ip, 0)).into())?;
        socket.set_read_timeout(Some(req.timeout))?;
        socket.set_write_timeout(Some(req.timeout))?;
        socket
            .connect_timeout(&addr.into(), req.timeout)
            .with_context(|| format!("connect {addr} failed"))?;
        let mut stream = TcpStream::from(socket);
        let request = format!(
            "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: {}\r\nConnection: close\r\n\r\n",
            target.path, target.host_header, req.user_agent
        );
        stream.write_all(request.as_bytes())?;
        let mut buf = Vec::new();
        if let Err(e) = stream.take(req.max_bytes).read_to_end(&mut buf) {
            if !read_acceptable(req.read_policy, false, !buf.is_empty()) {
                return Err(e).context("read response failed");
            }
        }
        let text = String::from_utf8_lossy(&buf);
        let status = parse_status_code(text.split("\r\n").next().unwrap_or_default())
            .ok_or_else(|| anyhow!("Malformed HTTP status line"))?;
        let location_lower = text
            .split("\r\n")
            .skip(1)
            .take_while(|l| !l.is_empty())
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.trim()
                    .eq_ignore_ascii_case("location")
                    .then(|| v.trim().to_ascii_lowercase())
            })
            .unwrap_or_default();
        let body = text
            .split_once("\r\n\r\n")
            .map(|(_, b)| b.to_string())
            .unwrap_or_default();
        Ok(Response {
            status,
            location_lower,
            body,
        })
    }

    /// async 适配：`spawn_blocking` 包同步实现；panic 视为错误。
    pub async fn get_async(
        url: String,
        bind_ip: Ipv4Addr,
        user_agent: &'static str,
        timeout: Duration,
        max_bytes: u64,
        read_policy: ReadPolicy,
    ) -> Result<Response> {
        tokio::task::spawn_blocking(move || {
            get(&Request {
                url: &url,
                bind_ip,
                user_agent,
                timeout,
                max_bytes,
                read_policy,
            })
        })
        .await
        .map_err(|e| anyhow!("http task panicked: {e}"))?
    }
}

#[cfg(windows)]
pub use win::{get, get_async, Request, Response};
