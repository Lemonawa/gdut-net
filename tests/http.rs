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

use gdut_net::http::{get, get_async, Request};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

fn serve(
    handler: impl FnOnce(TcpStream) + Send + 'static,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        handler(stream);
    });
    (format!("http://{address}/"), handle)
}

fn slow_body(mut stream: TcpStream) {
    stream.write_all(b"HTTP/1.0 200 OK\r\n\r\n").unwrap();
    // Every byte arrives well within the old per-read timeout.
    for _ in 0..30 {
        if stream.write_all(b"x").is_err() {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

#[test]
fn synchronous_request_deadline_bounds_a_continuously_dripping_body() {
    let (url, server) = serve(slow_body);
    let started = Instant::now();
    let result = get(&Request {
        url: &url,
        bind_ip: Ipv4Addr::LOCALHOST,
        user_agent: "test",
        timeout: Duration::from_millis(180),
        max_bytes: 65536,
        read_policy: ReadPolicy::Complete,
    });
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_millis(700));
    server.join().unwrap();
}

#[tokio::test]
async fn asynchronous_request_deadline_bounds_a_continuously_dripping_body() {
    let (url, server) = serve(slow_body);
    let started = Instant::now();
    let result = get_async(
        url,
        Ipv4Addr::LOCALHOST,
        "test",
        Duration::from_millis(180),
        65536,
        ReadPolicy::Complete,
    )
    .await;
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_millis(700));
    server.join().unwrap();
}

#[tokio::test]
async fn cancelling_async_request_closes_the_actual_socket() {
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    let (url, server) = serve(move |mut stream| {
        ready_tx.send(()).unwrap();
        let mut byte = [0u8; 1];
        let result = stream.read(&mut byte);
        closed_tx.send(matches!(result, Ok(0))).unwrap();
    });
    let task = tokio::spawn(get_async(
        url,
        Ipv4Addr::LOCALHOST,
        "test",
        Duration::from_secs(10),
        65536,
        ReadPolicy::Complete,
    ));
    ready_rx.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(tokio::time::timeout(Duration::from_secs(1), closed_rx)
        .await
        .unwrap()
        .unwrap());
    server.join().unwrap();
}

#[tokio::test]
async fn probe_finishes_after_complete_headers_without_waiting_for_body() {
    let (url, server) = serve(slow_body);
    let started = Instant::now();
    let response = get_async(
        url,
        Ipv4Addr::LOCALHOST,
        "test",
        Duration::from_secs(2),
        16384,
        ReadPolicy::AcceptPartial,
    )
    .await
    .unwrap();
    assert_eq!(response.status, 200);
    assert!(started.elapsed() < Duration::from_millis(500));
    server.join().unwrap();
}

#[tokio::test]
async fn partial_auth_redirect_header_is_not_mistaken_for_alive() {
    let (url, server) = serve(|mut stream| {
        stream
            .write_all(b"HTTP/1.0 302 Found\r\nLocation: http://portal")
            .unwrap();
    });
    assert!(get_async(
        url,
        Ipv4Addr::LOCALHOST,
        "test",
        Duration::from_secs(1),
        16384,
        ReadPolicy::AcceptPartial
    )
    .await
    .is_err());
    server.join().unwrap();
}
