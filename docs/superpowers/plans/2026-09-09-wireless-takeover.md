# Wireless Takeover & Tray Redo Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Plug in = wired PPPoE; unplug = automatic WiFi takeover (SSID `gdut` + eportal auth); two runtime-switchable modes (exclusive / standby); tray menu + status panel redo.

**Architecture:** New `wireless` module: pure decision core (`Brain`) + Win32 glue (WlanAPI connect/disconnect, /32 host-route guard, bound-source HTTP portal login), wired into `runtime.rs` as a sibling actor of the heartbeat task. Watchdog untouched. Tray: native menu with checkable mode items + per-state icon; panel = eframe glow + `default_fonts` (black screen was a font-feature bug, not iGPU — ADR-0006).

**Tech Stack:** Rust 1.98 stable, tokio, windows 0.62 (+`Win32_NetworkManagement_Wifi` feature), tray-icon 0.24/muda, eframe 0.36 glow. No new HTTP client crate (hand-rolled socket2 GET, same as probe.rs).

**Spec:** `docs/superpowers/specs/2026-09-09-wireless-takeover-design.md` + ADR-0005/0006. Read both before starting.

## Global Constraints

- All user-facing output (CLI prints, log lines, menu/panel text, events) MUST be English (AGENTS.md anti-garble rule). Doc comments may stay Chinese per house style.
- Portal URL contains the cleartext password — it must NEVER appear in logs/events (redact to host+path).
- Pure logic (portal parsing, brain, config, protocol) has zero `windows::` imports and runs on Linux CI; Win32 glue is `#[cfg(windows)]` and is verified via `cargo check --target x86_64-pc-windows-msvc`.
- Every Windows task ends with: `cargo check --target x86_64-pc-windows-msvc` AND `cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings` AND `cargo fmt`.
- Every Linux-testable task ends with: `cargo test` (all green) AND `cargo clippy -- -D warnings` AND `cargo fmt`.
- windows 0.62 API names below were verified against the vendored crate source (`~/.cargo/registry/src/index.crates.io-*/windows-0.62.2/src/Windows/Win32/...`). If `cargo check` disagrees with a struct/field name, the vendored source is the source of truth — fix the code, not the plan.
- Commit message style: `feat:/fix:/test:/docs:` prefixes like existing history.
- TDD: write failing test → run → implement → run → commit. Do not skip the failing-test run.

---

### Task 1: `wireless/portal.rs` — pure eportal URL builder + JSONP parser

**Files:**
- Create: `src/wireless/mod.rs` (skeleton with `pub mod portal;`, other submodules declared later)
- Create: `src/wireless/portal.rs` (pure half only)
- Modify: `src/lib.rs` (add `pub mod wireless;`)
- Test: `tests/wireless_portal.rs`

**Interfaces (produced, consumed by Tasks 8, 9, 10):**
```rust
pub fn urlencode(s: &str) -> String
pub fn build_login_url(base: &str, user: &str, pass: &str, ip: std::net::Ipv4Addr, ac_ip: &str) -> String
pub enum PortalResult { Success, Failure(String), Malformed }
pub fn parse_portal_reply(body: &str) -> PortalResult
pub fn redact_query(url: &str) -> String   // ".../login?callback=..." -> ".../login?***"
```

- [ ] **Step 1: Write failing tests** — create `tests/wireless_portal.rs`:

```rust
use std::net::Ipv4Addr;
use gdut_net::wireless::portal::*;

#[test]
fn urlencode_keeps_unreserved_and_encodes_specials() {
    assert_eq!(urlencode("abcXYZ019-._~"), "abcXYZ019-._~");
    assert_eq!(urlencode("p@ss+w&d"), "p%40ss%2Bw%26d");
    assert_eq!(urlencode("密码"), "%E5%AF%86%E7%A0%81");
}

#[test]
fn login_url_contains_eportal_fields() {
    let url = build_login_url(
        "http://10.0.3.2:801/eportal/portal/login",
        "1145141919", "p@ss", Ipv4Addr::new(10, 43, 199, 166), "172.16.254.2",
    );
    assert!(url.starts_with("http://10.0.3.2:801/eportal/portal/login?"));
    assert!(url.contains("callback=dr1004"));
    assert!(url.contains("login_method=1"));
    assert!(url.contains("user_account=1145141919"));
    assert!(url.contains("user_password=p%40ss"));
    assert!(url.contains("wlan_user_ip=10.43.199.166"));
    assert!(url.contains("wlan_user_mac=000000000000"));
    assert!(url.contains("wlan_ac_ip=172.16.254.2"));
    assert!(url.contains("jsVersion=4.1.3"));
}

#[test]
fn jsonp_success_and_failure() {
    assert_eq!(parse_portal_reply(r#"dr1004({"result":"1","msg":"Login is successful"})"#), PortalResult::Success);
    assert_eq!(
        parse_portal_reply(r#"dr1004({"result":"0","msg":"E2620: already online"})"#),
        PortalResult::Failure("E2620: already online".into())
    );
}

#[test]
fn jsonp_malformed_and_plain_json() {
    assert!(matches!(parse_portal_reply("<html>login page</html>"), PortalResult::Malformed));
    // 无 callback 包裹的裸 JSON 也要认（服务器行为兜底）
    assert_eq!(parse_portal_reply(r#"{"result":"1"}"#), PortalResult::Success);
}

#[test]
fn redact_query_strips_credentials() {
    let url = build_login_url("http://10.0.3.2:801/eportal/portal/login", "u", "secret", Ipv4Addr::LOCALHOST, "172.16.254.2");
    let red = redact_query(&url);
    assert!(red.starts_with("http://10.0.3.2:801/eportal/portal/login?"));
    assert!(red.contains("***"));
    assert!(!red.contains("secret"));
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test --test wireless_portal` → compile error (module missing).

- [ ] **Step 3: Implement** — `src/wireless/mod.rs`:

```rust
//! 无线接管（ADR-0005）：纯决策核心 + Win32 胶水分层（同 probe.rs 先例）。

pub mod portal;
#[cfg(windows)]
pub mod routes;
#[cfg(windows)]
pub mod test;
#[cfg(windows)]
pub mod wlan;
```
(只放 `pub mod portal;`，其余三行在对应任务再加——避免引用不存在的文件。)

`src/wireless/portal.rs` pure half:

```rust
//! eportal 认证（纯逻辑）：URL 构建（percent-encode）与 JSONP 回包解析。
//! 协议事实来自 lin-snow/GDUT-Login（2024 实证，大学城），常量见 ADR-0005。

use std::net::Ipv4Addr;

/// percent-encode：非 unreserved（RFC 3986）一律 %XX（大写 hex）。
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(*b as char),
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
    Failure(String),
    Malformed,
}

/// 解析 JSONP（`dr1004({...})`）或裸 JSON：`result` == "1"/1 → Success。
pub fn parse_portal_reply(body: &str) -> PortalResult {
    let inner = extract_json(body);
    let Some(v) = serde_json::from_str::<serde_json::Value>(inner).ok() else {
        return PortalResult::Malformed;
    };
    match v.get("result").map(|r| r.as_str().map(|s| s.to_string()).unwrap_or_else(|| r.to_string())) {
        Some(r) if r == "1" => PortalResult::Success,
        Some(_) => PortalResult::Failure(
            v.get("msg").and_then(|m| m.as_str()).unwrap_or("no msg").to_string(),
        ),
        None => PortalResult::Malformed,
    }
}

/// 取 callback(...) 括号内内容；无括号则原样（裸 JSON 兜底）。
fn extract_json(body: &str) -> &str {
    match (body.find('('), body.rfind(')')) {
        (Some(a), Some(b)) if a < b => &body[a + 1..b],
        _ => body.trim(),
    }
}
```

`src/lib.rs` add `pub mod wireless;` (alphabetical position: after `pub mod watchdog;`? No — alphabetically `wireless` < `watchdog`… house file lists modules alphabetically; put `pub mod wireless;` before `pub mod watchdog;`).

- [ ] **Step 4: Run, verify pass** — `cargo test --test wireless_portal` green; `cargo test` all green.
- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -- -D warnings && cargo fmt
git add src/wireless/ src/lib.rs tests/wireless_portal.rs
git commit -m "feat(wireless): eportal login URL builder + JSONP reply parser (pure)"
```

---

### Task 2: IPC protocol — NetMode, SetMode, WirelessSnapshot, EventLog

**Files:**
- Modify: `src/ipc/protocol.rs`
- Modify: `src/watchdog.rs` (snapshot() literal gains new fields)
- Modify: `tests/ipc_protocol.rs` (2 literals + 2 new tests)

**Interfaces (consumed by Tasks 3, 4, 9, 10, 11, 12):**
```rust
pub enum NetMode { WiredExclusive, WiredPlusStandby }        // serde snake_case, Default = WiredExclusive
pub enum WPhase { Off, Joining, Authing, Online, Error }     // serde snake_case
pub struct WirelessSnapshot { pub phase: WPhase, pub ip: Option<String>, pub last_error: Option<String> }  // + Default
pub struct EventLog { .. }  // cap 20, push(unix_secs, msg), ring() -> &VecDeque<String>
// StateSnapshot 增（全部 #[serde(default)]）:
//   pub mode: NetMode, pub wireless: WirelessSnapshot, pub events: VecDeque<String>
//   + mode_text() / wireless_text()
// Command 增: SetMode { mode: NetMode }
```

- [ ] **Step 1: Write failing tests** — in `tests/ipc_protocol.rs`, update the two `StateSnapshot { ... }` literals (add `mode: NetMode::WiredExclusive, wireless: WirelessSnapshot::default(), events: VecDeque::new()`; import `std::collections::VecDeque`) and append:

```rust
#[test]
fn set_mode_command_serializes() {
    let bytes = encode_frame(&ClientMsg::Cmd { c: Command::SetMode { mode: NetMode::WiredPlusStandby } });
    let msg: ClientMsg = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(msg, ClientMsg::Cmd { c: Command::SetMode { mode: NetMode::WiredPlusStandby } });
}

#[test]
fn old_snapshot_json_parses_into_new_struct() {
    // 旧版服务发出的快照（无 mode/wireless/events 字段）必须能被新托盘解析。
    let legacy = br#"{"status":"connected","since_unix":1756500000,"ip":"10.30.1.2","last_drop_reason":null,"redial_attempts":0,"heartbeat":"Off"}"#;
    let snap: StateSnapshot = serde_json::from_slice(legacy).unwrap();
    assert_eq!(snap.mode, NetMode::WiredExclusive);
    assert_eq!(snap.wireless.phase, WPhase::Off);
    assert!(snap.events.is_empty());
}

#[test]
fn event_log_caps_and_formats() {
    let mut log = EventLog::new();
    for i in 0..25 { log.push(1_757_000_000 + i, &format!("event {i}")); }
    assert_eq!(log.ring().len(), 20);
    assert!(log.ring().back().unwrap().ends_with("event 24"));
    assert!(log.ring().front().unwrap().contains("event 5"));
    assert!(log.ring().front().unwrap().starts_with('[')); // "[HH:MM:SS] ..."
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test --test ipc_protocol` (compile errors).

- [ ] **Step 3: Implement** in `src/ipc/protocol.rs`:

```rust
/// 网络模式（ADR-0005）：exclusive（失联去抖接管/恢复让位）或 standby（WLAN 常连）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NetMode {
    #[default]
    WiredExclusive,
    WiredPlusStandby,
}

/// 无线相位（快照用，无 payload；错误文本走 WirelessSnapshot.last_error）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WPhase {
    #[default]
    Off,
    Joining,
    Authing,
    Online,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WirelessSnapshot {
    #[serde(default)]
    pub phase: WPhase,
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}
```

`StateSnapshot` 新增字段（放在 `heartbeat` 之后）:
```rust
    #[serde(default)]
    pub mode: NetMode,
    #[serde(default)]
    pub wireless: WirelessSnapshot,
    #[serde(default)]
    pub events: VecDeque<String>,
```
impl 里的 text helpers:
```rust
    /// 模式英文描述（status/托盘共用）。
    pub fn mode_text(&self) -> String {
        match self.mode {
            NetMode::WiredExclusive => "Wired only (auto wireless takeover)".to_string(),
            NetMode::WiredPlusStandby => "Wired + wireless standby".to_string(),
        }
    }

    /// 无线链路英文描述。
    pub fn wireless_text(&self) -> String {
        let phase = match self.wireless.phase {
            WPhase::Off => "Off",
            WPhase::Joining => "Joining",
            WPhase::Authing => "Authenticating",
            WPhase::Online => "Online",
            WPhase::Error => "Error",
        };
        match (&self.wireless.ip, &self.wireless.last_error) {
            (Some(ip), _) => format!("{phase} {ip}"),
            (None, Some(e)) => format!("{phase} ({e})"),
            _ => phase.to_string(),
        }
    }
```

`Command` 增加变体:
```rust
pub enum Command {
    Redial,
    SetMode {
        mode: NetMode,
    },
}
```

`EventLog`（放 protocol.rs 尾部）:
```rust
/// 事件环（快照 events 的服务端源）：容量 20，UTC HH:MM:SS 前缀。
#[derive(Debug, Clone)]
pub struct EventLog {
    cap: usize,
    ring: VecDeque<String>,
}

impl Default for EventLog {
    fn default() -> Self {
        Self { cap: 20, ring: VecDeque::new() }
    }
}

impl EventLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, unix_secs: u64, msg: &str) {
        let d = unix_secs % 86_400;
        let line = format!("[{:02}:{:02}:{:02}] {msg}", d / 3600, (d % 3600) / 60, d % 60);
        if self.ring.len() >= self.cap {
            self.ring.pop_front();
        }
        self.ring.push_back(line);
    }

    pub fn ring(&self) -> &VecDeque<String> {
        &self.ring
    }
}
```

`src/watchdog.rs` `snapshot()`（约 :107）的字面量补三个字段:
```rust
            mode: NetMode::default(),
            wireless: WirelessSnapshot::default(),
            events: VecDeque::new(),
```
（`use` 处相应加 `NetMode, WirelessSnapshot`；`VecDeque` 已在 use。）

- [ ] **Step 4: Run** — `cargo test` all green（包括既有 watchdog/ipc 用例）。
- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -- -D warnings && cargo fmt
git add src/ipc/protocol.rs src/watchdog.rs tests/ipc_protocol.rs
git commit -m "feat(ipc): NetMode/SetMode command, wireless snapshot fields, event ring (serde-default compat)"
```

---

### Task 3: config — `[wireless]` section + validation

**Files:**
- Modify: `src/config.rs`
- Modify: `tests/config.rs`

**Interfaces (consumed by Tasks 9, 10):**
```rust
pub struct WirelessCfg {
    pub enabled: bool,               // default true
    pub mode: NetMode,               // default WiredExclusive
    pub ssid: String,                // "gdut"
    pub profile: String,             // "gdut"（Windows WLAN profile 名）
    pub portal_url: String,          // "http://10.0.3.2:801/eportal/portal/login"
    pub wlan_ac_ip: String,          // "172.16.254.2"（HEMC）
    pub probe_host: String,          // "223.5.5.5"
    pub takeover_after_secs: u64,    // 8
    pub release_after_secs: u64,     // 10
    pub standby_metric: u32,         // 10；0 = 不压制
}
// Config 增: #[serde(default)] pub wireless: WirelessCfg
```

- [ ] **Step 1: Write failing tests** — append to `tests/config.rs`:

```rust
#[test]
fn wireless_defaults_and_validation() {
    let mut cfg = Config::default();
    assert!(cfg.wireless.enabled);
    assert_eq!(cfg.wireless.profile, "gdut");
    assert!(cfg.validate().is_ok());

    cfg.wireless.probe_host = "not-an-ip".into();
    assert!(cfg.validate().is_err());

    cfg.wireless.probe_host = "223.5.5.5".into();
    cfg.wireless.portal_url = "http://portal.example.com/login".into(); // 非法：域名非 IPv4 字面量
    assert!(cfg.validate().is_err());

    cfg.wireless.portal_url = "http://10.0.3.2:801/eportal/portal/login".into();
    cfg.wireless.takeover_after_secs = 0;
    assert!(cfg.validate().is_err());

    cfg.wireless.takeover_after_secs = 8;
    assert!(cfg.validate().is_ok());
}

#[test]
fn sample_round_trips_with_wireless() {
    let cfg: Config = toml::from_str(&Config::sample()).unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.wireless.wlan_ac_ip, "172.16.254.2");
}
```

- [ ] **Step 2: Run, verify fail** (`cargo test --test config`).

- [ ] **Step 3: Implement** — `src/config.rs`:

`use crate::ipc::protocol::NetMode;` 加到文件头。`Config` struct 增 `#[serde(default)] pub wireless: WirelessCfg,`。新增：

```rust
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WirelessCfg {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub mode: NetMode,
    #[serde(default = "default_wlan_profile")]
    pub profile: String,
    #[serde(default = "default_portal_url")]
    pub portal_url: String,
    #[serde(default = "default_wlan_ac_ip")]
    pub wlan_ac_ip: String,
    #[serde(default = "default_probe_host")]
    pub probe_host: String,
    #[serde(default = "default_takeover_after")]
    pub takeover_after_secs: u64,
    #[serde(default = "default_release_after")]
    pub release_after_secs: u64,
    #[serde(default = "default_standby_metric")]
    pub standby_metric: u32,
}

fn default_true() -> bool { true }
fn default_wlan_profile() -> String { "gdut".into() }
fn default_portal_url() -> String { "http://10.0.3.2:801/eportal/portal/login".into() }
fn default_wlan_ac_ip() -> String { "172.16.254.2".into() }
fn default_probe_host() -> String { "223.5.5.5".into() }
fn default_takeover_after() -> u64 { 8 }
fn default_release_after() -> u64 { 10 }
fn default_standby_metric() -> u32 { 10 }

impl Default for WirelessCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: NetMode::default(),
            profile: default_wlan_profile(),
            portal_url: default_portal_url(),
            wlan_ac_ip: default_wlan_ac_ip(),
            probe_host: default_probe_host(),
            takeover_after_secs: default_takeover_after(),
            release_after_secs: default_release_after(),
            standby_metric: default_standby_metric(),
        }
    }
}
```

（`mode`/`enabled` 字段注释掉 ssid——设计里 ssid 与 profile 同值且 WlanConnect 只用 profile，砍掉冗余字段 YAGNI。）

`validate()` 末尾追加：
```rust
        let w = &self.wireless;
        if w.enabled {
            if crate::probe::parse_http_probe_target(&w.portal_url).is_none() {
                anyhow::bail!(
                    "wireless.portal_url must be http:// + IPv4 literal (with optional port), got {:?}",
                    w.portal_url
                );
            }
            if w.probe_host.parse::<std::net::Ipv4Addr>().is_err() {
                anyhow::bail!("wireless.probe_host must be an IPv4 literal, got {:?}", w.probe_host);
            }
            if w.takeover_after_secs < 1 || w.release_after_secs < 1 {
                anyhow::bail!("wireless takeover_after_secs/release_after_secs must be >= 1");
            }
            if w.standby_metric > 9999 {
                anyhow::bail!("wireless.standby_metric must be <= 9999 (0 = disable)");
            }
        }
```

- [ ] **Step 4: Run** — `cargo test` all green；`Config::sample()` 输出自动带 `[wireless]` 段（Default 驱动）。
- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -- -D warnings && cargo fmt
git add src/config.rs tests/config.rs
git commit -m "feat(config): [wireless] section with HEMC defaults + validation"
```

---

### Task 4: `wireless/mod.rs` — Brain decision core (pure)

**Files:**
- Modify: `src/wireless/mod.rs`
- Create: `tests/wireless_brain.rs`

**Interfaces (consumed by Task 9):**
```rust
pub const AUTH_RETRY_DELAYS: [u64; 3] = [5, 15, 30];
pub const JOIN_TIMEOUT_SECS: u64 = 60;
pub struct World { pub now: u64, pub eth_link_up: bool, pub wired_connected: bool,
                   pub wlan_associated: bool, pub wlan_ip: bool, pub probe: Option<ProbeVerdict> }
pub enum Action { None, Associate, Disassociate, PortalAuth, ProbeNow }
pub struct Brain { /* mode, phase, 计时器 */ }
impl Brain {
    pub fn new(mode: NetMode, takeover_after: u64, release_after: u64, probe_interval: u64) -> Self
    pub fn set_mode(&mut self, m: NetMode)
    pub fn restart(&mut self)                                   // Joining 超时由 manager 调
    pub fn on_auth(&mut self, ok: bool, msg: &str, now: u64)
    pub fn decide(&mut self, w: &World) -> Action
    pub fn snapshot(&self) -> WirelessSnapshot                   // ip 由 manager 填
}
```

语义表（实现即测试依据，全部已定）：
- standby：phase Off/Error →（Error 需过 next_auth_at 门）Associate；Joining 且 associated+ip → PortalAuth（→Authing）；Online 探测周期到 → ProbeNow；probe Kicked → PortalAuth；掉关联 → Associate。
- exclusive：wired Connected 持续 `release_after` 且 phase!=Off → Disassociate（→Off）；wired 失联持续 `takeover_after`（phase Off）或过退避门（phase Error）→ Associate；其余同 standby 维持逻辑。
- on_auth(true) → Online（清 error/退避，立即探一次）；on_auth(false) → Error + next_auth_at = now + [5,15,30][min(fails,3)-1]。

- [ ] **Step 1: Write failing tests** — `tests/wireless_brain.rs`:

```rust
use gdut_net::ipc::protocol::{NetMode, WPhase};
use gdut_net::probe::ProbeVerdict;
use gdut_net::wireless::*;

fn brain() -> Brain {
    Brain::new(NetMode::WiredExclusive, 8, 10, 30)
}
fn world(now: u64, wired: bool, assoc: bool, ip: bool, probe: Option<ProbeVerdict>) -> World {
    World { now, eth_link_up: wired, wired_connected: wired, wlan_associated: assoc, wlan_ip: ip, probe }
}

#[test]
fn exclusive_takeover_debounces_then_joins_and_auths() {
    let mut b = brain();
    assert_eq!(b.decide(&world(0, false, false, false, None)), Action::None);
    assert_eq!(b.decide(&world(7, false, false, false, None)), Action::None);
    assert_eq!(b.decide(&world(8, false, false, false, None)), Action::Associate);
    // 关联+IP 到位 → 立即认证
    assert_eq!(b.decide(&world(9, false, true, true, None)), Action::PortalAuth);
    b.on_auth(true, "ok", 9);
    assert_eq!(b.decide(&world(9, false, true, true, None)), Action::ProbeNow);
    assert_eq!(b.decide(&world(20, false, true, true, Some(ProbeVerdict::Alive))), Action::None);
    assert_eq!(b.decide(&world(39, false, true, true, Some(ProbeVerdict::Alive))), Action::None);
    assert_eq!(b.decide(&world(39 + 1, false, true, true, Some(ProbeVerdict::Alive))), Action::ProbeNow);
}

#[test]
fn exclusive_releases_after_wired_stable() {
    let mut b = brain();
    b.decide(&world(8, false, true, true, None)); // Associate → Online 流程走完
    b.decide(&world(9, false, true, true, None));
    b.on_auth(true, "ok", 9);
    assert_eq!(b.decide(&world(30, true, true, true, Some(ProbeVerdict::Alive))), Action::None);
    assert_eq!(b.decide(&world(39, true, true, true, Some(ProbeVerdict::Alive))), Action::None);
    assert_eq!(b.decide(&world(40, true, true, true, Some(ProbeVerdict::Alive))), Action::Disassociate);
    assert_eq!(b.decide(&world(41, true, false, false, None)), Action::None); // Off，有线健康不再动作
}

#[test]
fn standby_ignores_wired_state() {
    let mut b = Brain::new(NetMode::WiredPlusStandby, 8, 10, 30);
    assert_eq!(b.decide(&world(0, true, false, false, None)), Action::Associate);
    assert_eq!(b.decide(&world(1, true, true, true, None)), Action::PortalAuth);
}

#[test]
fn kicked_reauths_immediately() {
    let mut b = brain();
    b.decide(&world(8, false, true, true, None));
    b.decide(&world(9, false, true, true, None));
    b.on_auth(true, "ok", 9);
    b.decide(&world(9, false, true, true, None)); // ProbeNow
    assert_eq!(b.decide(&world(10, false, true, true, Some(ProbeVerdict::Kicked))), Action::PortalAuth);
}

#[test]
fn auth_failure_backs_off_5_15_30() {
    let mut b = brain();
    b.decide(&world(8, false, true, true, None));
    b.decide(&world(9, false, true, true, None)); // PortalAuth, phase Authing
    b.on_auth(false, "result=0", 9);
    assert_eq!(b.phase(), WPhase::Error);
    assert_eq!(b.decide(&world(13, false, true, true, None)), Action::None);
    assert_eq!(b.decide(&world(14, false, true, true, None)), Action::Associate); // 重走 join→auth
    b.on_auth(false, "again", 15);
    b.on_auth(false, "third", 16); // 第 3 次失败 → 30s
    assert_eq!(b.decide(&world(45, false, true, true, None)), Action::None);
    assert_eq!(b.decide(&world(46, false, true, true, None)), Action::Associate);
}

#[test]
fn assoc_lost_rejoins_and_mode_switch_releases() {
    let mut b = brain();
    b.decide(&world(8, false, true, true, None));
    b.decide(&world(9, false, true, true, None));
    b.on_auth(true, "ok", 9);
    assert_eq!(b.decide(&world(30, false, false, false, None)), Action::Associate);
    // standby→exclusive 且有线健康：10s 后让位
    b.set_mode(NetMode::WiredExclusive);
    b.on_auth(true, "ok", 31);
    assert_eq!(b.decide(&world(100, true, true, true, Some(ProbeVerdict::Alive))), Action::None);
    assert_eq!(b.decide(&world(110, true, true, true, Some(ProbeVerdict::Alive))), Action::Disassociate);
}
```

- [ ] **Step 2: Run, verify fail** (`cargo test --test wireless_brain`).

- [ ] **Step 3: Implement** — append to `src/wireless/mod.rs`（保留 Task 1 的 `pub mod portal;`）:

```rust
use crate::ipc::protocol::{NetMode, WPhase, WirelessSnapshot};
use crate::probe::ProbeVerdict;

/// portal 认证失败重试间隔（秒）：5s → 15s → 30s 封顶。
pub const AUTH_RETRY_DELAYS: [u64; 3] = [5, 15, 30];
/// Joining 相位等 IP/关联超时（manager 判定后调 restart）。
pub const JOIN_TIMEOUT_SECS: u64 = 60;

/// manager 每拍采集的世界状态（纯数据）。
#[derive(Debug, Clone, Default)]
pub struct World {
    pub now: u64,
    pub eth_link_up: bool,
    pub wired_connected: bool,
    pub wlan_associated: bool,
    pub wlan_ip: bool,
    pub probe: Option<ProbeVerdict>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    /// WlanConnect(profile)。
    Associate,
    /// WlanDisconnect + 路由/metric 回滚（manager 统一执行）。
    Disassociate,
    /// 发一次 eportal 登录，结果经 [`Brain::on_auth`] 回报。
    PortalAuth,
    /// 到探测周期，manager 执行 probe_once 并缓存进下一拍 World.probe。
    ProbeNow,
}

/// 纯决策核心（ADR-0005 状态机）。不碰系统，只出 Action。
pub struct Brain {
    mode: NetMode,
    phase: WPhase,
    unhealthy_since: Option<u64>,
    healthy_since: Option<u64>,
    last_probe_at: Option<u64>,
    auth_busy: bool,
    auth_fail_count: u32,
    next_auth_at: Option<u64>,
    last_error: Option<String>,
    takeover_after: u64,
    release_after: u64,
    probe_interval: u64,
}

impl Brain {
    pub fn new(mode: NetMode, takeover_after: u64, release_after: u64, probe_interval: u64) -> Self {
        Self {
            mode,
            phase: WPhase::Off,
            unhealthy_since: None,
            healthy_since: None,
            last_probe_at: None,
            auth_busy: false,
            auth_fail_count: 0,
            next_auth_at: None,
            last_error: None,
            takeover_after,
            release_after,
            probe_interval,
        }
    }

    pub fn phase(&self) -> WPhase {
        self.phase
    }

    pub fn set_mode(&mut self, m: NetMode) {
        self.mode = m;
    }

    /// Joining 超时/外部复位：回 Off（带外计数清零）。
    pub fn restart(&mut self) {
        self.reset_off();
    }

    pub fn on_auth(&mut self, ok: bool, msg: &str, now: u64) {
        self.auth_busy = false;
        if ok {
            self.phase = WPhase::Online;
            self.auth_fail_count = 0;
            self.next_auth_at = None;
            self.last_error = None;
            self.last_probe_at = None;
        } else {
            self.auth_fail_count = self.auth_fail_count.saturating_add(1);
            let idx = (self.auth_fail_count as usize - 1).min(AUTH_RETRY_DELAYS.len() - 1);
            self.next_auth_at = Some(now + AUTH_RETRY_DELAYS[idx]);
            self.phase = WPhase::Error;
            self.last_error = Some(msg.to_string());
        }
    }

    pub fn snapshot(&self) -> WirelessSnapshot {
        WirelessSnapshot { phase: self.phase, ip: None, last_error: self.last_error.clone() }
    }

    pub fn decide(&mut self, w: &World) -> Action {
        match self.mode {
            NetMode::WiredPlusStandby => self.keep_wireless(w),
            NetMode::WiredExclusive => {
                if w.wired_connected {
                    self.unhealthy_since = None;
                    let since = *self.healthy_since.get_or_insert(w.now);
                    if w.now - since >= self.release_after && self.phase != WPhase::Off {
                        self.reset_off();
                        return Action::Disassociate;
                    }
                    Action::None
                } else {
                    self.healthy_since = None;
                    let since = *self.unhealthy_since.get_or_insert(w.now);
                    let debounced = w.now - since >= self.takeover_after;
                    match self.phase {
                        WPhase::Off if debounced => self.start_join(),
                        WPhase::Off => Action::None,
                        WPhase::Error if self.next_auth_at.map_or(true, |t| w.now >= t) => {
                            self.start_join()
                        }
                        WPhase::Error => Action::None,
                        _ => self.keep_wireless(w),
                    }
                }
            }
        }
    }

    /// 无线侧维持逻辑（standby 全量 / exclusive 失联期）。
    fn keep_wireless(&mut self, w: &World) -> Action {
        match self.phase {
            WPhase::Off => self.start_join(),
            WPhase::Error if self.next_auth_at.map_or(true, |t| w.now >= t) => self.start_join(),
            WPhase::Error => Action::None,
            WPhase::Joining => {
                if w.wlan_associated && w.wlan_ip {
                    self.phase = WPhase::Authing;
                    self.auth_busy = true;
                    self.next_auth_at = None;
                    Action::PortalAuth
                } else {
                    Action::None
                }
            }
            WPhase::Authing => {
                if !self.auth_busy && self.next_auth_at.map_or(true, |t| w.now >= t) {
                    self.auth_busy = true;
                    Action::PortalAuth
                } else {
                    Action::None
                }
            }
            WPhase::Online => {
                if w.probe == Some(ProbeVerdict::Kicked) {
                    self.phase = WPhase::Authing;
                    self.auth_busy = true;
                    self.next_auth_at = None;
                    return Action::PortalAuth;
                }
                if !w.wlan_associated {
                    return self.start_join();
                }
                match self.last_probe_at {
                    None => {
                        self.last_probe_at = Some(w.now);
                        Action::ProbeNow
                    }
                    Some(t) if w.now - t >= self.probe_interval => {
                        self.last_probe_at = Some(w.now);
                        Action::ProbeNow
                    }
                    _ => Action::None,
                }
            }
        }
    }

    fn start_join(&mut self) -> Action {
        self.phase = WPhase::Joining;
        self.auth_busy = false;
        self.next_auth_at = None;
        self.last_error = None;
        Action::Associate
    }

    fn reset_off(&mut self) {
        self.phase = WPhase::Off;
        self.auth_busy = false;
        self.next_auth_at = None;
        self.last_error = None;
        self.last_probe_at = None;
    }
}
```

- [ ] **Step 4: Run** — `cargo test --test wireless_brain` 全绿（若个别断言与实现差 1 tick，修测试中 world 时间使语义吻合，不许放宽实现语义）。
- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -- -D warnings && cargo fmt
git add src/wireless/mod.rs tests/wireless_brain.rs
git commit -m "feat(wireless): takeover/release brain decision core (pure, event-testable)"
```

---

### Task 5: adapter.rs — WLAN 适配器 + 以太网链路态 + ifindex

**Files:**
- Modify: `src/adapter.rs`
- Modify: `src/runtime.rs`（仅构造处补 ifindex 字段——见下）

**Interfaces (consumed by Tasks 6, 7, 9, 10):**
```rust
pub struct AdapterInfo { pub name: String, pub ipv4: Ipv4Addr, pub gateway: Option<Ipv4Addr>, pub ifindex: u32 }
#[cfg(windows)] pub fn wlan_adapter() -> Option<AdapterInfo>   // IF_TYPE_IEEE80211 + Up + 有 IPv4 + 非虚拟（按 gateway 优先排序取一）
#[cfg(windows)] pub fn ethernet_link_up() -> bool              // 任一非虚拟以太网卡 OperStatus Up（不要求 IP）
```

- [ ] **Step 1: Implement（win 胶水无 Linux 测试，编译验证即门）** — `src/adapter.rs`:

`AdapterInfo` 加字段 `pub ifindex: u32`。`win` 模块 `RawAdapter` 加 `pub ifindex: u32`；`adapters()` push 处加 `ifindex: unsafe { a.Anonymous1.Anonymous.IfIndex }`（union 读法：若 `a.IfIndex` 直接可读则用之——以 cargo check 为准；GAA 的 `Anonymous1` 是 union，若Index 读法见 vendored `IP_ADAPTER_ADDRESSES_LH_0_0`）。注意 `Default` 不需改（无 derive）。

三处构造点（`physical_adapter`、`ppp_adapter`、新增 `wlan_adapter`）的 `AdapterInfo { .. }` 全部补 `ifindex: a.ifindex`。

新增两个 win 函数（挂在 `ppp_adapter` 后）:
```rust
    /// WLAN 适配器（IF_TYPE_IEEE80211 + OperStatus Up + 有 IPv4，非虚拟），有网关者优先。
    pub(super) fn wlan_adapter() -> Option<AdapterInfo> {
        let selector = |a: &IP_ADAPTER_ADDRESSES_LH| {
            a.IfType == IF_TYPE_IEEE80211 && a.OperStatus == IfOperStatusUp
        };
        let mut candidates: Vec<AdapterInfo> = adapters(&selector)
            .ok()?
            .into_iter()
            .filter(|a| !super::is_virtual(&a.name) && !super::is_virtual(&a.desc))
            .filter_map(|a| {
                a.ipv4.map(|ipv4| AdapterInfo {
                    name: a.name,
                    ipv4,
                    gateway: a.gateway,
                    ifindex: a.ifindex,
                })
            })
            .collect();
        candidates.sort_by_key(|a| a.gateway.is_none());
        candidates.into_iter().next()
    }

    /// 以太网链路态：任一非虚拟以太网卡 Up（拔线 → false，秒级信号）。
    /// 与 physical_adapter() 区别：不要求已有 IPv4（DHCP 前的 link up 也算）。
    pub(super) fn ethernet_link_up() -> bool {
        let selector = |a: &IP_ADAPTER_ADDRESSES_LH| {
            a.IfType == IF_TYPE_ETHERNET_CSMACD && a.OperStatus == IfOperStatusUp
        };
        adapters(&selector)
            .map(|list| {
                list.iter()
                    .any(|a| !super::is_virtual(&a.name) && !super::is_virtual(&a.desc))
            })
            .unwrap_or(false)
    }
```
`IF_TYPE_IEEE80211` 加进 IpHelper use（常量已核实存在）。文件尾部 re-export:
```rust
#[cfg(windows)]
pub fn wlan_adapter() -> Option<AdapterInfo> {
    win::wlan_adapter()
}

#[cfg(windows)]
pub fn ethernet_link_up() -> bool {
    win::ethernet_link_up()
}
```
`src/runtime.rs` 中如有 `AdapterInfo` 字面量构造则补字段（grep 确认：实际只消费字段不构造，预计零改动）。

- [ ] **Step 2: Verify**
```bash
cargo check --target x86_64-pc-windows-msvc
cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings
cargo test && cargo fmt
```
- [ ] **Step 3: Commit**
```bash
git add src/adapter.rs
git commit -m "feat(adapter): wlan adapter lookup, ethernet link state, ifindex in AdapterInfo"
```

---

### Task 6: `wireless/routes.rs` — RouteGuard（/32 路由 + metric，自带回滚）

**Files:**
- Create: `src/wireless/routes.rs`
- Modify: `src/wireless/mod.rs`（放开 `#[cfg(windows)] pub mod routes;`）

**Interfaces (consumed by Tasks 9, 10):**
```rust
#[cfg(windows)]
pub struct RouteGuard { /* added routes + saved metric */ }
impl RouteGuard {
    pub fn new() -> Self
    /// 幂等：确保 dests 的 /32 都经 gw@ifindex 存在，多余的删掉；失败仅记日志（warn）。
    pub fn ensure(&mut self, dests: &[Ipv4Addr], gw: Ipv4Addr, ifindex: u32)
    /// 幂等压制 WLAN 接口 metric 到 target（首次保存原值）；target==0 不动作。
    pub fn set_standby_metric(&mut self, ifindex: u32, target: u32)
    /// 全量回滚：删已加路由 + 还原 metric。teardown 后 Guard 可复用。
    pub fn teardown(&mut self)
}
/// 启动清残留：删除指向 dests 的 /32（Protocol==NETMGMT）。
pub fn cleanup_stale(dests: &[Ipv4Addr])
```

- [ ] **Step 1: Implement** — `src/wireless/routes.rs`（win 胶水）:

```rust
//! /32 主机路由 + WLAN 接口 metric 管理（ADR-0005）。
//! 自包含铁律：ensure/teardown 成对，三条出口（让位/切模式/服务停止）全覆盖。

use std::net::Ipv4Addr;

use anyhow::{anyhow, Result};
use windows::Win32::Foundation::{ERROR_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::Networking::WinSock::{
    AF_INET, IN_ADDR, IN_ADDR_0, MIB_IPPROTO_NETMGMT, SOCKADDR_IN, SOCKADDR_INET,
};
use windows::Win32::NetworkManagement::IpHelper::{
    CreateIpForwardEntry2, DeleteIpForwardEntry2, GetIpForwardTable2, GetIpInterfaceEntry,
    InitializeIpForwardEntry, SetIpInterfaceEntry, IP_ADDRESS_PREFIX, MIB_IPFORWARD_ROW2,
    MIB_IPFORWARD_TABLE2,
};

type Entry = (Ipv4Addr, Ipv4Addr, u32); // (dest, gw, ifindex)

fn sockaddr_in4(ip: Ipv4Addr) -> SOCKADDR_INET {
    SOCKADDR_INET {
        Ipv4: SOCKADDR_IN {
            sin_family: AF_INET,
            sin_addr: IN_ADDR { S_un: IN_ADDR_0 { S_addr: u32::from(ip) } },
            ..Default::default()
        },
    }
}

fn forward_row(dest: Ipv4Addr, gw: Ipv4Addr, ifindex: u32) -> MIB_IPFORWARD_ROW2 {
    let mut row = MIB_IPFORWARD_ROW2::default();
    unsafe { InitializeIpForwardEntry(&mut row) };
    row.InterfaceIndex = ifindex;
    row.DestinationPrefix = IP_ADDRESS_PREFIX { Prefix: sockaddr_in4(dest), PrefixLength: 32 };
    row.NextHop = sockaddr_in4(gw);
    row.Metric = 1;
    row.Protocol = MIB_IPPROTO_NETMGMT;
    row
}

fn add(dest: Ipv4Addr, gw: Ipv4Addr, ifindex: u32) -> Result<()> {
    let row = forward_row(dest, gw, ifindex);
    let err = unsafe { CreateIpForwardEntry2(&row) };
    if err != ERROR_SUCCESS {
        return Err(anyhow!("CreateIpForwardEntry2({dest}) failed: {}", err.0));
    }
    Ok(())
}

fn del(dest: Ipv4Addr, gw: Ipv4Addr, ifindex: u32) -> Result<()> {
    let row = forward_row(dest, gw, ifindex);
    let err = unsafe { DeleteIpForwardEntry2(&row) };
    if err != ERROR_SUCCESS && err != ERROR_NOT_FOUND {
        return Err(anyhow!("DeleteIpForwardEntry2({dest}) failed: {}", err.0));
    }
    Ok(())
}

fn set_metric(ifindex: u32, metric: u32) -> Result<()> {
    let mut row = windows::Win32::NetworkManagement::IpHelper::MIB_IPINTERFACE_ROW::default();
    unsafe { windows::Win32::NetworkManagement::IpHelper::InitializeIpInterfaceEntry(&mut row) };
    row.Family = AF_INET;
    row.InterfaceIndex = ifindex;
    let err = unsafe { GetIpInterfaceEntry(&mut row) };
    if err != ERROR_SUCCESS {
        return Err(anyhow!("GetIpInterfaceEntry({ifindex}) failed: {}", err.0));
    }
    // 已知坑：IPv4 Set 前必须 SitePrefixLength=0（MSDN 明示）。
    row.SitePrefixLength = 0;
    row.Metric = metric;
    row.UseAutomaticMetric = false;
    let err = unsafe { SetIpInterfaceEntry(&mut row) };
    if err != ERROR_SUCCESS {
        return Err(anyhow!("SetIpInterfaceEntry({ifindex}) failed: {}", err.0));
    }
    Ok(())
}

/// 保存已加条目与已改接口，成对回滚。
#[derive(Default)]
pub struct RouteGuard {
    added: Vec<Entry>,
    saved_metric: Option<(u32, u32, bool)>, // (ifindex, 原值, 原 UseAutomaticMetric)
    applied_metric_ifindex: Option<u32>,
}

impl RouteGuard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ensure(&mut self, dests: &[Ipv4Addr], gw: Ipv4Addr, ifindex: u32) {
        for d in dests {
            if !self.added.iter().any(|(x, _, _)| x == d) {
                match add(*d, gw, ifindex) {
                    Ok(()) => self.added.push((*d, gw, ifindex)),
                    Err(e) => log::warn!("RouteGuard add {d} failed (kept going): {e:#}"),
                }
            }
        }
        self.added.retain(|(d, g, i)| {
            if dests.contains(d) {
                true
            } else {
                if let Err(e) = del(*d, *g, *i) {
                    log::warn!("RouteGuard del stale {d} failed: {e:#}");
                }
                false
            }
        });
    }

    pub fn set_standby_metric(&mut self, ifindex: u32, target: u32) {
        if target == 0 {
            return;
        }
        if self.applied_metric_ifindex == Some(ifindex) {
            return; // 幂等
        }
        // 先读原值（保存），再压
        let mut row = windows::Win32::NetworkManagement::IpHelper::MIB_IPINTERFACE_ROW::default();
        unsafe { windows::Win32::NetworkManagement::IpHelper::InitializeIpInterfaceEntry(&mut row) };
        row.Family = AF_INET;
        row.InterfaceIndex = ifindex;
        if unsafe { GetIpInterfaceEntry(&mut row) } != ERROR_SUCCESS {
            log::warn!("RouteGuard: read metric of if{ifindex} failed, skip suppression");
            return;
        }
        if let Err(e) = set_metric(ifindex, target) {
            log::warn!("RouteGuard suppress metric if{ifindex} -> {target} failed: {e:#}");
            return;
        }
        self.saved_metric = Some((ifindex, row.Metric, row.UseAutomaticMetric));
        self.applied_metric_ifindex = Some(ifindex);
    }

    fn restore_metric(&mut self) {
        if let Some((ifindex, metric, auto)) = self.saved_metric.take() {
            if let Err(e) = set_metric(ifindex, metric) {
                log::warn!("RouteGuard restore metric if{ifindex} -> {metric} failed: {e:#}");
                return;
            }
            // UseAutomaticMetric 还原（auto 时交还系统）
            if auto {
                let mut row = windows::Win32::NetworkManagement::IpHelper::MIB_IPINTERFACE_ROW::default();
                unsafe { windows::Win32::NetworkManagement::IpHelper::InitializeIpInterfaceEntry(&mut row) };
                row.Family = AF_INET;
                row.InterfaceIndex = ifindex;
                if unsafe { GetIpInterfaceEntry(&mut row) } == ERROR_SUCCESS {
                    row.SitePrefixLength = 0;
                    row.UseAutomaticMetric = true;
                    unsafe {
                        let _ = SetIpInterfaceEntry(&mut row);
                    }
                }
            }
        }
        self.applied_metric_ifindex = None;
    }

    pub fn teardown(&mut self) {
        for (d, g, i) in self.added.drain(..) {
            if let Err(e) = del(d, g, i) {
                log::warn!("RouteGuard teardown {d} failed: {e:#}");
            }
        }
        self.restore_metric();
    }
}

/// 启动清残留：删指向 dests 的 /32（仅 NETMGMT 协议，避免误删用户静态路由）。
pub fn cleanup_stale(dests: &[Ipv4Addr]) {
    let mut table: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
    if unsafe { GetIpForwardTable2(AF_INET, &mut table) } != ERROR_SUCCESS {
        return;
    }
    let n = unsafe { (*table).NumEntries } as usize;
    for i in 0..n {
        let row = unsafe { &(*table).Table[i] };
        if row.Protocol != MIB_IPPROTO_NETMGMT || row.DestinationPrefix.PrefixLength != 32 {
            continue;
        }
        let hop = row.NextHop.Ipv4.sin_addr;
        let hop_u32 = unsafe { hop.S_un.S_addr };
        let hop_ip = Ipv4Addr::from(hop_u32);
        let dest_ip = Ipv4Addr::from(unsafe { row.DestinationPrefix.Prefix.Ipv4.sin_addr.S_un.S_addr });
        if dests.contains(&dest_ip) {
            if let Err(e) = del(dest_ip, hop_ip, row.InterfaceIndex) {
                log::warn!("cleanup_stale del {dest_ip} failed: {e:#}");
            } else {
                log::info!("Removed stale /32 route to {dest_ip} via {hop_ip}");
            }
        }
    }
    unsafe { windows::Win32::NetworkManagement::IpHelper::FreeMibTable(table.cast()) };
}
```

实现注意：
- `IN_ADDR_0` 是 union：`S_un.S_addr` 读取按 cargo check 提示调整（也可能字段名 `S_addr` 直取）。`Ipv4Addr::from(u32)` 与 `u32::from(Ipv4Addr)` 均为 std 现成。
- `FreeMibTable` 在 IpHelper（确认存在；若无此名以 check 为准）。
- 该文件无 Linux 测试——交叉编译 + clippy 为验证门。

- [ ] **Step 2: Verify**
```bash
cargo check --target x86_64-pc-windows-msvc
cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings
cargo test && cargo fmt
```
- [ ] **Step 3: Commit**
```bash
git add src/wireless/routes.rs src/wireless/mod.rs
git commit -m "feat(wireless): /32 route guard + WLAN metric suppression with paired rollback"
```

---

### Task 7: `wireless/wlan.rs` — WlanAPI（netsh 兜底）

**Files:**
- Create: `src/wireless/wlan.rs`
- Modify: `src/wireless/mod.rs`（放开 `#[cfg(windows)] pub mod wlan;`）
- Modify: `Cargo.toml`（windows features 加 `"Win32_NetworkManagement_Wifi"`）

**Interfaces (consumed by Tasks 9, 10):**
```rust
#[cfg(windows)]
pub fn associate(profile: &str) -> anyhow::Result<()>    // WlanConnect，失败回退 netsh wlan connect
#[cfg(windows)]
pub fn disassociate() -> anyhow::Result<()>              // WlanDisconnect，失败回退 netsh wlan disconnect
#[cfg(windows)]
pub fn associated() -> bool                              // WlanEnumInterfaces state==connected（不解析 netsh 文本，躲 GBK）
```

- [ ] **Step 1: Implement** — Cargo.toml windows features 列表加一行 `"Win32_NetworkManagement_WiFi",`（目录名 `WiFi`，feature 名 `Wifi`——已对 vendored crate 核实）。`src/wireless/wlan.rs`:

```rust
//! WLAN 关联控制：WlanAPI 直调（Session 0 可用、全用户 profile），netsh 兜底
//! （仅动作命令，不解析输出文本——中文系统 netsh 文本 GBK 且本地化，不可解析）。

use anyhow::{anyhow, bail, Result};
use windows::core::PCWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::NetworkManagement::WiFi::{
    dot11_BSS_type_infrastructure, wlan_connection_mode_profile, wlan_interface_state_connected,
    WlanCloseHandle, WlanConnect, WlanDisconnect, WlanEnumInterfaces, WlanFreeMemory,
    WlanOpenHandle, WLAN_CONNECTION_PARAMETERS, WLAN_INTERFACE_INFO_LIST, WLAN_INTERFACE_STATE,
};

const CLIENT_VERSION: u32 = 2; // WLAN_CLIENT_VERSION_LONGHORN（Vista+）

fn with_handle<T>(f: impl FnOnce(HANDLE) -> Result<T>) -> Result<T> {
    let mut negotiated: u32 = 0;
    let mut handle = HANDLE::default();
    let err = unsafe { WlanOpenHandle(CLIENT_VERSION, None, &mut negotiated, &mut handle) };
    if err != 0 {
        bail!("WlanOpenHandle failed: {err}");
    }
    let out = f(handle);
    unsafe {
        let _ = WlanCloseHandle(handle, None);
    }
    out
}

/// 首个 WLAN 接口 GUID（单无线网卡机型，够用）。
fn first_interface(handle: HANDLE) -> Result<windows::core::GUID> {
    let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
    let err = unsafe { WlanEnumInterfaces(handle, None, &mut list) };
    if err != 0 {
        bail!("WlanEnumInterfaces failed: {err}");
    }
    let n = unsafe { (*list).dwNumberOfItems };
    let guid = if n == 0 {
        None
    } else {
        Some(unsafe { (*list).InterfaceInfo[0].InterfaceGuid })
    };
    unsafe {
        WlanFreeMemory(list.cast());
    }
    guid.ok_or_else(|| anyhow!("no WLAN interface present"))
}

fn wlanapi_connect(profile: &str) -> Result<()> {
    with_handle(|h| {
        let guid = first_interface(h)?;
        let mut profile_w: Vec<u16> = profile.encode_utf16().chain(std::iter::once(0)).collect();
        let params = WLAN_CONNECTION_PARAMETERS {
            wlanConnectionMode: wlan_connection_mode_profile,
            strProfile: PCWSTR(profile_w.as_mut_ptr()),
            pDot11Ssid: std::ptr::null_mut(),
            pDesiredBssidList: std::ptr::null_mut(),
            dot11BssType: dot11_BSS_type_infrastructure,
            dwFlags: 0,
        };
        let err = unsafe { WlanConnect(h, &guid, &params, None) };
        if err != 0 {
            bail!("WlanConnect({profile}) failed: {err}");
        }
        Ok(())
    })
}

fn wlanapi_disconnect() -> Result<()> {
    with_handle(|h| {
        let guid = first_interface(h)?;
        let err = unsafe { WlanDisconnect(h, &guid, None) };
        if err != 0 {
            bail!("WlanDisconnect failed: {err}");
        }
        Ok(())
    })
}

fn netsh(args: &[&str]) -> Result<()> {
    let out = std::process::Command::new("netsh").args(args).output()?;
    if !out.status.success() {
        bail!("netsh {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

/// 关联 profile（WlanAPI 失败回退 netsh；两种后端可切换，ADR-0005）。
pub fn associate(profile: &str) -> Result<()> {
    match wlanapi_connect(profile) {
        Ok(()) => Ok(()),
        Err(e) => {
            log::warn!("WlanApi connect failed, falling back to netsh: {e:#}");
            netsh(&["wlan", "connect", &format!("name={profile}")])
        }
    }
}

pub fn disassociate() -> Result<()> {
    match wlanapi_disconnect() {
        Ok(()) => Ok(()),
        Err(e) => {
            log::warn!("WlanApi disconnect failed, falling back to netsh: {e:#}");
            netsh(&["wlan", "disconnect"])
        }
    }
}

/// 是否已关联（state==connected）。不解析 netsh 文本（locale）。
pub fn associated() -> bool {
    with_handle(|h| {
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        let err = unsafe { WlanEnumInterfaces(h, None, &mut list) };
        if err != 0 {
            return Ok(false);
        }
        let n = unsafe { (*list).dwNumberOfItems };
        let mut hit = false;
        for i in 0..n as usize {
            let state: WLAN_INTERFACE_STATE = unsafe { (*list).InterfaceInfo[i].isState };
            if state == wlan_interface_state_connected {
                hit = true;
                break;
            }
        }
        unsafe {
            WlanFreeMemory(list.cast());
        }
        Ok(hit)
    })
    .unwrap_or(false)
}
```

（若 `WLAN_INTERFACE_STATE` 比较需要 `.0` 数值比较，以 cargo check 为准微调。）

- [ ] **Step 2: Verify** — 交叉编译 + clippy + fmt（同 Task 6 命令组）。
- [ ] **Step 3: Commit**
```bash
git add src/wireless/wlan.rs src/wireless/mod.rs Cargo.toml Cargo.lock
git commit -m "feat(wireless): WLAN associate/disassociate via WlanApi with netsh fallback"
```

---

### Task 8: `wireless/portal.rs` win half — `portal_get`（绑源 GET 带 body）

**Files:**
- Modify: `src/wireless/portal.rs`（追加 `#[cfg(windows)] mod win` 段 + re-export）

**Interfaces (consumed by Tasks 9, 10):**
```rust
#[cfg(windows)]
pub async fn portal_get(src_ip: Ipv4Addr, url: &str) -> Option<(u16, String)>  // (status, body)
```

- [ ] **Step 1: Implement** — 复用 probe.rs 的 socket2 手搓模式（不引 HTTP 客户端 crate）：

```rust
#[cfg(windows)]
mod win {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    use std::time::Duration;

    use socket2::{Domain, Protocol, Socket, Type};
    use tokio::task::spawn_blocking;

    const TIMEOUT: Duration = Duration::from_secs(3);
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
        let req = format!("GET {path} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: {PORTAL_UA}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).ok()?;
        let mut buf = Vec::new();
        stream.take(MAX_RESPONSE).read_to_end(&mut buf).ok()?;
        let text = String::from_utf8_lossy(&buf);
        let status = parse_status(text.split("\r\n").next()?)?;
        let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
        Some((status, body))
    }

    pub async fn portal_get(src_ip: Ipv4Addr, url: &str) -> Option<(u16, String)> {
        spawn_blocking(move || portal_get_blocking(src_ip, &url))
            .await
            .unwrap_or(None)
    }
}

#[cfg(windows)]
pub use win::portal_get;
```

注意：URL host 是 `IPv4:port` 字面量（config 校验保证），无 DNS；上面 addr 解析兼容 `IP:port` 与默认 80 两种。`crate::probe::parse_http_probe_target` 亦可复用——实现取更直白的如上写法即可，保持单一实现。

- [ ] **Step 2: Verify** — 交叉编译 + clippy + fmt；`cargo test` 仍全绿。
- [ ] **Step 3: Commit**
```bash
git add src/wireless/portal.rs
git commit -m "feat(wireless): bound-source portal HTTP GET returning body"
```

---

### Task 9: runtime.rs — wireless manager actor + SetMode 接线

**Files:**
- Modify: `src/runtime.rs`
- Modify: `src/service.rs`（`start_all` 调用点传 config 路径）
- Modify: `src/lib.rs`（无需——runtime 已注册）

**Interfaces:**
```rust
// start_all 签名变化：
pub fn start_all(cfg: Config, cfg_path: std::path::PathBuf, stop: CancellationToken) -> Result<()>
```

- [ ] **Step 1: Implement** — `src/runtime.rs` 改动清单（一次提交内完成，逻辑正确性由 Task 4 的 Brain 测试背书 + 交叉编译验证）：

1. `start_all(cfg, cfg_path, stop)`；`run(cfg, cfg_path, stop)` 签名同步。
2. `run` 内、watchdog 装配后新增（wired 广播 + wireless actor）：
```rust
        let init_mode = cfg.wireless.mode;
        let (mode_tx, mode_rx) = watch::channel(init_mode);
        let (wired_tx, wired_rx) = watch::channel(false);
        let (wl_tx, mut wl_rx) = watch::channel(WirelessSnapshot::default());
        let (ev_tx, mut ev_rx) = mpsc::channel::<String>(64);
        let mut events = EventLog::new();
        events.push(unix_now(), &format!("Service started, mode {}", mode_text(init_mode)));
        if cfg.wireless.enabled {
            let mcfg = WirelessCfg-clone-into-manager-args; // profile/portal_url/wlan_ac_ip/probe_host/takeover/release/probe_interval/standby_metric/user/pass
            let mstop = stop.child_token();
            let ev_tx = ev_tx.clone();
            tokio::spawn(async move {
                wireless_manager(mcfg, mstop, mode_rx, wired_rx, wl_tx, ev_tx).await;
            });
        }
```
   （`mode_text`/`unix_now` 为本文件小 helper：`fn unix_now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }`；`mode_text` 直接 match。）
3. wireless_manager 本体（win 模块内自由 async fn，约 120 行）：
```rust
    struct ManagerCfg {
        profile: String, portal_url: String, wlan_ac_ip: String, probe_host: String,
        user: String, pass: String,
        takeover_after: u64, release_after: u64, probe_interval: u64, standby_metric: u32,
    }

    async fn wireless_manager(
        cfg: ManagerCfg, stop: CancellationToken,
        mut mode_rx: watch::Receiver<NetMode>,
        mut wired_rx: watch::Receiver<bool>,
        wl_tx: watch::Sender<WirelessSnapshot>, ev_tx: mpsc::Sender<String>,
    ) {
        let start = Instant::now();
        let now = || start.elapsed().as_secs();
        let portal_ip = crate::probe::parse_http_probe_target(&cfg.portal_url).map(|(ip, _)| ip);
        let probe_ip: Option<Ipv4Addr> = cfg.probe_host.parse().ok();
        let (portal_ip, probe_ip) = match (portal_ip, probe_ip) {
            (Some(p), Some(q)) => (p, q),
            _ => { log::error!("Wireless: portal_url/probe_host invalid, manager disabled"); return; }
        };
        routes::cleanup_stale(&[portal_ip, probe_ip]);
        let mut brain = Brain::new(*mode_rx.borrow_and_update(), cfg.takeover_after, cfg.release_after, cfg.probe_interval);
        let mut guard = routes::RouteGuard::new();
        let mut join_since: Option<u64> = None;
        let mut verdict: Option<ProbeVerdict> = None;
        let ev = |tx: &mpsc::Sender<String>, msg: &str| { let _ = tx.try_send(msg.to_string()); };
        loop {
            if stop.is_cancelled() { break; }
            brain.set_mode(*mode_rx.borrow_and_update());
            let (eth_up, assoc, wlan) = tokio::task::spawn_blocking(|| {
                (adapter::ethernet_link_up(), wlan::associated(), adapter::wlan_adapter())
            }).await.unwrap_or((false, false, None));
            let world = World {
                now: now(), eth_link_up: eth_up,
                wired_connected: *wired_rx.borrow_and_update(),
                wlan_associated: assoc, wlan_ip: wlan.is_some(), probe: verdict,
            };
            match brain.decide(&world) {
                Action::None => {}
                Action::Associate => {
                    ev(&ev_tx, "Wireless: associating to campus SSID");
                    let p = cfg.profile.clone();
                    if let Err(e) = tokio::task::spawn_blocking(move || wlan::associate(&p)).await.unwrap_or_else(|_| Err(anyhow!("join task panicked"))) {
                        log::warn!("Wireless associate failed (will retry): {e:#}");
                    }
                    join_since = Some(now());
                }
                Action::Disassociate => {
                    ev(&ev_tx, "Wireless: releasing (wired healthy)");
                    guard.teardown();
                    let _ = tokio::task::spawn_blocking(wlan::disassociate).await;
                    verdict = None;
                }
                Action::PortalAuth => {
                    let Some(a) = wlan else { brain.on_auth(false, "wlan ip lost", now()); continue; };
                    let Some(gw) = a.gateway else { brain.on_auth(false, "wlan gateway missing", now()); continue; };
                    guard.ensure(&[portal_ip, probe_ip], gw, a.ifindex);
                    let url = portal::build_login_url(&cfg.portal_url, &cfg.user, &cfg.pass, a.ipv4, &cfg.wlan_ac_ip);
                    log::info!("Wireless: portal login from {} ({})", a.ipv4, portal::redact_query(&url));
                    let r = portal::portal_get(a.ipv4, &url).await;
                    let (ok, msg) = match r {
                        Some((code, body)) => match parse_portal_reply(&body) {
                            PortalResult::Success => (true, "login ok".to_string()),
                            PortalResult::Failure(m) => (false, m),
                            PortalResult::Malformed => (false, format!("unparseable reply (HTTP {code})")),
                        },
                        None => (false, "no reply".to_string()),
                    };
                    if ok { ev(&ev_tx, "Wireless: portal login success"); }
                    else { ev(&ev_tx, &format!("Wireless: portal login failed: {msg}")); }
                    brain.on_auth(ok, &msg, now());
                }
                Action::ProbeNow => {
                    let Some(a) = wlan else { verdict = Some(ProbeVerdict::LinkDown); continue; };
                    let url = format!("http://{}/", cfg.probe_host);
                    let v = probe_once(a.ipv4, a.gateway, &url).await;
                    if v != ProbeVerdict::Alive {
                        ev(&ev_tx, &format!("Wireless probe: {v:?}"));
                    }
                    verdict = Some(v);
                }
            }
            // standby metric 压制（幂等）；exclusive 且非在线则还原
            if let Some(a) = &wlan {
                if brain-mode == standby（从 mode_rx borrow） { guard.set_standby_metric(a.ifindex, cfg.standby_metric); }
            }
            if brain-mode == exclusive { guard.release_metric_only(); } // 新增小方法：仅还原 metric，不动路由
            // Joining 超时 → 复位
            if brain.phase() == WPhase::Joining && join_since.map_or(false, |t| now() - t > JOIN_TIMEOUT_SECS) {
                ev(&ev_tx, "Wireless: join timeout, restarting");
                brain.restart();
                join_since = None;
            }
            let mut snap = brain.snapshot();
            snap.ip = wlan.map(|a| a.ipv4.to_string());
            let _ = wl_tx.send(snap);
            tokio::select! {
                _ = stop.cancelled() => break,
                _ = sleep(Duration::from_secs(2)) => {}
            }
        }
        guard.teardown();
        let _ = tokio::task::spawn_blocking(wlan::disassociate).await;
        let _ = wl_tx.send(WirelessSnapshot::default());
        ev(&ev_tx, "Wireless: manager stopped");
    }
```
   （`release_metric_only`：Task 6 的 `restore_metric` 改为 `pub(crate)` 级小包装，或把 set_standby_metric/restore 拆两个 pub 方法——实现时二选一，保语义：exclusive 模式下不压制。）
4. 主循环改动：
   - `cmd_rx` match 增 `Some(Command::SetMode { mode })` arm：`mode_tx.send(mode)`（replace 语义即最新胜）+ `cfg.wireless.mode = mode; cfg.save(&cfg_path)`（失败 warn 不阻断）+ `events.push(unix_now(), &format!("Mode switched to {}", ...))` + 立即推快照。
   - select 增两臂：`_ = wl_rx.changed()`（Ok 时重推快照）与 `ev = ev_rx.recv()`（Some 时 events.push + 重推快照）。
   - 快照组装两处（heartbeat-error 路径 + run_once 后）统一经新 helper：
```rust
        let compose = |mut snap: StateSnapshot| {
            snap.ip = adapter::ppp_adapter_ip().map(|ip| ip.to_string());
            snap.heartbeat = hb_tx.borrow().clone();
            snap.mode = *mode_tx.borrow();
            snap.wireless = wl_tx.borrow().clone();
            snap.events = events.ring().clone();
            snap
        };
```
   - `watchdog.run_once()` 后：`wired_tx.send(matches!(watchdog.snapshot().status, SessionStatus::Connected));`
5. `service.rs` 的 `start_all` 调用点补 `cfg_path` 实参（该函数里已有 config 路径变量，按名传入）。

- [ ] **Step 2: Verify**
```bash
cargo check --target x86_64-pc-windows-msvc
cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings
cargo test && cargo fmt
```
- [ ] **Step 3: Commit**
```bash
git add src/runtime.rs src/service.rs src/wireless/
git commit -m "feat(runtime): wireless manager actor, SetMode wiring + persistence, event ring"
```

---

### Task 10: CLI — `wireless test/off/standby` + status 输出

**Files:**
- Modify: `src/cli.rs`
- Create: `src/wireless/test.rs`（放开 mod.rs 的 `#[cfg(windows)] pub mod test;`）
- Modify: `src/ipc/client.rs`（status_once 两行）

**Interfaces:**
```rust
// cli.rs
Wireless { #[command(subcommand)] action: WirelessAction }
enum WirelessAction { Test, Off, Standby }
// wireless/test.rs
pub fn cli_test(cfg_path: &std::path::Path) -> anyhow::Result<()>
```

- [ ] **Step 1: Implement** — `src/cli.rs` 增子命令（doc comment 全英文）：
```rust
    /// Wireless (campus WiFi) utilities
    Wireless {
        #[command(subcommand)]
        action: WirelessAction,
    },
```
```rust
#[derive(Subcommand)]
pub enum WirelessAction {
    /// Join campus SSID once, try one portal login, print reply (field check)
    Test,
    /// Switch service to wired-exclusive mode now
    Off,
    /// Switch service to wired+wireless standby mode now
    Standby,
}
```
dispatch 增三个 arm（Windows/非 Windows 各一对，模式同现有）：
```rust
        #[cfg(windows)]
        Cmd::Wireless { action: WirelessAction::Test } => crate::wireless::test::cli_test(&cli.config),
        #[cfg(not(windows))]
        Cmd::Wireless { action: WirelessAction::Test } => bail!("wireless is only supported on Windows"),
        #[cfg(windows)]
        Cmd::Wireless { action: WirelessAction::Off } => wireless_set_mode(NetMode::WiredExclusive),
        #[cfg(windows)]
        Cmd::Wireless { action: WirelessAction::Standby } => wireless_set_mode(NetMode::WiredPlusStandby),
        #[cfg(not(windows))]
        Cmd::Wireless { action: _ } => bail!("wireless is only supported on Windows"),
```
（`wireless_set_mode` 是 cli.rs 内小 helper：current_thread runtime + PipeClient connect + send_cmd(SetMode) + next_state 打印确认行 "Mode set: ..."。）

`src/wireless/test.rs`：
```rust
//! `gdut-net wireless test`：现场验证 portal 常量（连 SSID → 等 IP → 临时 /32 →
//! 一次 login → 打印回包 → 自回滚断开）。见 spec §10。

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::adapter;
use crate::config::Config;
use crate::wireless::{portal, routes, wlan};

const WAIT_IP: Duration = Duration::from_secs(20);

pub fn cli_test(cfg_path: &Path) -> Result<()> {
    let cfg = Config::load(cfg_path)?;
    let pass = crate::crypto::unprotect(&cfg.account.password_blob)
        .context("Failed to decrypt password_blob (run install first)")?;
    let portal_ip = crate::probe::parse_http_probe_target(&cfg.wireless.portal_url)
        .map(|(ip, _)| ip)
        .context("wireless.portal_url invalid")?;

    println!("Joining SSID profile {:?} ...", cfg.wireless.profile);
    wlan::associate(&cfg.wireless.profile)
        .context("Failed to associate (profile missing? connect to the SSID manually once to create it)")?;
    let deadline = Instant::now() + WAIT_IP;
    let adapter = loop {
        if let Some(a) = adapter::wlan_adapter() { break a; }
        if Instant::now() > deadline { bail!("Timed out waiting for WLAN IPv4 (DHCP)"); }
        std::thread::sleep(Duration::from_secs(2));
    };
    let gw = adapter.gateway.context("WLAN has no gateway — portal unreachable")?;
    println!("WLAN up: {} gw {} (if {})", adapter.ipv4, gw, adapter.ifindex);

    // 自包含 /32：结束即删（含失败路径——scope guard 手法）
    struct Teardown(routes::RouteGuard);
    impl Drop for Teardown {
        fn drop(&mut self) {
            self.0.teardown();
            let _ = wlan::disassociate();
        }
    }
    let mut guard = routes::RouteGuard::new();
    guard.ensure(&[portal_ip], gw, adapter.ifindex);
    let _teardown = Teardown(guard);

    let url = portal::build_login_url(
        &cfg.wireless.portal_url, &cfg.account.student_id, &pass, adapter.ipv4, &cfg.wireless.wlan_ac_ip,
    );
    println!("GET {}", portal::redact_query(&url));
    match portal::portal_get(adapter.ipv4, &url) {
        Some((code, body)) => {
            println!("HTTP {code}");
            println!("Body: {body}");
            match portal::parse_portal_reply(&body) {
                portal::PortalResult::Success => println!("RESULT: SUCCESS"),
                portal::PortalResult::Failure(m) => println!("RESULT: FAILURE ({m})"),
                portal::PortalResult::Malformed => println!("RESULT: MALFORMED (check wlan_ac_ip / portal_url)"),
            }
        }
        None => println!("RESULT: NO REPLY (route/TUN interference? see ADR-0005 §routes)"),
    }
    println!("Cleaning up (disconnecting WLAN) ...");
    drop(_teardown);
    println!("Done.");
    Ok(())
}
```

`src/ipc/client.rs` `status_once` 打印段补两行：
```rust
        println!("Mode:     {}", s.mode_text());
        println!("Wireless: {}", s.wireless_text());
        println!("Events:   {} recent", s.events.len());
```

- [ ] **Step 2: Verify** — 交叉编译 + clippy + fmt + `cargo test`。
- [ ] **Step 3: Commit**
```bash
git add src/cli.rs src/wireless/test.rs src/wireless/mod.rs src/ipc/client.rs
git commit -m "feat(cli): wireless test/off/standby; status prints mode + wireless + events"
```

---

### Task 11: 托盘菜单重做（checkable 模式 + 状态图标）

**Files:**
- Modify: `src/tray/mod.rs`

**Interfaces (consumed by Task 12):**
```rust
// 模块内新增（不导出）：
enum IconKind { WiredUp, WirelessUp, Backoff, Down }
fn icon_kind(s: Option<&StateSnapshot>) -> IconKind
fn send_set_mode(mode: NetMode)                       // 同 send_redial 模式
// panel::show 签名在本任务先保持不变，Task 12 扩展
```

- [ ] **Step 1: Implement** — `src/tray/mod.rs` 改动：

1. use 增：`use tray_icon::menu::{CheckMenuItem, MenuEvent, MenuItem, PredefinedMenuItem};` 与 `use crate::ipc::protocol::NetMode;`（Command/StateSnapshot 已有）。
2. 菜单构建替换：
```rust
    let status_item = MenuItem::new("Wired: Disconnected", false, None);
    let sep1 = PredefinedMenuItem::separator();
    let mode_exclusive = CheckMenuItem::new("Wired only (auto wireless takeover)", true, true, None);
    let mode_standby = CheckMenuItem::new("Wired + wireless standby", true, false, None);
    let sep2 = PredefinedMenuItem::separator();
    let redial_item = MenuItem::new("Redial now", true, None);
    let panel_item = MenuItem::new("Details", true, None);
    let sep3 = PredefinedMenuItem::separator();
    let quit_item = MenuItem::new("Exit", true, None);
    menu.append_items(&[&status_item, &sep1, &mode_exclusive, &mode_standby, &sep2, &redial_item, &panel_item, &sep3, &quit_item])?;
```
3. `tray_icon_rgba` 泛化为 `icon_rgba(color: [u8; 3]) -> Vec<u8>`（保留 2px 透明边、32×32 方块），四种色：绿 `0x2e,0xc4,0x8a`、蓝 `0x30,0x9c,0xdc`、黄 `0xe0,0xb0,0x00`、灰 `0x88,0x88,0x88`。预建 4 个 `tray_icon::Icon`。
4. 图标/tooltip/文本随快照更新（泵线程每拍，幂等 set）：
```rust
    let status_text = |state: Option<&StateSnapshot>| match state {
        None => "Wired: Disconnected".to_string(),
        Some(s) => format!("Wired: {} · WiFi: {}", s.status_text(), s.wireless_text()),
    };
    // 每拍：
    if let Ok(guard) = snapshot.lock() {
        let want_status = status_text(guard.as_ref());
        if status_item.text() != want_status { status_item.set_text(want_status); }
        let mode = guard.as_ref().map(|s| s.mode).unwrap_or_default();
        mode_exclusive.set_checked(mode == NetMode::WiredExclusive);
        mode_standby.set_checked(mode == NetMode::WiredPlusStandby);
        let kind = icon_kind(guard.as_ref());
        if kind != last_kind { tray.set_icon(Some(&icons[kind as usize])); tray.set_tooltip(Some(...)).ok(); last_kind = kind; }
    }
```
   `let tray = TrayIconBuilder...build()?;`（去掉原 `_tray` 下划线，保留变量）。`IconKind` 转 usize 用 `as`（无字段 enum）或 match——clippy 若拒绝 `as`，用 `const KINDS: [IconKind; 4]` + `iter().position()`。
   `icon_kind`：
```rust
    fn icon_kind(s: Option<&StateSnapshot>) -> IconKind {
        match s {
            None => IconKind::Down,
            Some(s) if s.wireless.phase == WPhase::Online => IconKind::WirelessUp,
            Some(s) => match s.status {
                SessionStatus::Connected => IconKind::WiredUp,
                SessionStatus::Backoff | SessionStatus::AuthFail | SessionStatus::Dialing => IconKind::Backoff,
                SessionStatus::Idle => IconKind::Down,
            },
        }
    }
```
   tooltip 文本：`format!("gdut-net — Wired: {} / WiFi: {}", ...)`（英文）。
5. 菜单事件处理增：
```rust
            if event.id == mode_exclusive.id() {
                send_set_mode(NetMode::WiredExclusive);
            } else if event.id == mode_standby.id() {
                send_set_mode(NetMode::WiredPlusStandby);
            } else if event.id == redial_item.id() { ...
```
6. `send_set_mode`（克隆 `send_redial` 结构，cmd 换 `Command::SetMode { mode }`）。
7. 状态文本两处重复闭包（`run_tray` 内 + `ipc_loop` 内）统一引用 `status_text`——`ipc_loop` 的 push_text 改调共享 fn（放模块级 `fn status_line(s: Option<&StateSnapshot>) -> String`，两处复用，消既有重复）。

- [ ] **Step 2: Verify** — 交叉编译 + clippy + fmt。
- [ ] **Step 3: Commit**
```bash
git add src/tray/mod.rs
git commit -m "feat(tray): checkable mode items, separators, per-state icon and tooltip"
```

---

### Task 12: 面板重做 — eframe glow + default_fonts（ADR-0006）

**Files:**
- Modify: `Cargo.toml`（windows target deps + eframe/winit）
- Rewrite: `src/tray/panel.rs`
- Modify: `src/tray/mod.rs`（show 调用点加 setmode 通道 + 泵线程消费）

**Interfaces:**
```rust
// panel.rs
pub fn show(snapshot: SharedSnapshot, redial_tx: Sender<()>, setmode_tx: Sender<NetMode>)
```

- [ ] **Step 0: 机器验证 gate（先于写代码）** — 在目标机（Intel iGPU 本本）跑已编译好的 spike：
  `/tmp/opencode/panel-spike/target/x86_64-pc-windows-msvc/release/panel-spike.exe`（拷过去运行，确认文字可见、uptime 走秒、关窗进程退；可选负对照：`features=["glow"]` 去掉 `default_fonts` 重编译应复现黑屏）。**Spike 不过 → 不做本任务，改走 B1 原生对话框 fallback（另立任务）。** 机器不可得时标注 TODO-ON-MACHINE 并继续（代码与 spike 同构，风险已对齐）。
- [ ] **Step 1: Cargo.toml** — `[target.'cfg(windows)'.dependencies]` 增：
```toml
eframe = { version = "0.36", default-features = false, features = ["glow", "default_fonts"] }
winit = { version = "0.30", default-features = false }
```
- [ ] **Step 2: Rewrite `src/tray/panel.rs`**（egui 经 `eframe::egui` re-export；线程 + any_thread + glow 全按 spike 已验证结构）：

```rust
//! 详情面板：egui（eframe glow + default_fonts，ADR-0006）。
//! 每次打开起独立线程跑 run_native（with_any_thread），关窗线程退。
//! 旧 MessageBox 实现已删；黑屏根因是 default_fonts 被关，非核显。

use std::sync::mpsc::Sender;

use eframe::egui;
use eframe::egui::{CentralPanel, Grid, ScrollArea, ViewportBuilder};
use eframe::{NativeOptions, Renderer};

use crate::ipc::protocol::{NetMode, StateSnapshot};

use super::SharedSnapshot;

pub fn show(snapshot: SharedSnapshot, redial_tx: Sender<()>, setmode_tx: Sender<NetMode>) {
    let _ = std::thread::Builder::new()
        .name("gdut-net-panel".into())
        .spawn(move || {
            if let Err(e) = run_panel(snapshot, redial_tx, setmode_tx) {
                log::error!("Panel exited: {e:#}");
            }
        });
}

fn run_panel(snapshot: SharedSnapshot, redial_tx: Sender<()>, setmode_tx: Sender<NetMode>) -> anyhow::Result<()> {
    let mut options = NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("gdut-net")
            .with_inner_size(egui::vec2(420.0, 360.0))
            .with_resizable(false),
        renderer: Renderer::Glow,
        ..Default::default()
    };
    options.event_loop_builder = Some(Box::new(|builder| {
        use winit::platform::windows::EventLoopBuilderExtWindows as _;
        builder.with_any_thread(true);
    }));
    eframe::run_native(
        "gdut-net-panel",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::light());
            Ok(Box::new(Panel { snapshot, redial_tx, setmode_tx }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe failed: {e}"))
}

struct Panel {
    snapshot: SharedSnapshot,
    redial_tx: Sender<()>,
    setmode_tx: Sender<NetMode>,
}

impl eframe::App for Panel {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
        CentralPanel::default().show(ctx, |ui| {
            let (wired, wireless, mode, events) = {
                let guard = self.snapshot.lock().expect("snapshot poisoned");
                let s = guard.as_ref();
                (
                    s.map(|s| (s.status_text(), s.uptime_text(), s.ip.clone(), s.redial_attempts, s.heartbeat_text())),
                    s.map(|s| (s.wireless.wireless_phase_text(), s.wireless.ip.clone())),
                    s.map(|s| s.mode),
                    s.map(|s| s.events.iter().cloned().collect::<Vec<_>>()).unwrap_or_default(),
                )
            };
            ui.heading("gdut-net");
            ui.add_space(8.0);
            Grid::new("wired").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                ui.strong("Wired");
                ui.label(wired.as_ref().map(|w| w.0.clone()).unwrap_or_else(|| "No service".into()));
                ui.end_row();
                ui.strong("Uptime");
                ui.label(wired.as_ref().map(|w| w.1.clone()).unwrap_or_else(|| "—".into()));
                ui.end_row();
                ui.strong("Wired IP");
                ui.label(wired.as_ref().and_then(|w| w.2.clone()).unwrap_or_else(|| "—".into()));
                ui.end_row();
                ui.strong("Heartbeat");
                ui.label(wired.map(|w| w.4).unwrap_or_else(|| "—".into()));
                ui.end_row();
                ui.strong("Wireless");
                ui.label(wireless.map(|w| w.0).unwrap_or_else(|| "—".into()));
                ui.end_row();
                ui.strong("Wireless IP");
                ui.label(wireless.and_then(|w| w.1).unwrap_or_else(|| "—".into()));
                ui.end_row();
            });
            ui.add_space(10.0);
            ui.label(egui::RichText::new("Network mode").strong());
            let current = mode.unwrap_or_default();
            if ui.radio(current == NetMode::WiredExclusive, "Wired only (auto wireless takeover)").clicked() {
                let _ = self.setmode_tx.send(NetMode::WiredExclusive);
            }
            if ui.radio(current == NetMode::WiredPlusStandby, "Wired + wireless standby").clicked() {
                let _ = self.setmode_tx.send(NetMode::WiredPlusStandby);
            }
            ui.add_space(10.0);
            if ui.button("Redial now").clicked() {
                let _ = self.redial_tx.send(());
            }
            ui.add_space(10.0);
            ui.label(egui::RichText::new("Recent events").strong());
            ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                for line in events.iter().rev() {
                    ui.monospace(line);
                }
            });
        });
    }
}
```

   依赖的 text helper：`StateSnapshot::wireless_phase_text()`——protocol.rs 里 Task 2 已有 `wireless_text()`；面板拆两行用，直接复用 `s.wireless_text()` 即可（上面 `wireless_phase_text` 若觉得多余就换成 `s.wireless_text()` 一行展示 IP 合并——**实现取合并方案，砍掉 wireless_phase_text**）。drop reason 一行也补上（`last_drop_reason`）。
   注意 borrow：闭包返回 tuple 时 `w.2` 等 move 字段——实现按 clippy 提示整理 clone。
- [ ] **Step 3: `src/tray/mod.rs` 接线**：
```rust
    let (panel_setmode_tx, panel_setmode_rx) = mpsc::channel::<NetMode>();
```
   `panel_item` 分支改 `panel::show(Arc::clone(&snapshot), panel_redial_tx.clone(), panel_setmode_tx.clone());`
   泵线程循环加：
```rust
        while let Ok(mode) = panel_setmode_rx.try_recv() {
            send_set_mode(mode);
        }
```
- [ ] **Step 4: Verify** — 交叉编译 + clippy + fmt + `cargo test`。`mise exec -- cargo xwin build --target x86_64-pc-windows-msvc --release` 确认体积可接受（spike 实测 5.8MB，全量预计 <8MB）。
- [ ] **Step 5: Commit**
```bash
git add Cargo.toml Cargo.lock src/tray/panel.rs src/tray/mod.rs
git commit -m "feat(tray): egui glow panel with default_fonts (fixes black screen root cause, ADR-0006)"
```

---

### Task 13: 文档 + 真机验收清单

**Files:**
- Modify: `docs/acceptance.md`（新增无线章节）
- Modify: `docs/desktop-kit.md`（场景两行）
- Modify: `README.md`（config 样例补 `[wireless]` 段 + CLI 一行）

- [ ] **Step 1: acceptance.md 追加**（英文标题中文正文，跟现有风格）：

```markdown
## 无线接管（v0.3）

前置：`gdut-net.exe install` 过一次；WLAN profile `gdut` 存在（手工连过一次校园 WiFi）。

```powershell
.\gdut-net.exe wireless test    # 一次性现场验证：应打印 WLAN IP/gw、HTTP 200、RESULT: SUCCESS，结束自动断开
.\gdut-net.exe status           # Mode: ... / Wireless: ... 两行出现
```

| # | 标准 | 验证方法 |
|---|---|---|
| 6 | exclusive：拔线 ≤15s 无线可用 | `Get-Content ...log -Wait` 观察 "associating" → "portal login success"；插回线 ≤20s 出现 "releasing"；`netsh wlan show interfaces` 回 Disconnected |
| 7 | standby：拔线零感知 | 托盘切 "Wired + wireless standby"，常驻 ping 窗口拔线观察丢包 ≤2 个；插回线 WLAN 不断（仍 Online） |
| 8 | 模式持久化 | 切 standby → `net stop/start gdut-net` → status 的 Mode 仍为 standby |
| 9 | 路由无残留 | 服务停止后 `route print` 无 `10.0.3.2 /32`、`223.5.5.5 /32`；WLAN metric 还原（`Get-NetIPInterface -InterfaceAlias WLAN`） |
| 10 | TUN 共存 | Mihomo TUN 开着跑 6/7 两项（/32 由服务自管，ADR-0005） |
```

- [ ] **Step 2: desktop-kit.md 场景区补两行**（跟现有 bullet 风格）：
```markdown
- **拔线改无线**：服务自动接管（exclusive 默认）；托盘/面板可切 "Wired + wireless standby" 常备无缝。
- **现场排障**：管理员 `.\gdut-net.exe wireless test` 打一次真实 portal 回包（自回滚，不留状态）。
```
- [ ] **Step 3: README.md** — config 样例块补：
```toml
[wireless]
enabled = true
mode = "wired_exclusive"                # or "wired_plus_standby"
profile = "gdut"                        # Windows WLAN profile (create by joining once)
portal_url = "http://10.0.3.2:801/eportal/portal/login"
wlan_ac_ip = "172.16.254.2"             # HEMC; Longdong/Dongfeng Road unverified
probe_host = "223.5.5.5"
takeover_after_secs = 8
release_after_secs = 10
standby_metric = 10                     # 0 = do not suppress
```
   CLI 表/段补一行 `gdut-net wireless test|off|standby`。Features 段补一句 wireless takeover。
- [ ] **Step 4: Commit**
```bash
git add docs/acceptance.md docs/desktop-kit.md README.md
git commit -m "docs: wireless takeover acceptance checklist + kit scenarios + README config"
```

---

### Task 14: 全量验证 + 收尾

- [ ] **Step 1: 全绿矩阵**

```bash
cargo test
cargo clippy -- -D warnings
cargo fmt --check
cargo check --target x86_64-pc-windows-msvc
cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings
mise exec -- cargo xwin build --target x86_64-pc-windows-msvc --release   # 本地快速产出
ls -la target/x86_64-pc-windows-msvc/release/gdut-net.exe   # 记录体积
```

- [ ] **Step 2: 真机部署验收（用户执行，AI 不可达段）** — 按 `docs/acceptance.md` 无线章节跑 6-10 项；失败项回填本文件勾选状态。部署走既有 `gdut-net-new.exe` + `switch-v4.ps1` A0 流程（AGENTS.md），**不要**直接覆盖在跑 exe。
- [ ] **Step 3: 收尾提交**

```bash
git add -A
git commit -m "chore: wireless takeover + tray redo complete (spec: docs/superpowers/specs/2026-09-09)"
```

---

## Self-Review 记录（已执行）

- **Spec 覆盖**：§2 配置→T3；§4 状态机→T4+T9；§5 路由→T6；§7 IPC→T2/T9；§8 托盘→T11/T12；§9 事件环→T2（EventLog）+T9（push 点）；§10 CLI→T10；§11 测试→各任务步骤；§13 验收→T13。§6 standby metric 在 T6（set/restore）+T9（时机）。
- **类型一致性**：`NetMode/WPhase/WirelessSnapshot/EventLog` 定义于 T2，T3/T4/T9/T10/T11/T12 消费签名一致；`AdapterInfo.ifindex`（T5）→T6/T9；`RouteGuard`（T6）→T9/T10；`wlan::{associate,disassociate,associated}`（T7）→T9/T10；`portal::{build_login_url,parse_portal_reply,redact_query,portal_get}`（T1/T8）→T9/T10。
- **占位符扫描**：T9 wireless_manager 代码块中 `WirelessCfg-clone-into-manager-args` 与 `brain-mode == standby（从 mode_rx borrow）` 两处是**伪代码指示**，实现者须落成真代码（ManagerCfg 结构已给全字段）；`release_metric_only` 指示 T6 拆方法。其余任务代码可直接落地。
- **已知留白**（有意，非占位）：portal 会话寿命/keepalive 需求由 T13 观察期回答；`wlan_ac_ip` 龙洞/东风路校区未验证（配置可改）；Session 0 WlanConnect 不可用时 netsh 兜底已内建，真机验证归 T13。
