//! 两级掉线探测：网关 ICMP 探链路 + HTTP 复核是否被服务器踢。
//!
//! 一切探测流量绑定物理适配器源 IP（ADR-0003），绝不走 TUN。
//! 纯逻辑（HTTP 判定）跨平台 TDD；Win32 胶水以
//! `cargo check --target x86_64-pc-windows-msvc` 验证。

use std::net::Ipv4Addr;
use std::time::Duration;

/// 单次探测结论：LinkDown 由 watchdog 决定是否重拨。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeVerdict {
    Alive,
    LinkDown,
    Kicked,
}

#[derive(Debug, Clone)]
pub struct ProbeCfg {
    pub interval: Duration,
    pub http_url: String,
}

/// HTTP 复核判定：302 且 Location 指向认证页（wlanacip|nexturl|portal）→ 被踢；
/// 其余（200、非认证页跳转）→ 仍 Alive。纯函数，便于测试。
pub fn verdict_from_http(saw_redirect: bool, nexturl_is_auth: bool) -> ProbeVerdict {
    if saw_redirect && nexturl_is_auth {
        ProbeVerdict::Kicked
    } else {
        ProbeVerdict::Alive
    }
}

/// 解析 HTTP 探测目标：仅接受 `http://` + IPv4 字面量（可带端口与路径），
/// 返回 (IP, `host:port`)。config::validate 与 http_get_probe 共用本实现，
/// 避免双实现漂移。纯函数，便于测试。
pub fn parse_http_probe_target(url: &str) -> Option<(Ipv4Addr, String)> {
    let rest = url.strip_prefix("http://")?;
    let (host, _path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (ip, port) = match host.rsplit_once(':') {
        Some((h, p)) => (h.parse::<Ipv4Addr>().ok()?, p.parse::<u16>().ok()?),
        None => (host.parse::<Ipv4Addr>().ok()?, 80),
    };
    Some((ip, format!("{ip}:{port}")))
}

/// 两级探测综合判定（ADR-0003）：ICMP 探链路、HTTP 探被踢，结果综合。
/// `icmp_ok`：Some(网关 ICMP 是否通)，None = 未执行/失败不可判；
/// `http`：Some(HTTP 复核结论)，None = 未执行/失败不可判。
///
/// - HTTP 302+认证页 → Kicked（无论 ICMP 结果——被踢时网关仍通）；
/// - ICMP 通 且 HTTP 非 Kicked（可达或不可判）→ Alive；
/// - ICMP 不通 且 HTTP 不可判 → LinkDown（双失败）；
/// - ICMP 不通/不可判 但 HTTP Alive → Alive（链路探不到但出口通，保守在线）。
pub fn combine(icmp_ok: Option<bool>, http: Option<ProbeVerdict>) -> ProbeVerdict {
    match (icmp_ok, http) {
        (_, Some(ProbeVerdict::Kicked)) => ProbeVerdict::Kicked,
        (Some(true), _) => ProbeVerdict::Alive,
        (_, Some(ProbeVerdict::Alive)) => ProbeVerdict::Alive,
        _ => ProbeVerdict::LinkDown,
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::{combine, parse_http_probe_target, verdict_from_http, ProbeVerdict};

    #[test]
    fn redirect_to_auth_page_means_kicked() {
        assert_eq!(verdict_from_http(true, true), ProbeVerdict::Kicked);
        assert_eq!(verdict_from_http(true, false), ProbeVerdict::Alive);
        assert_eq!(verdict_from_http(false, false), ProbeVerdict::Alive);
    }

    #[test]
    fn icmp_down_http_alive_is_alive() {
        // 链路探不到但出口通：保守在线。
        assert_eq!(
            combine(Some(false), Some(ProbeVerdict::Alive)),
            ProbeVerdict::Alive
        );
    }

    #[test]
    fn icmp_down_http_fail_is_link_down() {
        assert_eq!(combine(Some(false), None), ProbeVerdict::LinkDown);
        assert_eq!(combine(None, None), ProbeVerdict::LinkDown);
    }

    #[test]
    fn http_auth_redirect_is_kicked_regardless_of_icmp() {
        assert_eq!(
            combine(Some(true), Some(ProbeVerdict::Kicked)),
            ProbeVerdict::Kicked
        );
        assert_eq!(
            combine(Some(false), Some(ProbeVerdict::Kicked)),
            ProbeVerdict::Kicked
        );
        assert_eq!(
            combine(None, Some(ProbeVerdict::Kicked)),
            ProbeVerdict::Kicked
        );
    }

    #[test]
    fn icmp_ok_means_alive_unless_kicked() {
        assert_eq!(
            combine(Some(true), Some(ProbeVerdict::Alive)),
            ProbeVerdict::Alive
        );
        // HTTP 不可判但链路通：保守在线。
        assert_eq!(combine(Some(true), None), ProbeVerdict::Alive);
        assert_eq!(
            combine(None, Some(ProbeVerdict::Alive)),
            ProbeVerdict::Alive
        );
    }

    #[test]
    fn parse_http_probe_target_accepts_ipv4_literal() {
        let (ip, hostport) = parse_http_probe_target("http://9.9.9.9").unwrap();
        assert_eq!(ip, Ipv4Addr::new(9, 9, 9, 9));
        assert_eq!(hostport, "9.9.9.9:80");
        let (ip, hostport) = parse_http_probe_target("http://192.168.1.1:8080/portal").unwrap();
        assert_eq!(ip, Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(hostport, "192.168.1.1:8080");
    }

    #[test]
    fn parse_http_probe_target_rejects_non_ipv4() {
        assert!(parse_http_probe_target("http://example.com").is_none());
        assert!(parse_http_probe_target("https://9.9.9.9").is_none());
        assert!(parse_http_probe_target("").is_none());
        assert!(parse_http_probe_target("http://9.9.9.9:99999").is_none());
        assert!(parse_http_probe_target("http://[::1]/").is_none());
    }
}

#[cfg(windows)]
mod win {
    use std::io::{Read, Write};
    use std::mem::size_of;
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    use std::time::Duration;

    use socket2::{Domain, Protocol, Socket, Type};
    use tokio::task::spawn_blocking;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::NetworkManagement::IpHelper::{
        IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho2Ex, ICMP_ECHO_REPLY, IP_OPTION_INFORMATION,
    };
    use windows::Win32::System::IO::PIO_APC_ROUTINE;

    use super::{combine, parse_http_probe_target, verdict_from_http, ProbeVerdict};

    const ICMP_TIMEOUT_MS: u32 = 1500;
    const HTTP_TIMEOUT: Duration = Duration::from_secs(3);
    const HTTP_MAX_RESPONSE: u64 = 16 * 1024;

    /// IPv4 → 网络序 u32（`htonl` 语义；IcmpSendEcho2Ex 的 IP 参数要求）。
    fn htonl(ip: Ipv4Addr) -> u32 {
        u32::from(ip).swap_bytes()
    }

    /// 宽容的状态行解析：`HTTP/1.0 302 Found`、`HTTP/1.1 200 OK`、`HTTP/2 200` 均可。
    fn parse_status_code(status_line: &str) -> Option<u16> {
        let mut parts = status_line.split_ascii_whitespace();
        let version = parts.next()?;
        if !version.starts_with("HTTP/") {
            return None;
        }
        parts.next()?.parse().ok()
    }

    /// 判定 Location（已小写）是否为认证页跳转（wlanacip|nexturl|portal）。
    fn is_auth_redirect(location_lower: &str) -> bool {
        ["wlanacip", "nexturl", "portal"]
            .iter()
            .any(|k| location_lower.contains(k))
    }

    /// HTTP GET 探测（HTTP/1.0，不跟随重定向），返回 (状态码, Location 小写)。
    /// socket2 建套接字并绑物理适配器源 IP（std 1.97 TcpStream 无 bind）。
    fn http_get_probe(src_ip: Ipv4Addr, http_url: &str) -> Option<(u16, String)> {
        let rest = http_url.strip_prefix("http://")?;
        let (host, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let addr: SocketAddr = parse_http_probe_target(http_url)
            .and_then(|(_, hostport)| hostport.parse().ok())
            .or_else(|| {
                log::debug!("探测：HTTP 地址解析失败 {http_url}");
                None
            })?;
        let socket = match Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)) {
            Ok(s) => s,
            Err(e) => {
                log::debug!("探测：套接字创建失败：{e}");
                return None;
            }
        };
        if let Err(e) = socket.bind(&SocketAddr::from((src_ip, 0)).into()) {
            log::debug!("探测：绑源 IP {src_ip} 失败：{e}");
            return None;
        }
        socket.set_read_timeout(Some(HTTP_TIMEOUT)).ok()?;
        socket.set_write_timeout(Some(HTTP_TIMEOUT)).ok()?;
        if let Err(e) = socket.connect_timeout(&addr.into(), HTTP_TIMEOUT) {
            log::debug!("探测：HTTP connect {addr} 超时/失败：{e}");
            return None;
        }
        let mut stream = TcpStream::from(socket);
        let req = format!(
            "GET {path} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: gdut-net-probe\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).ok()?;
        let mut buf = Vec::new();
        let read_ok = stream.take(HTTP_MAX_RESPONSE).read_to_end(&mut buf).is_ok();
        if !read_ok && buf.is_empty() {
            return None;
        }
        let text = String::from_utf8_lossy(&buf);
        let code = parse_status_code(text.split("\r\n").next()?)?;
        let location = text
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
        Some((code, location))
    }

    /// 网关 ICMP 探链路：绑源 IP 发 32 字节 echo，1500ms 内收到应答即链路通。
    /// 不通（超时/不可达）是预期探测分支，返回 false 而非错误。
    fn icmp_gateway_alive(src_ip: Ipv4Addr, gateway: Ipv4Addr) -> bool {
        let handle: HANDLE = match unsafe { IcmpCreateFile() } {
            Ok(h) if !h.is_invalid() => h,
            Err(e) => {
                log::debug!("探测：IcmpCreateFile 失败：{e}");
                return false;
            }
            _ => return false,
        };
        let payload = [0u8; 32];
        let reply_size = size_of::<ICMP_ECHO_REPLY>() as u32 + payload.len() as u32 + 8;
        let mut reply_buf = vec![0u8; reply_size as usize];
        let sent = unsafe {
            IcmpSendEcho2Ex(
                handle,
                None,
                PIO_APC_ROUTINE::None,
                None,
                htonl(src_ip),
                htonl(gateway),
                payload.as_ptr().cast(),
                payload.len() as u16,
                None::<*const IP_OPTION_INFORMATION>,
                reply_buf.as_mut_ptr().cast(),
                reply_size,
                ICMP_TIMEOUT_MS,
            )
        };
        // 仅在收到应答（sent > 0）时读取 reply，避免读未写缓冲。
        let alive = if sent > 0 {
            let status = unsafe { (*(reply_buf.as_ptr() as *const ICMP_ECHO_REPLY)).Status };
            status == 0
        } else {
            log::debug!("探测：网关 {gateway} ICMP 不通（超时/不可达）");
            false
        };
        unsafe {
            let _ = IcmpCloseHandle(handle);
        }
        alive
    }

    /// 两级探测（ADR-0003）：每次都走两级——网关 ICMP 探链路 + HTTP 复核探被踢，
    /// 结果经 [`combine`] 综合判定。被踢时网关仍通，单看 ICMP 会漏判僵死会话。
    pub async fn probe_once(
        src_ip: Ipv4Addr,
        gateway: Option<Ipv4Addr>,
        http_url: &str,
    ) -> ProbeVerdict {
        let icmp_ok = match gateway {
            Some(gw) => Some(
                spawn_blocking(move || icmp_gateway_alive(src_ip, gw))
                    .await
                    .unwrap_or(false),
            ),
            None => None,
        };
        let url = http_url.to_string();
        let http = spawn_blocking(move || http_get_probe(src_ip, &url))
            .await
            .unwrap_or(None)
            .map(|(status, location)| {
                verdict_from_http(status == 302, is_auth_redirect(&location))
            });
        let verdict = combine(icmp_ok, http);
        log::info!(
            "探测：ICMP {}，HTTP 复核 {} → {verdict:?}",
            match icmp_ok {
                Some(true) => "通",
                Some(false) => "不通",
                None => "未执行",
            },
            match http {
                Some(ProbeVerdict::Kicked) => "判被踢",
                Some(ProbeVerdict::Alive) => "判在线",
                Some(ProbeVerdict::LinkDown) | None => "失败/不可判",
            },
        );
        verdict
    }
}

/// 单次两级探测（Windows）：网关 ICMP → HTTP 复核。
#[cfg(windows)]
pub async fn probe_once(
    src_ip: Ipv4Addr,
    gateway: Option<Ipv4Addr>,
    http_url: &str,
) -> ProbeVerdict {
    win::probe_once(src_ip, gateway, http_url).await
}
