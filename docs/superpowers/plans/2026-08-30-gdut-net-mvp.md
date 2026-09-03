# gdut-net MVP 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 广工（大学城）有线网第三方认证客户端：PPPoE 拨号守护 + 两级掉线探测 + 可选 Dr.COM 心跳兼容模式（默认关）+ 托盘小 UI，Windows 服务常驻，与 TUN 共存。

**Architecture:** 单 Rust 二进制多子命令。服务进程（`gdut-net run`，Session 0）持有 RAS 会话、守护状态机、可选心跳、命名管道 IPC 服务端；托盘进程（`gdut-net tray`，用户会话）常驻托盘图标 + 原生菜单，状态面板按需创建 egui 窗口。纯逻辑（协议、退避、配置、IPC 协议）与平台胶水（RAS/DPAPI/服务/toast）严格分层，前者 Linux 上 TDD，后者 `cargo check --target x86_64-pc-windows-msvc` + CI windows-latest 验证。

**Tech Stack:** Rust 1.97（edition 2021）、tokio（服务侧异步）、windows 0.62（RAS/DPAPI/IpHelper/EventLog/WinRT toast）、windows-service 0.8、flexi_logger 0.31（大小滚动）、clap 4、serde + toml 1.x、serde_json、tray-icon 0.24 + eframe 0.36（glow）、tauri-winrt-notification 0.8、md-5/sha1/md4 0.11、rand 0.10、base64 0.22、thiserror、anyhow、rpassword、async-trait。

**参考文档:** `CONTEXT.md`（术语）、`docs/adr/0001..0004`（决策）、`gdut-drcom-client-prompt.md`（需求）、心跳协议 spec 见 Task 4 注释与 ADR-0002。

## Global Constraints

- 目标平台：Windows 11 x64；开发机 Linux。纯逻辑模块（`backoff`、`config`、`heartbeat::spec`、`ipc::protocol`、`crypto` blob 格式、`probe` 判定函数、`watchdog` 状态机）不得 `use windows::...`，单元测试必须在 Linux 通过。
- Windows 绑定代码每个任务必须 `cargo check --target x86_64-pc-windows-msvc` 通过。
- 密码不明文落盘：DPAPI 机器级 + 32B 随机 entropy（HKLM 注册表）。
- 心跳默认关（`[heartbeat] enabled = false`）；发包全部绑物理适配器 IP；本地 bind 61440 失败必须报错进通知，不得静默。
- 掉线判定两级：网关 ICMP → HTTP 复核，双失败才重拨；连续 2 次探测异常才判 Drop。
- 错误 691 走慢速重试（600s）；其余指数退避 1s→300s 封顶，稳定 300s 重置。
- 服务内存 <50MB，托盘空闲 <30MB。
- 日志 flexi_logger 5MB×5 份滚动；含认证结果/掉线原因/重拨历史。
- git 分支 `main`，一任务一提交。开发机 git 配置了 GPG 签名但签名可能不可用：**所有 commit 用 `git -c commit.gpgsign=false commit`**。
- 平台 API 签名以 docs.rs/windows 0.62 为准微调（计划中的 win 代码是行为契约参考，不是逐字模板），但**纯逻辑测试断言（协议向量、退避、状态机）不得更改**。

---

### Task 1: 项目脚手架 + CLI 骨架

**Files:**
- Create: `Cargo.toml`、`src/lib.rs`、`src/main.rs`、`src/cli.rs`

**Interfaces:**
- Produces: `gdut-net` 二进制子命令 `run/install/uninstall/status/tray`；`gdut_net::cli::dispatch()`；lib+bin 双 target。

- [ ] **Step 1: 写 Cargo.toml**

```toml
[package]
name = "gdut-net"
version = "0.1.0"
edition = "2021"

[lib]
name = "gdut_net"
path = "src/lib.rs"

[[bin]]
name = "gdut-net"
path = "src/main.rs"

[dependencies]
tokio = { version = "1.53", features = ["rt-multi-thread", "macros", "sync", "time", "net", "io-util"] }
clap = { version = "4.6", features = ["derive"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
toml = "1.1"
thiserror = "2.0"
anyhow = "1.0"
log = "0.4"
flexi_logger = "0.31"
rpassword = "7.3"
async-trait = "0.1"
base64 = "0.22"

[target.'cfg(windows)'.dependencies]
windows = { version = "0.62", features = [
  "Win32_Foundation",
  "Win32_NetworkManagement_Rras",
  "Win32_NetworkManagement_IpHelper",
  "Win32_NetworkManagement_Ndis",
  "Win32_Networking_WinSock",
  "Win32_Security_Cryptography",
  "Win32_System_Registry",
  "Win32_System_EventLog",
  "Win32_System_Services",
  "Win32_System_Memory",
  "Win32_UI_Shell",
  "Win32_UI_WindowsAndMessaging",
] }
windows-service = "0.8"
tauri-winrt-notification = "0.8"
md-5 = "0.11"
sha1 = "0.11"
md4 = "0.11"
rand = "0.10"
tray-icon = "0.24"
eframe = { version = "0.36", default-features = false, features = ["glow"] }

[profile.release]
opt-level = "z"
lto = true
strip = true
```

- [ ] **Step 2: 写 src/lib.rs、src/main.rs、src/cli.rs**

```rust
// src/lib.rs
pub mod cli;
```

```rust
// src/main.rs
fn main() -> anyhow::Result<()> {
    gdut_net::cli::dispatch()
}
```

```rust
// src/cli.rs
use anyhow::{bail, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "gdut-net", version, about = "GDUT 有线网第三方认证客户端")]
pub struct Cli {
    #[arg(long, global = true, default_value = r"C:\ProgramData\gdut-net\config.toml")]
    pub config: std::path::PathBuf,

    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// 以 Windows 服务方式运行（内部使用）
    Run,
    /// 安装服务、创建拨号条目、写配置
    Install,
    /// 卸载并清理
    Uninstall,
    /// 显示当前状态
    Status,
    /// 启动托盘（用户会话）
    Tray,
}

pub fn dispatch() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Run => bail!("Task 11 实现"),
        Cmd::Install => bail!("Task 11 实现"),
        Cmd::Uninstall => bail!("Task 11 实现"),
        Cmd::Status => bail!("Task 13 实现"),
        Cmd::Tray => bail!("Task 14 实现"),
    }
}
```

- [ ] **Step 3: 验证**

Run: `cargo build && cargo clippy -- -D warnings && cargo fmt --check`
Expected: 通过。

Run: `cargo check --target x86_64-pc-windows-msvc`
Expected: 通过。

- [ ] **Step 4: Commit**

```bash
git add -A && git -c commit.gpgsign=false commit -m "chore: 项目脚手架与 CLI 骨架"
```

---

### Task 2: backoff.rs（指数退避，纯）

**Files:**
- Create: `src/backoff.rs`；Modify: `src/lib.rs`（+`pub mod backoff;`）

**Interfaces:**
- Produces:
  - `Backoff::new(min: Duration, max: Duration) -> Self`
  - `Backoff::next_delay(&mut self) -> Duration`（min, 2min, 4min…封顶 max）
  - `Backoff::reset(&mut self)`
  - `pub const AUTH_FAIL_DELAY: Duration`（=600s，691 慢速路径）

- [ ] **Step 1: 写失败测试**（`src/backoff.rs` 底部 `#[cfg(test)]`）

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_then_caps() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(300));
        assert_eq!(b.next_delay(), Duration::from_secs(1));
        assert_eq!(b.next_delay(), Duration::from_secs(2));
        assert_eq!(b.next_delay(), Duration::from_secs(4));
        let mut last = Duration::ZERO;
        for _ in 0..20 { last = b.next_delay(); }
        assert_eq!(last, Duration::from_secs(300));
    }

    #[test]
    fn reset_restarts() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(300));
        b.next_delay();
        b.reset();
        assert_eq!(b.next_delay(), Duration::from_secs(1));
    }
}
```

- [ ] **Step 2: 确认失败** — Run: `cargo test backoff`；Expected: 编译失败。

- [ ] **Step 3: 实现**

```rust
use std::time::Duration;

pub const AUTH_FAIL_DELAY: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub struct Backoff {
    min: Duration,
    max: Duration,
    attempt: u32,
}

impl Backoff {
    pub fn new(min: Duration, max: Duration) -> Self {
        Self { min, max, attempt: 0 }
    }

    pub fn next_delay(&mut self) -> Duration {
        let d = self.min.saturating_mul(1u32 << self.attempt.min(20));
        self.attempt = self.attempt.saturating_add(1);
        d.min(self.max).max(self.min)
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}
```

- [ ] **Step 4: 确认通过** — Run: `cargo test backoff`；Expected: 2 passed。

- [ ] **Step 5: Commit** — `git add -A && git -c commit.gpgsign=false commit -m "feat: 指数退避策略"`

---

### Task 3: config.rs（TOML schema，纯）

**Files:**
- Create: `src/config.rs`、`tests/config.rs`；Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - `Config { account: Account, dial: Dial, heartbeat: HeartbeatCfg, log: LogCfg }`（derive Serialize/Deserialize/Clone/Debug；字段与默认值见实现）
  - `Config::load(path: &Path) -> Result<Config>`（load 即 validate）
  - `Config::save(&self, path: &Path) -> Result<()>`
  - `Config::validate(&self) -> Result<()>`
  - `Config::sample() -> String`（带注释样例，install 模板）
  - 默认值：`entry_name="gdut"`、`pbk_path=C:\ProgramData\gdut-net\gdut.pbk`、`probe_interval_secs=30`、`http_probe_url=http://9.9.9.9`、`heartbeat.enabled=false`、`heartbeat.module="gdut"`、`heartbeat.server="10.0.3.2"`、`heartbeat.port=61440`、`heartbeat.interval_secs=20`、`log.dir=C:\ProgramData\gdut-net\logs`、`log.max_size_mb=5`、`log.rotate_keep=5`、`log.event_log=false`
- 校验规则：学号非空；`heartbeat.enabled && module != "gdut"` 报错；`probe_interval_secs >= 5`。

- [ ] **Step 1: 写失败测试** `tests/config.rs`：

```rust
use gdut_net::config::Config;

#[test]
fn roundtrip_and_defaults() {
    let cfg: Config = toml::from_str(&Config::sample()).unwrap();
    cfg.validate().unwrap();
    assert_eq!(cfg.dial.entry_name, "gdut");
    assert!(!cfg.heartbeat.enabled);
    assert_eq!(cfg.heartbeat.server, "10.0.3.2");
    assert_eq!(cfg.heartbeat.port, 61440);
    assert_eq!(cfg.dial.probe_interval_secs, 30);
}

#[test]
fn reject_bad_heartbeat_module() {
    let mut cfg: Config = toml::from_str(&Config::sample()).unwrap();
    cfg.heartbeat.enabled = true;
    cfg.heartbeat.module = "unknown".into();
    assert!(cfg.validate().is_err());
}

#[test]
fn reject_short_probe_interval() {
    let mut cfg: Config = toml::from_str(&Config::sample()).unwrap();
    cfg.dial.probe_interval_secs = 1;
    assert!(cfg.validate().is_err());
}
```

- [ ] **Step 2: 确认失败** — Run: `cargo test --test config`；Expected: 编译失败。

- [ ] **Step 3: 实现 config.rs**：结构如上 Interfaces；实现要点：`load` 读文件→`toml::from_str`→`validate`；`save` 建父目录后 `toml::to_string_pretty`；`sample()` 前置注释两行（`# 大学城认证服务器 10.0.3.2；龙洞/东风路为 10.0.3.6`、`# 心跳默认关闭；开启前必须抓包验证（见 ADR-0002）`）+ 序列化默认 Config（`student_id: "你的学号"`，`password_blob: ""`）。

- [ ] **Step 4: 确认通过** — Run: `cargo test`；Expected: config 3 passed + backoff 2 passed。

- [ ] **Step 5: Commit** — `git add -A && git -c commit.gpgsign=false commit -m "feat: 配置 schema 与校验"`

---

### Task 4: heartbeat/spec.rs（Dr.COM GDUT 报文，纯）

> 协议来源 ADR-0002（gdut-drcom auth.c 与 drcom-generic issue #82 抓包交叉验证）。纯字节操作，无 IO。

**Files:**
- Create: `src/heartbeat/mod.rs`（`pub mod spec;`）、`src/heartbeat/spec.rs`、`tests/heartbeat_spec.rs`；Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - `pub type Seed = [u8; 4]; pub type HostIp = [u8; 4]; pub type Key = [u8; 4]; pub type Flag = [u8; 2];`
  - `pub const PORT: u16 = 61440; pub const KA1_LEN: usize = 96; pub const KA2_LEN: usize = 40;`
  - `pub fn next_cnt(cnt: u8) -> u8`（`(cnt+2)%127`）
  - `pub fn ka1_pkt1(cnt: u8) -> [u8; 8]`（`07 cnt 08 00 01 00 00 00`）
  - `pub struct Ka1Init { seed, host_ip, flag: Option<Flag> }`；`pub fn parse_ka1_resp(pkt: &[u8]) -> Option<Ka1Init>`（`pkt[0]==0x07 && len>=30`；seed=pkt[8..12]；host_ip=pkt[12..16]；`pkt[2]==0x10` 时 flag=Some(pkt[6..8])）
  - `pub fn ka1_pkt2(cnt: u8, first: bool, host_ip: HostIp, seed: Seed, flag: Flag) -> [u8; 96]`（`07 cnt 6000 03 00`+uid 零 6B+`host_ip@12`+`00 62|63 flag@16..20`+`seed@20..24`+`checksum@24..32`）
  - checksum = `seed[0]&3` → 0:`le32(20000711)+le32(126)` / 1:MD5 挑`[2,3,8,9,5,6,13,14]` / 2:MD4 挑`[1,2,8,9,4,5,11,12]` / 3:SHA1 挑`[2,3,9,10,5,6,15,16]`
  - `pub fn ka2_checksum(pkt: &[u8; 40]) -> [u8; 4]`（16 位小端字全包 XOR → `&0xffff` → `*0x2c7` → le 写出）
  - `pub fn ka2_pkt1(cnt: u8, flag: Flag, rand: u16, key: Key) -> [u8; 40]`（`07 cnt 2800 0b01 flag rand@8..10 零6B key@16..20 零`）
  - `pub struct Ka2Resp { key: Key, flag: Option<Flag> }`；`pub fn parse_ka2_resp(pkt: &[u8]) -> Option<Ka2Resp>`（`pkt[0]==0x07 && len>=20`；key=pkt[16..20]；`pkt[2]==0x10` 即文件报文，flag=Some(pkt[6..8])）
  - `pub fn ka2_pkt2(cnt: u8, flag: Flag, rand: u16, key: Key, host_ip: HostIp) -> [u8; 40]`（`07 cnt 2800 0b03 flag rand 零6B key 零4B crc@24..28 host_ip@28..32 零8B`；crc 对校验位全零的全包计算）

- [ ] **Step 1: 写失败测试** `tests/heartbeat_spec.rs`：

```rust
use gdut_net::heartbeat::spec::*;

#[test]
fn ka1_pkt1_layout() {
    assert_eq!(ka1_pkt1(1), [0x07, 1, 0x08, 0x00, 0x01, 0, 0, 0]);
}

#[test]
fn parse_ka1_resp_issue82_capture() {
    // issue #82 真实抓包（file packet 形态）
    let pkt = [
        0x07u8, 0x6f, 0x10, 0x00, 0x02, 0x03, 0x00, 0x00, 0xa3, 0xe2, 0xf3, 0x00, 0x0a, 0x1e,
        0x84, 0xa7, 0xa8, 0xa8, 0x00, 0x00, 0xe6, 0x59, 0xf1, 0x67, 0x00, 0x00, 0x00, 0x00,
        0xdc, 0x02,
    ];
    let init = parse_ka1_resp(&pkt).unwrap();
    assert_eq!(init.seed, [0xa3, 0xe2, 0xf3, 0x00]);
    assert_eq!(init.host_ip, [0x0a, 0x1e, 0x84, 0xa7]);
    assert_eq!(init.flag, Some([0x00, 0x00]));
}

#[test]
fn ka1_pkt2_checksum_sha1_mode_issue82() {
    // seed=a3e2f300 → 0xa3&3=3 → SHA1；抓包校验值 9ae9cef84b020aa3
    let pkt = ka1_pkt2(1, true, [0x0a, 0x1e, 0x84, 0xa7], [0xa3, 0xe2, 0xf3, 0x00], [0x2a, 0x00]);
    assert_eq!(&pkt[0..5], &[0x07, 1, 0x60, 0x00, 0x03]);
    assert_eq!(&pkt[17..18], &[0x62]);
    assert_eq!(&pkt[20..24], &[0xa3, 0xe2, 0xf3, 0x00]);
    assert_eq!(&pkt[24..32], &[0x9a, 0xe9, 0xce, 0xf8, 0x4b, 0x02, 0x0a, 0xa3]);
}

#[test]
fn crypt_mode_selection() {
    assert_eq!(crypt_bytes(&[0x00, 0, 0, 0]), plain_bytes());
    assert_eq!(crypt_bytes(&[0x01, 0, 0, 0]), md5_bytes(&[0x01, 0, 0, 0]));
    assert_eq!(crypt_bytes(&[0x02, 0, 0, 0]), md4_bytes(&[0x02, 0, 0, 0]));
    assert_eq!(crypt_bytes(&[0xa3, 0, 0, 0]), sha1_bytes(&[0xa3, 0, 0, 0]));
}

#[test]
fn ka2_pkt2_layout_and_crc() {
    let pkt = ka2_pkt2(3, [0xdc, 0x02], 0x03e9, [0x43, 0xe1, 0xf3, 0x00], [0x0a, 0x1e, 0x84, 0xa7]);
    assert_eq!(pkt[0], 0x07);
    assert_eq!(pkt[1], 3);
    assert_eq!(&pkt[2..4], &[0x28, 0x00]);
    assert_eq!(&pkt[4..6], &[0x0b, 0x03]);
    assert_eq!(&pkt[6..8], &[0xdc, 0x02]);
    assert_eq!(&pkt[8..10], &[0x03, 0xe9]);
    assert_eq!(&pkt[16..20], &[0x43, 0xe1, 0xf3, 0x00]);
    // CRC 自洽：清零校验位重算一致
    let mut p2 = pkt;
    p2[24..28].fill(0);
    assert_eq!(&pkt[24..28], &ka2_checksum(&p2));
}

#[test]
fn ka2_pkt1_layout() {
    let pkt = ka2_pkt1(0, [0x00, 0x00], 0x1234, [0; 4]);
    assert_eq!(&pkt[0..2], &[0x07, 0x00]);
    assert_eq!(&pkt[4..6], &[0x0b, 0x01]);
    assert_eq!(&pkt[8..10], &[0x12, 0x34]);
    assert!(pkt[16..20].iter().all(|&b| b == 0));
}

#[test]
fn file_packet_flag_learning() {
    let mut pkt = [0u8; 40];
    pkt[0] = 0x07;
    pkt[2] = 0x10;
    pkt[6] = 0xdc;
    pkt[7] = 0x02;
    assert_eq!(parse_ka2_resp(&pkt).unwrap().flag, Some([0xdc, 0x02]));
}

#[test]
fn cnt_wraps_below_128() {
    assert_eq!(next_cnt(125), 127);
    assert_eq!(next_cnt(127), 1);
}
```

- [ ] **Step 2: 确认失败** — Run: `cargo test --test heartbeat_spec`；Expected: 编译失败。

- [ ] **Step 3: 实现 spec.rs**（按 Interfaces 逐函数实现；`crypt_bytes`/`plain_bytes`/`md5_bytes`/`md4_bytes`/`sha1_bytes` 设为 `pub` 供测试；哈希用 `md5::Md5`、`md4::Md4`、`sha1::Sha1` 的 `Digest::digest`，取 `d[i]` 按下标挑字节。`ka2_checksum` 用 `u16::from_le_bytes` 逐步 XOR，`(sum & 0xffff).wrapping_mul(0x2c7).to_le_bytes()`。）

- [ ] **Step 4: 确认通过** — Run: `cargo test --test heartbeat_spec`；Expected: 8 passed。若 `ka1_pkt2_checksum_sha1_mode_issue82` 失败：先写独立脚本对 `[0xa3,0xe2,0xf3,0x00]` 算 SHA1 挑下标核对 `9ae9cef84b020aa3`；若下标有误**以抓包值为准**调整下标并在代码注释记录。

- [ ] **Step 5: Commit** — `git add -A && git -c commit.gpgsign=false commit -m "feat: Dr.COM GDUT 心跳报文规格与测试向量"`

---

### Task 5: ipc/protocol.rs（JSON-line，纯）

**Files:**
- Create: `src/ipc/mod.rs`（`pub mod protocol;`）、`src/ipc/protocol.rs`、`tests/ipc_protocol.rs`；Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - `SessionStatus { Idle, Dialing, Connected, Backoff, AuthFail }`（snake_case serde）
  - `HeartbeatStatus { Off, Running, Error(String) }`
  - `StateSnapshot { status, since_unix: Option<u64>, ip: Option<String>, last_drop_reason: Option<String>, redial_attempts: u32, heartbeat: HeartbeatStatus }`
  - `ServerMsg { State { state }, Ack }`、`ClientMsg { Cmd { c: Command } }`、`Command { Redial }`（内部 tag `"t"`，snake_case）
  - `encode_frame<T: Serialize>(msg: &T) -> Vec<u8>`（JSON+\n）
  - `FrameDecoder::default()`；`feed(&mut self, chunk: &[u8]) -> Vec<Vec<u8>>`（半包缓存）

- [ ] **Step 1: 写失败测试** `tests/ipc_protocol.rs`：

```rust
use gdut_net::ipc::protocol::*;

#[test]
fn state_msg_roundtrip() {
    let snap = StateSnapshot {
        status: SessionStatus::Connected,
        since_unix: Some(1756500000),
        ip: Some("10.30.132.167".into()),
        last_drop_reason: None,
        redial_attempts: 0,
        heartbeat: HeartbeatStatus::Off,
    };
    let bytes = encode_frame(&ServerMsg::State { state: snap.clone() });
    assert!(bytes.ends_with(b"\n"));
    let mut dec = FrameDecoder::default();
    let frames = dec.feed(&bytes);
    assert_eq!(frames.len(), 1);
    let msg: ServerMsg = serde_json::from_slice(&frames[0]).unwrap();
    assert_eq!(msg, ServerMsg::State { state: snap });
}

#[test]
fn split_partial_frames() {
    let a = encode_frame(&ClientMsg::Cmd { c: Command::Redial });
    let b = encode_frame(&ClientMsg::Cmd { c: Command::Redial });
    let mut dec = FrameDecoder::default();
    let mut all = a.clone();
    all.extend_from_slice(&b[..b.len() - 1]);
    assert_eq!(dec.feed(&all).len(), 1);
    assert_eq!(dec.feed(&b[b.len() - 1..]).len(), 1);
}

#[test]
fn heartbeat_error_status_serializes() {
    let snap = StateSnapshot {
        status: SessionStatus::Connected,
        since_unix: None,
        ip: None,
        last_drop_reason: None,
        redial_attempts: 0,
        heartbeat: HeartbeatStatus::Error("bind 61440 被占用".into()),
    };
    let bytes = encode_frame(&ServerMsg::State { state: snap });
    assert!(String::from_utf8_lossy(&bytes).contains("bind 61440"));
}
```

- [ ] **Step 2: 确认失败** — Run: `cargo test --test ipc_protocol`；Expected: 编译失败。

- [ ] **Step 3: 实现 protocol.rs**（按 Interfaces；`FrameDecoder { buf: Vec<u8> }` 用 `position(|&b| b == b'\n')` + `drain(..=pos)`）。

- [ ] **Step 4: 确认通过** — Run: `cargo test --test ipc_protocol`；Expected: 3 passed。

- [ ] **Step 5: Commit** — `git add -A && git -c commit.gpgsign=false commit -m "feat: IPC JSON-line 协议"`

---

### Task 6: adapter.rs（物理适配器识别）

**Files:**
- Create: `src/adapter.rs`；Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - 纯：`pub fn is_virtual(name_or_desc: &str) -> bool`（关键字表：wintun/tun/tap/tailscale/clash/wireguard/hyper-v/vmware/virtualbox/vethernet/loopback/wan miniport，大小写不敏感）
  - `pub struct AdapterInfo { name: String, ipv4: Ipv4Addr, gateway: Option<Ipv4Addr> }`
  - `#[cfg(windows)] pub fn physical_adapter() -> Result<AdapterInfo>`：`GetAdaptersAddresses(AF_INET, GAA_FLAG_INCLUDE_GATEWAYS)`，取 `IfType == IF_TYPE_ETHERNET_CSMACD && OperStatus == Up && !is_virtual(name) && !is_virtual(desc)` 的适配器，优先有网关者
  - `#[cfg(windows)] pub fn ppp_adapter_ip() -> Option<Ipv4Addr>`：`IfType == IF_TYPE_PPP` 的适配器 IPv4

- [ ] **Step 1: 写失败测试**（`src/adapter.rs` 内 `#[cfg(test)]`）：

```rust
#[cfg(test)]
mod tests {
    use super::is_virtual;

    #[test]
    fn flags_known_virtual_adapters() {
        for name in ["wintun", "Tailscale", "Clash TUN", "TAP-Windows Adapter", "Hyper-V Virtual Ethernet", "VMware Virtual Ethernet", "VirtualBox Host-Only"] {
            assert!(is_virtual(name), "{name} 应判虚拟");
        }
    }

    #[test]
    fn physical_names_pass() {
        for name in ["Realtek Gaming GbE", "Intel(R) Ethernet Connection", "以太网"] {
            assert!(!is_virtual(name), "{name} 不应误判");
        }
    }
}
```

- [ ] **Step 2: 确认失败** — Run: `cargo test is_virtual`；Expected: 编译失败。

- [ ] **Step 3: 实现**（纯部分如上；Win32 部分按 docs.rs/windows 0.62 对齐：`GetAdaptersAddresses` 两次调用法（先探缓冲大小），遍历链表取 `FirstUnicastAddress`/`FirstGatewayAddress` 的 `SOCKADDR_IN.sin_addr`（注意 `S_un` 联合体字段名以实际为准），`FriendlyName`/`Description` 是 `PWSTR`。）

- [ ] **Step 4: 验证** — Run: `cargo test is_virtual`（2 passed）+ `cargo check --target x86_64-pc-windows-msvc`（通过）。

- [ ] **Step 5: Commit** — `git add -A && git -c commit.gpgsign=false commit -m "feat: 物理适配器识别与 IP 获取"`

---

### Task 7: ras.rs（RAS 封装）

**Files:**
- Create: `src/ras.rs`；Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - 纯：`pub enum ErrKind { Auth, Transient }`；`pub fn classify(code: u32) -> ErrKind`（691→Auth，其余→Transient）
  - `#[derive(thiserror::Error)] pub enum RasError { Auth, Other { code: u32, msg: String } }`
  - `#[cfg(windows)] pub fn ensure_entry(pbk: &str, name: &str) -> Result<()>`：`RasSetEntryPropertiesW` 幂等创建 `RASET_Broadband` + `RASFP_PPP` + 设备 `WAN Miniport (PPPoE)`/`PPPoE` 条目
  - `#[cfg(windows)] pub fn set_credentials(pbk, name, user, pass) -> Result<()>`：`RasSetCredentialsW`（`RASCM_UserName | RASCM_Password`）
  - `#[cfg(windows)] pub fn dial(pbk, name, user, pass) -> Result<RasSession, RasError>`：`RasDialW` 同步模式（回调 None）
  - `#[cfg(windows)] pub struct RasSession`：`status(&self) -> ConnState`（`RasGetConnectStatusW`；`RASCS_Connected→Connected`，`RASCS_Disconnected→Disconnected`，中间态→Connected 等下轮复核）；`hangup(&self)`（`RasHangUpW` + sleep 300ms 等句柄释放）
  - `#[cfg(windows)] pub fn error_string(code: u32) -> String`：`RasGetErrorStringW`

- [ ] **Step 1: 写失败测试**（`src/ras.rs` 内）：

```rust
#[cfg(test)]
mod tests {
    use super::{classify, ErrKind};

    #[test]
    fn classify_691_as_auth_others_transient() {
        assert_eq!(classify(691), ErrKind::Auth);
        assert_eq!(classify(651), ErrKind::Transient);
        assert_eq!(classify(678), ErrKind::Transient);
        assert_eq!(classify(619), ErrKind::Transient);
        assert_eq!(classify(99999), ErrKind::Transient);
    }
}
```

- [ ] **Step 2: 确认失败** — Run: `cargo test classify`；Expected: 编译失败。（ErrKind 需 derive PartialEq, Eq, Debug）

- [ ] **Step 3: 实现**（纯部分如上；Win32 部分注意：`RASENTRYW`/`RASDIALPARAMSW`/`RASCONNW`/`RASCREDENTIALSW` 先 `default()` 再设 `dwSize = size_of::<T>() as u32`；字符串字段是定长 `[u16; N]` 数组，写辅助函数 `fn wide(s: &str, len: usize) -> Vec<u16>`（null 结尾截断）。`RasDialW` 的回调参数传 `None`。）

- [ ] **Step 4: 验证** — Run: `cargo test classify`（1 passed）+ `cargo check --target x86_64-pc-windows-msvc`（通过）。

- [ ] **Step 5: Commit** — `git add -A && git -c commit.gpgsign=false commit -m "feat: RAS API 封装（条目/凭据/拨号/状态/挂断）"`

---

### Task 8: probe.rs（两级探测）

**Files:**
- Create: `src/probe.rs`；Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - `pub enum ProbeVerdict { Alive, LinkDown, Kicked }`
  - `pub struct ProbeCfg { interval: Duration, http_url: String }`
  - 纯：`pub fn verdict_from_http(saw_redirect: bool, nexturl_is_auth: bool) -> ProbeVerdict`（302+认证页→Kicked，否则 Alive）
  - `#[cfg(windows)] pub async fn probe_once(src_ip: Ipv4Addr, gateway: Option<Ipv4Addr>, http_url: &str) -> ProbeVerdict`：每次都走两级并综合判定（ADR-0003）——网关 ICMP（`IcmpCreateFile`/`IcmpSendEcho2Ex` 绑源 IP，超时 1500ms）探链路 + HTTP GET（`std::net::TcpStream::bind((src_ip, 0))` 后 connect，3s 超时，读响应头判 302 + Location 含 `wlanacip|nexturl|portal`）探被踢；纯函数 `combine(icmp_ok: Option<bool>, http: Option<ProbeVerdict>) -> ProbeVerdict` 综合：HTTP 302+认证页→Kicked（无论 ICMP——被踢时网关仍通）；ICMP 通且非 Kicked→Alive；ICMP 失败且 HTTP 失败→LinkDown；ICMP 失败但 HTTP Alive→Alive（保守在线）

- [ ] **Step 1: 写失败测试**（`src/probe.rs` 内）：

```rust
#[cfg(test)]
mod tests {
    use super::{verdict_from_http, ProbeVerdict};

    #[test]
    fn redirect_to_auth_page_means_kicked() {
        assert_eq!(verdict_from_http(true, true), ProbeVerdict::Kicked);
        assert_eq!(verdict_from_http(true, false), ProbeVerdict::Alive);
        assert_eq!(verdict_from_http(false, false), ProbeVerdict::Alive);
    }
}
```

- [ ] **Step 2: 确认失败** — Run: `cargo test verdict_from_http`；Expected: 编译失败。

- [ ] **Step 3: 实现**（纯部分如上；Win32 部分 ICMP 参数为网络序 u32（`u32::from(ip).to_be()` 不对——用 `htonl` 等价 `ip.octets()` 反转；以 docs.rs `IcmpSendEcho2Ex` 签名为准）。HTTP 判定逻辑抽出纯函数便于测试。）

- [ ] **Step 4: 验证** — Run: `cargo test verdict_from_http`（1 passed）+ `cargo check --target x86_64-pc-windows-msvc`（通过）。

- [ ] **Step 5: Commit** — `git add -A && git -c commit.gpgsign=false commit -m "feat: 两级掉线探测（ICMP 链路 + HTTP 被踢复核）"`

---

### Task 9: watchdog.rs（守护状态机，trait 注入）

**Files:**
- Create: `src/watchdog.rs`、`tests/watchdog.rs`；Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - `pub struct DialError { kind: ErrKind, code: u32, msg: String }`
  - `#[async_trait] pub trait Dialer: Send { async fn dial(&mut self) -> Result<(), DialError>; async fn hangup(&mut self); fn is_connected(&mut self) -> bool; }`
  - `#[async_trait] pub trait Prober: Send { async fn probe(&mut self) -> ProbeVerdict; }`
  - `pub struct WatchdogCfg { redial_min: Duration, redial_max: Duration, probe_interval: Duration, auth_fail_delay: Duration }`
  - `Watchdog::new(dialer, prober, cfg)`；`snapshot(&self) -> StateSnapshot`（`ip`/`heartbeat` 字段填默认，装配层覆盖）；`run_once(&mut self) -> Duration`（状态机单步，返回建议等待）
  - 状态机：`{Idle,Backoff,AuthFail,Dialing} → dial`；`Connected → probe`。probe 失败 1 次 → 间隔/4 复核；连续 ≥2 次 → hangup + 立即重拨（drop）。dial 成功 → Connected（`since=now`，`probe_fails=0`）；Auth 错 → AuthFail（auth_fail_delay）；Transient → Backoff（backoff.next_delay()）。Connected 稳定累计 ≥300s 且 attempts>0 → backoff.reset + attempts=0。
  - `snapshot().redial_attempts` = attempts；`dial_calls()` 测试辅助。

- [ ] **Step 1: 写失败测试** `tests/watchdog.rs`：

```rust
use std::time::Duration;
use gdut_net::probe::ProbeVerdict;
use gdut_net::watchdog::*;
use gdut_net::ipc::protocol::SessionStatus;
use gdut_net::ras::ErrKind;

#[derive(Default)]
struct MockDialer {
    fail_times: u8,
    dial_calls: std::rc::Rc<std::cell::Cell<u32>>,
    connected: bool,
}

#[async_trait::async_trait]
impl Dialer for MockDialer {
    async fn dial(&mut self) -> Result<(), DialError> {
        self.dial_calls.set(self.dial_calls.get() + 1);
        if self.fail_times > 0 {
            self.fail_times -= 1;
            return Err(DialError { kind: ErrKind::Transient, code: 678, msg: "mock".into() });
        }
        self.connected = true;
        Ok(())
    }
    async fn hangup(&mut self) { self.connected = false; }
    fn is_connected(&mut self) -> bool { self.connected }
}

struct MockProber(Vec<ProbeVerdict>);

#[async_trait::async_trait]
impl Prober for MockProber {
    async fn probe(&mut self) -> ProbeVerdict {
        if self.0.len() > 1 { self.0.remove(0) } else { self.0[0] }
    }
}

fn cfg() -> WatchdogCfg {
    WatchdogCfg {
        redial_min: Duration::from_secs(1),
        redial_max: Duration::from_secs(60),
        probe_interval: Duration::from_secs(30),
        auth_fail_delay: Duration::from_secs(600),
    }
}

#[tokio::test]
async fn dials_then_reports_connected() {
    let mut wd = Watchdog::new(MockDialer::default(), MockProber(vec![ProbeVerdict::Alive]), cfg());
    wd.run_once().await;
    assert_eq!(wd.snapshot().status, SessionStatus::Connected);
}

#[tokio::test]
async fn transient_fail_enters_backoff() {
    let d = MockDialer { fail_times: 2, ..Default::default() };
    let mut wd = Watchdog::new(d, MockProber(vec![ProbeVerdict::Alive]), cfg());
    wd.run_once().await;
    assert_eq!(wd.snapshot().status, SessionStatus::Backoff);
    assert_eq!(wd.snapshot().redial_attempts, 1);
}

#[tokio::test]
async fn double_probe_fail_drops() {
    let mut wd = Watchdog::new(
        MockDialer::default(),
        MockProber(vec![ProbeVerdict::LinkDown, ProbeVerdict::LinkDown]),
        cfg(),
    );
    wd.run_once().await; // dial → Connected
    wd.run_once().await; // probe fail #1（复核，不 drop）
    assert_eq!(wd.snapshot().status, SessionStatus::Connected);
    wd.run_once().await; // probe fail #2 → drop → redial
    assert_eq!(wd.dial_calls(), 2);
}

#[tokio::test]
async fn auth_fail_enters_slow_path() {
    struct AuthDialer;
    #[async_trait::async_trait]
    impl Dialer for AuthDialer {
        async fn dial(&mut self) -> Result<(), DialError> {
            Err(DialError { kind: ErrKind::Auth, code: 691, msg: "denied".into() })
        }
        async fn hangup(&mut self) {}
        fn is_connected(&mut self) -> bool { false }
    }
    let mut wd = Watchdog::new(AuthDialer, MockProber(vec![ProbeVerdict::Alive]), cfg());
    let d = wd.run_once().await;
    assert_eq!(wd.snapshot().status, SessionStatus::AuthFail);
    assert_eq!(d, Duration::from_secs(600));
}
```

注意：`MockDialer` 若需跨 await 共享计数，改用 `Arc<AtomicU32>`（`std::rc::Rc` 不能跨 `Send` bound；trait 要求 `Send`，mock 也须满足——直接把 `dial_calls: Arc<AtomicU32>` 存字段，测试里持 `Arc` 克隆断言）。

- [ ] **Step 2: 确认失败** — Run: `cargo test --test watchdog`；Expected: 编译失败。

- [ ] **Step 3: 实现**（按 Interfaces；内部字段 `phase: Phase`、`backoff: Backoff`、`attempts: u32`、`since: Option<SystemTime>`、`probe_fails: u8`、`stable_for: Duration`。`run_once` 的 Connected 分支：probe `Alive` → `probe_fails=0`、`stable_for += probe_interval`、稳定判定重置；异常 → `probe_fails += 1`，<2 返回 `probe_interval/4`，≥2 hangup+`do_dial()`。`do_dial` 按 Err.kind 分流 Auth/Transient。日志：拨号成功/失败原因/退避时长/掉线判定均 `log::info!/warn!/error!`。）

- [ ] **Step 4: 确认通过** — Run: `cargo test --test watchdog`；Expected: 4 passed。

- [ ] **Step 5: Commit** — `git add -A && git -c commit.gpgsign=false commit -m "feat: 守护状态机（trait 注入可测）"`

---

### Task 10: crypto.rs（DPAPI 密码）

**Files:**
- Create: `src/crypto.rs`、`tests/crypto_blob.rs`；Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - 纯：`pub fn wrap_blob(entropy_hex: &str, protected: &[u8]) -> String`（`GDUT1:<hex>:<base64>`）；`pub fn unwrap_blob(s: &str) -> Option<(String, Vec<u8>)>`
  - `#[cfg(windows)] pub fn protect(plain: &str) -> Result<String>`：32B 随机 entropy（`rand::rng().fill_bytes`）→ `CryptProtectData(CRYPTPROTECT_LOCAL_MACHINE, entropy)` → wrap
  - `#[cfg(windows)] pub fn unprotect(blob: &str) -> Result<String>`
  - `#[cfg(windows)] pub fn ensure_entropy() -> Result<Vec<u8>>`：注册表 `HKLM\SOFTWARE\gdut-net` 值 `entropy`(REG_BINARY) 读/建
  - `#[cfg(windows)] pub fn delete_entropy() -> Result<()>`（uninstall 用）

- [ ] **Step 1: 写失败测试** `tests/crypto_blob.rs`：

```rust
use gdut_net::crypto::{unwrap_blob, wrap_blob};

#[test]
fn blob_roundtrip() {
    let b = wrap_blob("aabbccdd", &[1, 2, 3, 0xff]);
    assert!(b.starts_with("GDUT1:"));
    let (e, p) = unwrap_blob(&b).unwrap();
    assert_eq!(e, "aabbccdd");
    assert_eq!(p, vec![1, 2, 3, 0xff]);
}

#[test]
fn blob_rejects_garbage() {
    assert!(unwrap_blob("nonsense").is_none());
    assert!(unwrap_blob("GDUT2:x:y").is_none());
}
```

- [ ] **Step 2: 确认失败** — Run: `cargo test --test crypto_blob`；Expected: 编译失败。

- [ ] **Step 3: 实现**（纯部分如上；DPAPI 部分用 `CRYPT_INTEGER_BLOB { cbData, pbData }`，输出 blob 用 `LocalFree` 释放（`Win32_System_Memory` feature）。注册表用 `RegGetValueW`/`RegCreateKeyExW`/`RegSetValueExW`/`RegDeleteTreeW`。）

- [ ] **Step 4: 验证** — Run: `cargo test --test crypto_blob`（2 passed）+ `cargo check --target x86_64-pc-windows-msvc`（通过）。

- [ ] **Step 5: Commit** — `git add -A && git -c commit.gpgsign=false commit -m "feat: DPAPI 机器级密码保护与 blob 格式"`

---

### Task 11: service.rs + eventlog.rs + install/uninstall

**Files:**
- Create: `src/service.rs`、`src/eventlog.rs`；Modify: `src/cli.rs`（实现 Install/Uninstall 分支）、`src/lib.rs`

**Interfaces:**
- Produces:
  - `#[cfg(windows)] pub const SERVICE_NAME: &str = "gdut-net";`
  - `#[cfg(windows)] pub fn install(cfg_path: &Path, password_stdin: bool) -> Result<()>`，流程：管理员校验（非管理员 bail）→ 读学号（配置存在则复用，否则交互输入）→ 密码（`--password-stdin` 或 rpassword 交互）→ `crypto::protect` → `Config::save`（合并既有配置）→ `ras::ensure_entry` + `ras::set_credentials` → 建目录 → `ServiceManager::create_service`（OWN_PROCESS / AutoStart / Normal，参数 `--config <path> run`）→ 失败恢复 3 段（SC_ACTION_RESTART，delay 5s/30s/60s，dwResetPeriod=86400，`ChangeServiceConfig2W`）→ Event Source 注册表（`HKLM\SYSTEM\CurrentControlSet\Services\EventLog\Application\gdut-net`，`EventMessageFile=%SystemRoot%\System32\netmsg.dll` REG_EXPAND_SZ，`TypesSupported=7` REG_DWORD）→ 打印完成信息
  - `#[cfg(windows)] pub fn uninstall(cfg_path: &Path, purge: bool) -> Result<()>`：stop（轮询 Stopped 最多 10s）→ delete → Event Source 键删 → entropy 键删 → `--purge` 时删 ProgramData 目录
  - `#[cfg(windows)] pub fn service_main() -> Result<()>`：windows-service dispatcher（`Service::run` + 事件处理器收到 Stop → `CancellationToken` cancel），主体调 `runtime::start_all`（Task 12 前 bail 占位）
- Consumes: `windows-service 0.8`（ServiceManager/Service）、windows crate `Win32_System_Services`（ChangeServiceConfig2W 裸调，因 windows-service 0.8 不暴露 failure actions）、`Win32_System_EventLog`（RegisterEventSourceW/ReportEventW/DeregisterEventSource）、`Win32_UI_Shell`（IsUserAnAdmin）。

- [ ] **Step 1: 实现 eventlog.rs**（register/unregister source、`EventLog { handle }` + `report(level, msg)` + Drop deregister）

- [ ] **Step 2: 实现 service.rs install/uninstall/service_main**（按 Interfaces 契约；`set_service_recovery` 直接裸 API：`OpenSCManagerW`→`OpenServiceW(SERVICE_CHANGE_CONFIG)`→`ChangeServiceConfig2W(SERVICE_CONFIG_FAILURE_ACTIONS)`）

- [ ] **Step 3: 验证**

Run: `cargo check --target x86_64-pc-windows-msvc` + `cargo test`
Expected: 通过；既有测试全绿。

- [ ] **Step 4: 真机手动验证步骤**（写进 README 验收节，本任务 PR 描述里列出）：

```powershell
# 管理员 PowerShell
.\gdut-net.exe install          # 输入学号密码
net start gdut-net              # 服务启动，日志出现"拨号成功"
net stop gdut-net               # 优雅停止
sc.exe qfailure gdut-net        # 显示 3 段恢复动作
.\gdut-net.exe uninstall --purge
sc.exe query gdut-net           # 期望 1060（服务不存在）
```

- [ ] **Step 5: Commit** — `git add -A && git -c commit.gpgsign=false commit -m "feat: Windows 服务安装/卸载/运行骨架与事件日志"`

---

### Task 12: heartbeat/session.rs + runtime.rs 装配

**Files:**
- Create: `src/heartbeat/session.rs`、`src/runtime.rs`；Modify: `src/heartbeat/mod.rs`（+`pub mod session;`）、`src/cli.rs`（Run 分支接 service_main）、`src/lib.rs`

**Interfaces:**
- Produces:
  - `#[cfg(windows)] pub fn run_blocking(server: Ipv4Addr, src_ip: Ipv4Addr, interval: Duration, status_tx: watch::Sender<HeartbeatStatus>) -> Result<(), String>`：`UdpSocket::bind((src_ip, 61440))`——失败即 send `HeartbeatStatus::Error("61440 被占用，兼容模式不可用")` + log::error + 返回 Err（**不重试 bind**）；成功后 connect((server,61440))、recv 超时 2s、循环按 spec：ka1_pkt1→parse→ka1_pkt2(first=true)→sleep 3s→ka2_pkt1（响应是文件报文则学 flag 重发一次）→ka2_pkt2→sleep(interval-3s)；连续 5 次 recv 超时重置本轮（cnt 不重置）；rand 每轮新随机
  - `#[cfg(windows)] pub fn start_all(cfg: Config, stop: CancellationToken) -> Result<()>`：装配 RealDialer（ras::dial，密码 `crypto::unprotect`）、RealProber（每次 probe 刷新 `adapter::physical_adapter()`；配置 `interface` 非空按名匹配，不匹配 bail）→ Watchdog → 循环 `run_once` + sleep + 每轮填充 snapshot（ip=`adapter::ppp_adapter_ip()`）进 watch；`heartbeat.enabled` 时 `spawn_blocking` 跑 run_blocking + toast 钩子（bind 失败）；stop.cancelled() → hangup + 退出
- Consumes: Task 4 spec、Task 6/7/8/9/10 全部、Task 11 service_main。

- [ ] **Step 1: 实现 session.rs**（按上述契约；注意 `interval - Duration::from_secs(3)` 需 saturating_sub）

- [ ] **Step 2: 实现 runtime.rs**（按上述契约；watchdog 循环里 `tokio::select!` 监听 stop）

- [ ] **Step 3: 验证** — Run: `cargo check --target x86_64-pc-windows-msvc && cargo test`；Expected: 通过/全绿。

- [ ] **Step 4: Commit** — `git add -A && git -c commit.gpgsign=false commit -m "feat: 心跳会话循环与服务运行时装配"`

---

### Task 13: ipc server/client + notify + status 子命令

**Files:**
- Create: `src/ipc/server.rs`、`src/ipc/client.rs`、`src/notify.rs`；Modify: `src/cli.rs`（Status 分支）、`src/runtime.rs`（start_all 里起 IPC server + 接 Redial 命令）、`src/lib.rs`

**Interfaces:**
- Produces:
  - `#[cfg(windows)] pub const PIPE_NAME: &str = r"\\.\pipe\gdut-net";`
  - `#[cfg(windows)] pub fn spawn_server(snapshot_rx: watch::Receiver<StateSnapshot>, cmd_tx: mpsc::Sender<Command>, stop: CancellationToken)`：tokio named_pipe 循环 accept 多客户端，连接即推送当前快照 + 变更即推，收 ClientMsg::Cmd 转发 cmd_tx
  - `#[cfg(windows)] pub fn connect() -> Result<PipeClient>`；`PipeClient::next_state(&mut self) -> Result<StateSnapshot>`、`send_cmd(&mut self, c: Command) -> Result<()>`
  - `#[cfg(windows)] pub fn toast(title: &str, body: &str) -> Result<()>`（tauri-winrt-notification）
  - `#[cfg(windows)] pub fn status_once() -> Result<()>`：连接管道 → 打印快照（人类可读）→ 断开
  - runtime 侧：命令 `Redial` → 触发立即 `do_dial`（Watchdog 加 `request_redial` 标志位或 Channel）；连续重拨失败 ≥10min、心跳 Error、服务停止前 → `notify::toast`（配置节流：同一原因 30min 内不重复）
- Consumes: Task 5 protocol、Task 12 runtime。

- [ ] **Step 1: 实现 server.rs / client.rs**（JSON-line 帧用 Task 5 的 encode_frame/FrameDecoder）

- [ ] **Step 2: runtime 接入 IPC + 通知钩子**（watchdog 侧加 `request_redial()`：`AtomicBool`，`run_once` 顶部检查到即直接 do_dial）

- [ ] **Step 3: cli.rs Status 分支接 status_once**

- [ ] **Step 4: 验证** — Run: `cargo check --target x86_64-pc-windows-msvc && cargo test`；Expected: 通过/全绿。

- [ ] **Step 5: Commit** — `git add -A && git -c commit.gpgsign=false commit -m "feat: 命名管道 IPC、toast 通知与 status 子命令"`

---

### Task 14: 托盘 + 状态面板

**Files:**
- Create: `src/tray/mod.rs`、`src/tray/panel.rs`；Modify: `src/cli.rs`（Tray 分支）、`src/lib.rs`

**Interfaces:**
- Produces:
  - `#[cfg(windows)] pub fn run_tray() -> Result<()>`：`tray_icon::TrayIconBuilder` 建图标+菜单（`状态`、`立即重拨`、`退出`），左键默认弹菜单（`with_menu_on_left_click(true)`）；win32 事件循环线程跑 `winit`/裸 `GetMessageW` 泵——主线程 std 循环 + `tray_icon::TrayIconEvent::receiver()`/`MenuEvent::receiver()` channel
  - 状态展示：后台线程 `PipeClient` 循环收 StateSnapshot，缓存 `Arc<Mutex<Option<StateSnapshot>>>`；菜单项文本动态更新（`状态: 已连接 / IP x.x.x.x`）
  - `状态` 菜单 → `panel::show(snapshot_cache, pipe)`：按需建 egui（eframe glow）窗口，显示状态/在线时长/IP/掉线原因/重拨次数/心跳状态 + `立即重拨` 按钮（发 Redial 命令）；窗口关闭即销毁 app 释放内存
  - `立即重拨` 菜单 → send_cmd(Redial)
  - 托盘侧检测管道断开（服务退出）→ `notify::toast("gdut-net 服务已停止")`
- Consumes: Task 13 client/notify。

- [ ] **Step 1: 实现 tray/mod.rs**（图标用内嵌 32x32 PNG（`include_bytes!`，占位可用纯色方块））

- [ ] **Step 2: 实现 tray/panel.rs**（eframe run 在独立线程，窗口关闭 `exit(0)` 该 app）

- [ ] **Step 3: cli.rs Tray 分支接 run_tray**

- [ ] **Step 4: 验证** — Run: `cargo check --target x86_64-pc-windows-msvc && cargo test`；Expected: 通过/全绿。

- [ ] **Step 5: Commit** — `git add -A && git -c commit.gpgsign=false commit -m "feat: 托盘常驻图标与按需状态面板"`

---

### Task 15: 日志接线 + CI

**Files:**
- Modify: `src/runtime.rs`（日志初始化调用）、`src/cli.rs`（各子命令入口初始化）
- Create: `.github/workflows/ci.yml`、`README.md`

**Interfaces:**
- Produces:
  - `pub fn init_logging(cfg: &LogCfg) -> Result<flexi_logger::LoggerHandle>`：`flexi_logger::Logger::try_with_str("info")` + `log_to_file`/`duplicate_to_stderr` + `RotateBySize(max_size_mb MB)` + `cleanup_in_current_dir(rotate_keep)`；event_log=true 时加自定义 `Log` 实现转发 error/warn 到 EventLog
  - CI：`on: [push, pull_request]`，jobs：`linux-test`（ubuntu-latest：`cargo test`、`cargo clippy -- -D warnings`、`cargo fmt --check`）、`windows-build`（windows-latest：`cargo test`、`cargo clippy`、`cargo build --release`，artifact 上传 zip）
- Consumes: flexi_logger、Task 11 EventLog。

- [ ] **Step 1: 实现并接线 init_logging**（服务/CLI 命令入口各调一次）

- [ ] **Step 2: 写 ci.yml**（如上两 job；windows job 里 `cargo build --release` 后 PowerShell Compress 产物 `gdut-net-x86_64.zip`，`actions/upload-artifact@v4`）

- [ ] **Step 3: 写 README.md**（安装/卸载步骤、配置说明含心跳风险提示（ADR-0002）、真机验收清单（Task 11 Step 4 + 需求文档验收标准 1–5））

- [ ] **Step 4: 本机验证** — Run: `cargo test && cargo clippy -- -D warnings && cargo fmt --check`；Expected: 全绿。

- [ ] **Step 5: Commit** — `git add -A && git -c commit.gpgsign=false commit -m "ci: 日志接线与 GitHub Actions"`

---

## 验收映射（需求 → 任务）

| 验收标准 | 任务 |
|---|---|
| 1. 全新 Win11 安装→自动拨号→断网自动重连 | 11（install）+ 12（runtime）+ 9（watchdog） |
| 2. TUN/Tailscale 共存，重启后重拨成功 | 6（物理适配器绑定）+ 12 |
| 3. 内存 <50MB / 24h 无泄漏 | release profile opt-level=z + 状态机无累积分配 |
| 4. 72h 不掉线（默认关心跳）；心跳抓包验证 | 8（两级探测减少误重拨）+ 4/12（心跳，抓包真机验证） |
| 5. 卸载干净 | 11（uninstall --purge） |
