//! HTTP/1.0 GET 原语的唯一实现：源地址绑定是 interface 参数。
//!
//! cfg-free：URL / 状态行 / 认证跳转的纯解析（Linux TDD）；
//! `get` / `get_async`：源地址绑定、整体截止时间与字节上限；异步取消关闭 socket。
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
/// （探针收到完整状态行与响应头后即结束；不接受被截断的 Location）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadPolicy {
    Complete,
    AcceptPartial,
}

/// `read_ok` = read_to_end 正常到 EOF；`got_bytes` = 缓冲区已有数据。
pub fn read_acceptable(policy: ReadPolicy, read_ok: bool, got_bytes: bool) -> bool {
    read_ok || (policy == ReadPolicy::AcceptPartial && got_bytes)
}

mod transport {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    use std::time::{Duration, Instant};

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

    fn target_address(url: &str) -> Result<(super::Target, SocketAddr)> {
        let target = parse_url(url).ok_or_else(|| anyhow!("Unsupported URL (http only)"))?;
        let addr = format!("{}:{}", target.host, target.port)
            .parse()
            .with_context(|| format!("Bad host in URL: {}", target.host))?;
        Ok((target, addr))
    }

    fn request_bytes(target: &super::Target, user_agent: &str) -> Vec<u8> {
        format!(
            "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: {}\r\nConnection: close\r\n\r\n",
            target.path, target.host_header, user_agent
        )
        .into_bytes()
    }

    fn remaining(deadline: Instant) -> std::io::Result<Duration> {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "HTTP deadline exceeded")
            })
    }

    fn response(buf: &[u8]) -> Result<Response> {
        let text = String::from_utf8_lossy(buf);
        let (headers, body) = text
            .split_once("\r\n\r\n")
            .ok_or_else(|| anyhow!("Incomplete HTTP response headers"))?;
        let status = parse_status_code(headers.split("\r\n").next().unwrap_or_default())
            .ok_or_else(|| anyhow!("Malformed HTTP status line"))?;
        let location_lower = headers
            .split("\r\n")
            .skip(1)
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.trim()
                    .eq_ignore_ascii_case("location")
                    .then(|| value.trim().to_ascii_lowercase())
            })
            .unwrap_or_default();
        Ok(Response {
            status,
            location_lower,
            body: body.to_string(),
        })
    }

    fn headers_complete(buf: &[u8]) -> bool {
        buf.windows(4).any(|bytes| bytes == b"\r\n\r\n")
    }

    /// Synchronous transport: every I/O uses the remaining whole-request budget.
    pub fn get(req: &Request<'_>) -> Result<Response> {
        let deadline = Instant::now() + req.timeout;
        let (target, addr) = target_address(req.url)?;
        let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
        socket.bind(&SocketAddr::from((req.bind_ip, 0)).into())?;
        socket.connect_timeout(&addr.into(), remaining(deadline)?)?;
        let mut stream = TcpStream::from(socket);
        let request = request_bytes(&target, req.user_agent);
        let mut written = 0;
        while written < request.len() {
            stream.set_write_timeout(Some(remaining(deadline)?))?;
            let n = stream.write(&request[written..])?;
            if n == 0 {
                return Err(anyhow!("HTTP write returned zero"));
            }
            written += n;
        }
        let mut buf = Vec::new();
        while (buf.len() as u64) < req.max_bytes {
            let mut chunk = [0u8; 4096];
            let limit = (req.max_bytes - buf.len() as u64).min(chunk.len() as u64) as usize;
            let read = remaining(deadline).and_then(|budget| {
                stream.set_read_timeout(Some(budget))?;
                stream.read(&mut chunk[..limit])
            });
            match read {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) => {
                    if !read_acceptable(req.read_policy, false, !buf.is_empty()) {
                        return Err(e).context("read response failed");
                    }
                    break;
                }
            }
            // Probes need only status + Location, never the response body.
            if req.read_policy == ReadPolicy::AcceptPartial && headers_complete(&buf) {
                break;
            }
        }
        response(&buf)
    }

    /// Native async transport: dropping this future closes its socket immediately.
    pub async fn get_async(
        url: String,
        bind_ip: Ipv4Addr,
        user_agent: &'static str,
        timeout: Duration,
        max_bytes: u64,
        read_policy: ReadPolicy,
    ) -> Result<Response> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let deadline = tokio::time::Instant::now() + timeout;
        let io = async {
            let (target, addr) = target_address(&url)?;
            let socket = tokio::net::TcpSocket::new_v4()?;
            socket.bind(SocketAddr::from((bind_ip, 0)))?;
            let mut stream = socket.connect(addr).await?;
            stream
                .write_all(&request_bytes(&target, user_agent))
                .await?;
            let mut buf = Vec::new();
            while (buf.len() as u64) < max_bytes {
                let mut chunk = [0u8; 4096];
                let limit = (max_bytes - buf.len() as u64).min(chunk.len() as u64) as usize;
                match stream.read(&mut chunk[..limit]).await {
                    Ok(0) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    Err(e) => {
                        if !read_acceptable(read_policy, false, !buf.is_empty()) {
                            return Err(e).context("read response failed");
                        }
                        break;
                    }
                }
                if read_policy == ReadPolicy::AcceptPartial && headers_complete(&buf) {
                    break;
                }
            }
            response(&buf)
        };
        tokio::time::timeout_at(deadline, io)
            .await
            .map_err(|_| anyhow!("HTTP request deadline exceeded"))?
    }
}

pub use transport::{get, get_async, Request, Response};
