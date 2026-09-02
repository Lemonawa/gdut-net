//! Dr.COM 心跳 UDP 会话循环（兼容模式运行时）。
//!
//! 报文构造/解析全部来自 [`super::spec`]（纯字节操作）；本模块只负责 IO 编排：
//! 绑定物理适配器源 IP 的 61440 端口 → KA1 握手学 seed/host_ip → KA2 密钥交换
//! → 周期性 KA2 type1/type3 往返。端口 61440 被官方客户端占用视为兼容模式
//! 不可用（Rule：报错而非静默）——本函数 bind 失败即返回 Err；装配层（runtime）
//! 每 60s 重试，用户关掉官方客户端后自愈。
//!
//! 仅 Windows 编译（消费 ras/adapter 同族 Win32 胶水的运行环境），
//! 以 `cargo check --target x86_64-pc-windows-msvc` 验证。

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::thread::sleep;
use std::time::{Duration, Instant};

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::spec::{self, Flag, Key, Seed};
use crate::ipc::protocol::HeartbeatStatus;

/// recv 单次超时。
const RECV_TIMEOUT: Duration = Duration::from_secs(2);
/// KA1 pkt2 之后到 KA2 pkt1 的间隔。
const KA1_TO_KA2_DELAY: Duration = Duration::from_secs(3);
/// 连续 recv 超时容忍次数，超过则重置本轮（cnt 不重置）。
const RECV_TIMEOUT_LIMIT: u8 = 5;
/// 绑定失败提示（端口 61440 被官方客户端占用）。
const BIND_ERR_MSG: &str = "Port 61440 in use, compatibility mode unavailable";

/// 单轮 KA 交换内的一次 recv，带超时。
/// 连续 [`RECV_TIMEOUT_LIMIT`] 次超时后放弃（返回 None）；收到数据立即返回。
/// 每次醒来都检查 `stop`，保证取消后尽快返回。
fn recv_retry(sock: &UdpSocket, stop: &CancellationToken) -> Option<Vec<u8>> {
    let mut buf = [0u8; 512];
    for i in 1..=RECV_TIMEOUT_LIMIT {
        if stop.is_cancelled() {
            return None;
        }
        match sock.recv(&mut buf) {
            Ok(n) => return Some(buf[..n].to_vec()),
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                log::debug!("Heartbeat recv timeout ({i}/{RECV_TIMEOUT_LIMIT})");
            }
            Err(e) => {
                log::warn!("Heartbeat recv failed: {e}");
                return None;
            }
        }
    }
    log::warn!("Heartbeat: {RECV_TIMEOUT_LIMIT} consecutive recv timeouts, resetting round");
    None
}

/// 心跳主循环（阻塞）。返回 Err 表示兼容模式不可用或不可恢复的会话错误。
///
/// 状态机按 spec：每轮 `ka1_pkt1 → parse（学 seed/host_ip/flag）→ ka1_pkt2
/// （首次 first=true）→ sleep 3s → 新 rand → ka2_pkt1（响应为文件报文则学
/// flag 重发一次）→ parse 学 key → 新 rand → ka2_pkt2 → 双计数器 next_cnt
/// → sleep(interval-3s)`。连续 [`RECV_TIMEOUT_LIMIT`] 次 recv 超时则放弃
/// 本轮重头再来（计数器不重置——服务器以计数器识别轮次）。
///
/// `stop` 取消后 ≤1 个 interval 内退出（每轮开头与 recv 分片间检查，
/// 长睡眠以 100ms 分片断点）。
pub fn run_blocking(
    server: Ipv4Addr,
    port: u16,
    src_ip: Ipv4Addr,
    interval: Duration,
    stop: CancellationToken,
    status_tx: watch::Sender<HeartbeatStatus>,
) -> Result<(), String> {
    let sock = UdpSocket::bind((src_ip, port)).map_err(|e| format!("{BIND_ERR_MSG}: {e}"));
    let sock = match sock {
        Ok(s) => s,
        Err(msg) => {
            log::error!("Heartbeat: {msg}");
            let _ = status_tx.send(HeartbeatStatus::Error(BIND_ERR_MSG.to_string()));
            return Err(msg);
        }
    };
    let _ = status_tx.send(HeartbeatStatus::Running);
    let peer = SocketAddr::from((server, port));
    if let Err(e) = sock.connect(peer) {
        let msg = format!("connect {peer} failed: {e}");
        log::error!("Heartbeat: {msg}");
        let _ = status_tx.send(HeartbeatStatus::Error(msg.clone()));
        return Err(msg);
    }
    if let Err(e) = sock.set_read_timeout(Some(RECV_TIMEOUT)) {
        let msg = format!("Failed to set recv timeout: {e}");
        log::error!("Heartbeat: {msg}");
        let _ = status_tx.send(HeartbeatStatus::Error(msg.clone()));
        return Err(msg);
    }

    let mut ka1_cnt: u8 = 0x01;
    let mut ka2_cnt: u8 = 0;
    let mut ka1_flag: Flag = [0x00, 0x00];
    let mut ka2_flag: Flag = [0x00, 0x00];

    loop {
        if stop.is_cancelled() {
            log::info!("Heartbeat: stop signal received, exiting session loop");
            return Ok(());
        }
        // ---- KA1：探测 + 握手（学 seed / host_ip / 可选 flag）----
        let pkt1 = spec::ka1_pkt1(ka1_cnt);
        if let Err(e) = sock.send(&pkt1) {
            let msg = format!("ka1_pkt1 send failed: {e}");
            log::error!("Heartbeat: {msg}");
            let _ = status_tx.send(HeartbeatStatus::Error(msg.clone()));
            return Err(msg);
        }
        let init = match recv_retry(&sock, &stop) {
            Some(resp) => match spec::parse_ka1_resp(&resp) {
                Some(init) => init,
                None => {
                    log::warn!(
                        "Heartbeat: failed to parse ka1 response ({} bytes), resetting round",
                        resp.len()
                    );
                    continue;
                }
            },
            None => {
                log::warn!("Heartbeat: ka1_pkt1 response timeout, resetting round");
                continue;
            }
        };
        let seed: Seed = init.seed;
        let host_ip = init.host_ip;
        if let Some(f) = init.flag {
            ka1_flag = f;
        }
        if let Err(e) = sock.send(&spec::ka1_pkt2(ka1_cnt, true, host_ip, seed, ka1_flag)) {
            let msg = format!("ka1_pkt2 send failed: {e}");
            log::error!("Heartbeat: {msg}");
            let _ = status_tx.send(HeartbeatStatus::Error(msg.clone()));
            return Err(msg);
        }
        //（首个心跳 first=true；后续轮次在本简化实现中同样用 true——上游
        // drcom-generic 对 keepalive 每轮独立握手均标记 first，见 ADR-0002。）

        // ---- KA2：密钥交换 + 确认 ----
        if !sleep_interruptible(KA1_TO_KA2_DELAY, &stop) {
            log::info!("Heartbeat: stop signal, exiting session loop");
            return Ok(());
        }
        let mut key: Key = [0u8; 4];
        let rand_num = new_rand();
        if let Err(e) = sock.send(&spec::ka2_pkt1(ka2_cnt, ka2_flag, rand_num, key)) {
            let msg = format!("ka2_pkt1 send failed: {e}");
            log::error!("Heartbeat: {msg}");
            let _ = status_tx.send(HeartbeatStatus::Error(msg.clone()));
            return Err(msg);
        }
        let resp = match recv_retry(&sock, &stop) {
            Some(r) => r,
            None => {
                log::warn!("Heartbeat: ka2_pkt1 no response, resetting round");
                continue;
            }
        };
        // 文件报文（pkt[0]==0x07 && pkt[2]==0x10）：携带最新 flag，学习后重发一次 ka2_pkt1。
        if resp.first() == Some(&0x07) && resp.get(2) == Some(&0x10) {
            match spec::parse_ka2_resp(&resp).and_then(|r| r.flag) {
                Some(f) => {
                    ka2_flag = f;
                    log::info!(
                        "Heartbeat: received file packet, learned flag={:02x?}, resending",
                        ka2_flag
                    );
                }
                None => log::warn!(
                    "Heartbeat: file packet missing flag field ({} bytes), keeping old flag",
                    resp.len()
                ),
            }
            let rand_num = new_rand();
            if let Err(e) = sock.send(&spec::ka2_pkt1(ka2_cnt, ka2_flag, rand_num, key)) {
                let msg = format!("ka2_pkt1 (retry) send failed: {e}");
                log::error!("Heartbeat: {msg}");
                let _ = status_tx.send(HeartbeatStatus::Error(msg.clone()));
                return Err(msg);
            }
            let resp = match recv_retry(&sock, &stop) {
                Some(r) => r,
                None => {
                    log::warn!("Heartbeat: ka2_pkt1 (retry) no response, resetting round");
                    continue;
                }
            };
            match spec::parse_ka2_resp(&resp) {
                Some(r) => {
                    key = r.key;
                    if let Some(f) = r.flag {
                        ka2_flag = f;
                    }
                }
                None => {
                    log::warn!(
                        "Heartbeat: failed to parse ka2 (retry) response ({} bytes), resetting round",
                        resp.len()
                    );
                    continue;
                }
            }
        } else {
            match spec::parse_ka2_resp(&resp) {
                Some(r) => {
                    key = r.key;
                    if let Some(f) = r.flag {
                        ka2_flag = f;
                    }
                }
                None => {
                    log::warn!(
                        "Heartbeat: failed to parse ka2 response ({} bytes), resetting round",
                        resp.len()
                    );
                    continue;
                }
            }
        }

        let rand_num = new_rand();
        if let Err(e) = sock.send(&spec::ka2_pkt2(ka2_cnt, ka2_flag, rand_num, key, host_ip)) {
            let msg = format!("ka2_pkt2 send failed: {e}");
            log::error!("Heartbeat: {msg}");
            let _ = status_tx.send(HeartbeatStatus::Error(msg.clone()));
            return Err(msg);
        }

        // 本轮完成：双计数器推进，等待下一轮（KA2 往返本身占掉 3s）。
        ka1_cnt = spec::next_cnt(ka1_cnt);
        ka2_cnt = spec::next_cnt(ka2_cnt);
        if !sleep_interruptible(interval.saturating_sub(KA1_TO_KA2_DELAY), &stop) {
            log::info!("Heartbeat: stop signal, exiting session loop");
            return Ok(());
        }
    }
}

/// 可中断睡眠：每 100ms 检查一次 stop；返回 false 表示已取消。
fn sleep_interruptible(total: Duration, stop: &CancellationToken) -> bool {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        if stop.is_cancelled() {
            return false;
        }
        let step = Duration::from_millis(100).min(deadline - Instant::now());
        sleep(step);
    }
    !stop.is_cancelled()
}

/// 每轮新随机 16 位（KA2 rand，大端入报文）。
fn new_rand() -> u16 {
    use rand::RngExt as _;
    rand::rng().random_range(0u16..=u16::MAX)
}
