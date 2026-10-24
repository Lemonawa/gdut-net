//! Local-only comparison of changed I/O policies against the current implementation.
//! Baseline logic is reproduced from bc9a345 using the CURRENT compiler/dependencies.
//! No RAS, adapter enumeration, routes, services, or installed processes are touched.

use std::collections::VecDeque;
use std::hint::black_box;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{anyhow, ensure, Result};
use gdut_net::http::{self, ReadPolicy, Response};
use gdut_net::ipc::protocol::{FrameDecoder, MAX_FRAME_BYTES};
use serde_json::{json, Value};
use socket2::{Domain, Protocol, Socket, Type};

const SLOW_TIMEOUT: Duration = Duration::from_millis(200);
const BODY_TICKS: usize = 25;
const BODY_TICK: Duration = Duration::from_millis(40);

/// The old connect/read/write policy and response parsing, without Windows cfg.
/// Shared parse helpers have identical behavior for the benchmark's literal URL.
fn before_get(url: &str, timeout: Duration) -> Result<Response> {
    let target = http::parse_url(url).ok_or_else(|| anyhow!("Invalid local URL"))?;
    let addr: SocketAddr = format!("{}:{}", target.host, target.port).parse()?;
    ensure!(addr.ip().is_loopback(), "Simulation only permits loopback");
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    socket.bind(&SocketAddr::from((Ipv4Addr::LOCALHOST, 0)).into())?;
    socket.set_read_timeout(Some(timeout))?;
    socket.set_write_timeout(Some(timeout))?;
    socket.connect_timeout(&addr.into(), timeout)?;
    let mut stream = TcpStream::from(socket);
    let request = format!(
        "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: gdut-net-perf\r\nConnection: close\r\n\r\n",
        target.path, target.host_header
    );
    stream.write_all(request.as_bytes())?;
    let mut buf = Vec::new();
    stream.take(64 * 1024).read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    let status = http::parse_status_code(text.split("\r\n").next().unwrap_or_default())
        .ok_or_else(|| anyhow!("Invalid local status"))?;
    let location_lower = text
        .split("\r\n")
        .skip(1)
        .take_while(|line| !line.is_empty())
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
        body: text
            .split_once("\r\n\r\n")
            .map(|(_, body)| body.to_string())
            .unwrap_or_default(),
    })
}

#[derive(Default)]
struct BeforeDecoder {
    buf: Vec<u8>,
}
impl BeforeDecoder {
    // Verbatim FrameDecoder::feed algorithm from bc9a345.
    fn feed(&mut self, chunk: &[u8]) -> VecDeque<Vec<u8>> {
        self.buf.extend_from_slice(chunk);
        let mut frames = VecDeque::new();
        while let Some(pos) = self.buf.iter().position(|&byte| byte == b'\n') {
            frames.push_back(self.buf.drain(..=pos).collect());
        }
        frames
    }
}

#[derive(Clone, Copy)]
enum Profile {
    Fast,
    Slow,
    Cancellation,
}
struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
struct Server {
    url: String,
    active: Arc<AtomicUsize>,
    ready: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    address: SocketAddr,
    thread: JoinHandle<()>,
}
impl Server {
    fn start(requests: usize, profile: Profile) -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        let url = format!("http://{address}/");
        let active = Arc::new(AtomicUsize::new(0));
        let ready = Arc::new(AtomicUsize::new(0));
        let ready_count = Arc::clone(&ready);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_accepting = Arc::clone(&stop);
        let count = Arc::clone(&active);
        let thread = thread::spawn(move || {
            let mut handlers = Vec::new();
            while handlers.len() < requests {
                let (stream, _) = listener.accept().expect("Local accept failed");
                if stop_accepting.load(Ordering::Acquire) {
                    break;
                }
                let count = Arc::clone(&count);
                let ready = Arc::clone(&ready_count);
                count.fetch_add(1, Ordering::SeqCst);
                handlers.push(thread::spawn(move || {
                    let _active = Active(count);
                    serve(stream, profile, &ready);
                }));
            }
            for handler in handlers {
                handler.join().expect("Local response handler panicked");
            }
        });
        Ok(Self {
            url,
            active,
            ready,
            stop,
            address,
            thread,
        })
    }
    fn finish(self) {
        // Wake a blocking accept if cancellation prevented any expected request.
        // No polling sleep can contaminate the measured normal-request latency.
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(100));
        self.thread.join().expect("Local server panicked");
    }
}
fn serve(mut stream: TcpStream, profile: Profile, ready: &AtomicUsize) {
    stream.set_nodelay(true).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut request = Vec::new();
    let mut byte = [0u8; 1];
    while !request.ends_with(b"\r\n\r\n") && request.len() < 4096 {
        if stream.read_exact(&mut byte).is_err() {
            return;
        }
        request.push(byte[0]);
    }
    if stream
        .write_all(b"HTTP/1.0 200 OK\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return;
    }
    ready.fetch_add(1, Ordering::SeqCst);
    if !matches!(profile, Profile::Fast) {
        let ticks = if matches!(profile, Profile::Cancellation) {
            100
        } else {
            BODY_TICKS
        };
        for _ in 0..ticks {
            if stream.write_all(b"x").is_err() {
                break;
            }
            thread::sleep(BODY_TICK);
        }
    }
}
fn stats(samples: &[f64]) -> Value {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    json!({"samples":sorted.len(),"median_ms":sorted[sorted.len()/2],
        "min_ms":sorted[0],"max_ms":sorted[sorted.len()-1]})
}
fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}
fn http_case(
    rt: &tokio::runtime::Runtime,
    name: &str,
    profile: Profile,
    policy: ReadPolicy,
    n: usize,
) -> Result<Value> {
    let server = Server::start(2 * n, profile)?;
    let mut before = Vec::new();
    let mut after = Vec::new();
    for iteration in 0..n {
        // Alternate ordering to avoid assigning all warm/cold runs to one side.
        for current in if iteration % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let start = Instant::now();
            let result = if current {
                rt.block_on(http::get_async(
                    server.url.clone(),
                    Ipv4Addr::LOCALHOST,
                    "gdut-net-perf",
                    SLOW_TIMEOUT,
                    64 * 1024,
                    policy,
                ))
            } else {
                before_get(&server.url, SLOW_TIMEOUT)
            };
            let elapsed = ms(start);
            if current && matches!(profile, Profile::Slow) && policy == ReadPolicy::Complete {
                ensure!(
                    result.is_err(),
                    "Slow complete response must hit its total deadline"
                );
            } else {
                ensure!(
                    result?.status == 200,
                    "Local HTTP response was not successful"
                );
            }
            if current {
                after.push(elapsed);
            } else {
                before.push(elapsed);
            }
        }
    }
    server.finish(); // Response-worker cleanup is deliberately outside request latency.
    Ok(
        json!({"case":name,"kind":"real_loopback_tcp","before":stats(&before),"after":stats(&after)}),
    )
}
fn decoder_cases() -> Vec<Value> {
    let chunk = [b'x'; 16];
    let mut before = Vec::new();
    let mut after = Vec::new();
    for _ in 0..7 {
        let mut legacy = BeforeDecoder::default();
        let start = Instant::now();
        for _ in 0..(60 * 1024 / chunk.len()) {
            black_box(legacy.feed(black_box(&chunk)));
        }
        before.push(ms(start));
        let mut current = FrameDecoder::default();
        let start = Instant::now();
        for _ in 0..(60 * 1024 / chunk.len()) {
            black_box(current.feed(black_box(&chunk)).unwrap());
        }
        after.push(ms(start));
    }
    let chunk = [b'x'; 4096];
    let mut legacy = BeforeDecoder::default();
    for _ in 0..(4 * 1024 * 1024 / chunk.len()) {
        black_box(legacy.feed(&chunk));
    }
    let mut current = FrameDecoder::default();
    let mut accepted = 0;
    let rejected_at = loop {
        if current.feed(&chunk).is_err() {
            break accepted + chunk.len();
        }
        accepted += chunk.len();
        assert!(accepted <= MAX_FRAME_BYTES);
    };
    vec![
        json!({"case":"ipc_fragmented_frame_60kib","kind":"actual_decoders",
        "fragment_bytes":16,"before":stats(&before),"after":stats(&after)}),
        json!({"case":"ipc_unterminated_frame","kind":"actual_decoders_not_process_rss",
            "before_buffered_bytes":legacy.buf.len(),"after_accepted_bytes_before_rejection":accepted,
            "after_rejected_at_input_bytes":rejected_at,"configured_limit_bytes":MAX_FRAME_BYTES}),
    ]
}
async fn cancellation_case(current: bool) -> Result<Value> {
    let server = Server::start(8, Profile::Cancellation)?;
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let url = server.url.clone();
        tasks.push(tokio::spawn(async move {
            if current {
                http::get_async(
                    url,
                    Ipv4Addr::LOCALHOST,
                    "gdut-net-perf",
                    Duration::from_secs(6),
                    64 * 1024,
                    ReadPolicy::Complete,
                )
                .await
            } else {
                tokio::task::spawn_blocking(move || before_get(&url, Duration::from_secs(6)))
                    .await
                    .expect("Baseline blocking task panicked")
            }
        }));
    }
    let established = tokio::time::timeout(Duration::from_secs(2), async {
        while server.ready.load(Ordering::SeqCst) < 8 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    if established.is_err() {
        for task in &tasks {
            task.abort();
        }
        server.finish();
        anyhow::bail!("Eight local response streams did not become ready");
    }
    for task in &tasks {
        task.abort();
    }
    for task in tasks {
        let result = task.await;
        ensure!(
            result.as_ref().is_err_and(|e| e.is_cancelled()),
            "Local request finished before deliberate cancellation: {result:?}"
        );
    }
    tokio::time::sleep(Duration::from_millis(150)).await;
    let active = server.active.load(Ordering::SeqCst);
    server.finish();
    Ok(
        json!({"requests":8,"all_response_headers_sent_before_cancel":true,
        "observation_ms_after_cancel":150,"server_connections_still_active":active}),
    )
}
fn scheduling_case(rt: &tokio::runtime::Runtime) -> Value {
    let mut before = Vec::new();
    let mut after = Vec::new();
    for _ in 0..3 {
        let start = Instant::now();
        rt.block_on(async {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            tokio::time::sleep(Duration::from_millis(3000)).await;
        });
        before.push(ms(start));
        let start = Instant::now();
        rt.block_on(async {
            tokio::join!(
                tokio::time::sleep(Duration::from_millis(1500)),
                tokio::time::sleep(Duration::from_millis(3000))
            );
        });
        after.push(ms(start));
    }
    json!({"case":"probe_timeout_scheduling","kind":"timer_model_not_real_icmp",
        "icmp_model_ms":1500,"http_model_ms":3000,"before":stats(&before),"after":stats(&after)})
}
fn main() -> Result<()> {
    eprintln!("Loopback simulation only; installed services and adapters are untouched.");
    let mut cases = decoder_cases();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    for (name, profile, policy, n) in [
        ("http_normal", Profile::Fast, ReadPolicy::Complete, 30),
        (
            "http_slow_body_total_deadline",
            Profile::Slow,
            ReadPolicy::Complete,
            5,
        ),
        (
            "http_probe_headers_early_return",
            Profile::Slow,
            ReadPolicy::AcceptPartial,
            5,
        ),
    ] {
        eprintln!("Measuring {name}");
        cases.push(http_case(&rt, name, profile, policy, n)?);
    }
    eprintln!("Measuring actual socket lifetime after cancellation");
    cases.push(json!({"case":"cancelled_requests","kind":"real_loopback_tcp",
        "before":rt.block_on(cancellation_case(false))?,"after":rt.block_on(cancellation_case(true))?}));
    eprintln!("Measuring sequential/parallel timer model (about 23 seconds)");
    cases.push(scheduling_case(&rt));
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "platform":std::env::consts::OS,"architecture":std::env::consts::ARCH,
            "baseline_revision":"bc9a345","current_logic":"workspace production HTTP/decoder",
            "baseline_scope":"changed algorithms reproduced with current compiler and dependencies; not complete old/new executables",
            "cases":cases
        }))?
    );
    Ok(())
}
