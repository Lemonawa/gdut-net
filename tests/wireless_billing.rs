use gdut_net::wireless::portal::login;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

// A real HTTP peer checks the request order, bound source, and absence of follow-up
// requests after rejected/ambiguous confirmation or cancellation.
async fn portal(
    replies: Vec<(u16, &'static str)>,
) -> (
    String,
    mpsc::UnboundedReceiver<String>,
    tokio::task::JoinHandle<TcpListener>,
) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let url = format!(
        "http://{}/eportal/portal/login",
        listener.local_addr().unwrap()
    );
    let (tx, rx) = mpsc::unbounded_channel();
    let server = tokio::spawn(async move {
        for (status, body) in replies {
            let (mut stream, peer) = listener.accept().await.unwrap();
            assert_eq!(peer.ip(), Ipv4Addr::LOCALHOST);
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(stream.read_u8().await.unwrap());
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.contains("User-Agent: python-requests/2.31.0\r\n"));
            stream
                .write_all(
                    format!(
                        "HTTP/1.0 {status} OK\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.shutdown().await.unwrap();
            tx.send(request).unwrap();
        }
        listener
    });
    (url, rx, server)
}

async fn no_more_requests(server: tokio::task::JoinHandle<TcpListener>) {
    let listener = server.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn ready_cycle_logs_in_without_confirmation() {
    let (base, mut requests, server) = portal(vec![
        (200, r#"dr1003({"code":"1","result":"ok"})"#),
        (200, r#"dr1004({"result":1})"#),
    ])
    .await;
    let (_, reply) = login(
        Ipv4Addr::LOCALHOST,
        &base,
        "u&x",
        "p&secret",
        "172.16.254.2",
    )
    .await
    .unwrap();
    assert!(reply.contains(r#""result":1"#));
    let query = requests.recv().await.unwrap();
    assert!(query.contains("custom/operator?callback=dr1003&account=u%26x&r=0&"));
    assert!(!query.contains("secret"));
    let auth = requests.recv().await.unwrap();
    assert!(auth.contains("/portal/login?"));
    assert!(auth.contains("user_password=p%26secret&"));
    no_more_requests(server).await;
}

#[tokio::test]
async fn required_cycle_is_confirmed_once_then_settled_before_login() {
    let (base, mut requests, server) = portal(vec![
        (200, r#"{"code":"1","result":"fail"}"#),
        (200, r#"{"code":"1","result":"ok"}"#),
        (200, r#"{"result":1}"#),
    ])
    .await;
    let started = Instant::now();
    login(Ipv4Addr::LOCALHOST, &base, "u", "p", "172.16.254.2")
        .await
        .unwrap();
    assert!(started.elapsed() >= Duration::from_secs(4));
    assert!(requests.recv().await.unwrap().contains("&r=0&"));
    assert!(requests.recv().await.unwrap().contains("&r=1&"));
    assert!(requests.recv().await.unwrap().contains("/portal/login?"));
    no_more_requests(server).await;
}

#[tokio::test]
async fn invalid_status_does_not_confirm_or_send_password() {
    for reply in [
        (200, r#"{"code":0,"result":"fail","msg":"private value"}"#),
        (200, r#"{"result":"fail"}"#),
        (302, r#"{"code":1,"result":"fail"}"#),
    ] {
        let (base, mut requests, server) = portal(vec![reply]).await;
        let err = login(Ipv4Addr::LOCALHOST, &base, "u", "secret", "172.16.254.2")
            .await
            .unwrap_err()
            .to_string();
        assert!(!err.contains("secret") && !err.contains("private value"));
        assert!(requests.recv().await.unwrap().contains("&r=0&"));
        no_more_requests(server).await;
    }
}

#[tokio::test]
async fn rejected_or_malformed_confirmation_does_not_log_in() {
    for reply in [
        (200, r#"{"code":1,"result":"fail"}"#),
        (200, "<html>error</html>"),
        (500, r#"{"code":1,"result":"ok"}"#),
    ] {
        let (base, mut requests, server) =
            portal(vec![(200, r#"{"code":1,"result":"fail"}"#), reply]).await;
        assert!(login(Ipv4Addr::LOCALHOST, &base, "u", "p", "172.16.254.2")
            .await
            .is_err());
        assert!(requests.recv().await.unwrap().contains("&r=0&"));
        assert!(requests.recv().await.unwrap().contains("&r=1&"));
        no_more_requests(server).await;
    }
}

#[tokio::test]
async fn cancellation_during_cycle_settle_does_not_log_in() {
    let (base, mut requests, server) = portal(vec![
        (200, r#"{"code":1,"result":"fail"}"#),
        (200, r#"{"code":1,"result":"ok"}"#),
    ])
    .await;
    let flow =
        tokio::spawn(
            async move { login(Ipv4Addr::LOCALHOST, &base, "u", "p", "172.16.254.2").await },
        );
    requests.recv().await.unwrap();
    requests.recv().await.unwrap();
    flow.abort();
    assert!(flow.await.unwrap_err().is_cancelled());
    no_more_requests(server).await;
}
