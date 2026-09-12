# Deepening（机械波）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 按 `2026-09-12-deepening-design.md` 的第一波，把状态渲染、盘上布局/日志策略、死接口、Win32 原语、HTTP/portal 五处浅 module/漂移深挖成单一 interface，行为冻结、全部以测试或交叉编译 gates 固定。

**Architecture:** 新 cfg-free module `status` / `paths` / `win32(wide)` / `http(纯解析)` 承载唯一实现；Windows 半边只留 adapter（socket/注册表/egui/托盘图标）。每任务是独立可合并的机械重构，按 spec 的 D3 顺序执行。

**Tech Stack:** Rust 1.97, windows 0.62, socket2 0.6, tokio 1.53, eframe/egui 0.36（本波不碰 egui 视觉）。

**Spec:** `docs/superpowers/specs/2026-09-12-deepening-design.md`。开工前读 `CONTEXT.md` 的 Rules 与陷阱。

## Global Constraints

- 控制台输出（CLI/日志/脚本）**英文**；GUI 文案**中文**；源码注释中英文皆可。
- CLI 行为冻结：子命令、提示、退出码、`status` 输出文本逐字不变（`switch-v4.ps1` 依赖 `Dial succeeded` 等英文日志，不依赖 `status` 文本，但改文本仍属违约）。
- 纯逻辑（status/paths/http 解析/win32::wide）必须 cfg-free、Linux 可测；Win32 胶水仅 `cfg(windows)`。
- 每个任务收尾必须全绿：
  `cargo test && cargo clippy -- -D warnings && cargo fmt --check`
  以及
  `cargo check --target x86_64-pc-windows-msvc && cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings`
- 提交风格：`feat:/fix:/refactor:/test:/docs:`。
- 不新增依赖；不改 `Cargo.toml`（除本波明确注明）。
- 每步 TDD：先写失败测试，再实现，再跑 gates。

## File Map

- Create `src/status.rs` — snapshot → 状态语义/词表的唯一 module。
- Create `src/paths.rs` — ProgramData/Program Files 布局常量与派生。
- Create `src/win32.rs` — `wide()` + `#[cfg(windows)] reg::{set_string,set_dword}`。
- Create `src/http.rs` — HTTP/1.0 GET + 纯解析。
- Create `tests/status.rs`、`tests/paths.rs`、`tests/win32.rs`、`tests/http.rs`。
- Modify `src/lib.rs`（注册新 module）、`src/ipc/protocol.rs`、`src/ipc/client.rs`、`src/cli.rs`、`src/setup/work.rs`、`src/tray/mod.rs`、`src/tray/gui.rs`、`src/config.rs`、`src/logging.rs`、`src/service.rs`、`src/setup/mod.rs`、`src/setup/ui.rs`、`src/eventlog.rs`、`src/shell.rs`、`src/crypto.rs`、`src/probe.rs`、`src/wireless/portal.rs`、`tests/ipc_protocol.rs`。

---

### Task 1: 状态呈现 module（status）

**Files:**
- Create: `src/status.rs`
- Modify: `src/lib.rs`（`pub mod status;`，插在 `pub mod shell_shortcuts;` 与 `#[cfg(windows)] pub mod tray;` 之间）
- Modify: `src/ipc/protocol.rs`（删 `status_text/heartbeat_text/mode_text/wireless_text`，保留 `uptime_text/format_uptime` 与全部 serde 类型）
- Modify: `src/ipc/client.rs:95-105`、`src/cli.rs:156`、`src/setup/work.rs:439`
- Modify: `src/tray/mod.rs`（`icon_kind`、`status_line`、局部词表）
- Modify: `src/tray/gui.rs`（`status_view` 与无线相位词表；色板/布局不动）
- Test: Create `tests/status.rs`; Modify `tests/ipc_protocol.rs`（删 `snapshot_texts`，其主题迁往 `tests/status.rs`）

**Interfaces:**
- Consumes: `crate::ipc::protocol::{HeartbeatStatus, NetMode, SessionStatus, StateSnapshot, WPhase, WirelessSnapshot}`。
- Produces（后续任务/渲染端依赖）:

```rust
pub enum Light { Wired, Wireless, Busy, Off }
pub enum Primary { ServiceDown, Connected, WirelessOnline, Dialing, Backoff, AuthFail, Idle }
pub struct StatusView { pub primary: Primary, pub light: Light }
pub fn view(s: Option<&StateSnapshot>) -> StatusView;
impl Primary { pub fn word_zh(self) -> &'static str; pub fn egress_zh(self) -> &'static str; }
pub fn session_zh(SessionStatus) -> &'static str;
pub fn session_en(SessionStatus) -> &'static str;
pub fn wphase_zh(WPhase) -> &'static str;
pub fn wphase_en(WPhase) -> &'static str;
pub fn heartbeat_en(&HeartbeatStatus) -> String;
pub fn mode_en(NetMode) -> &'static str;
pub fn wireless_en(&WirelessSnapshot) -> String;
pub fn status_line_zh(Option<&StateSnapshot>) -> String;
```

- [ ] **Step 1: 写失败测试** — `tests/status.rs`：

```rust
use gdut_net::ipc::protocol::{
    HeartbeatStatus, NetMode, SessionStatus, StateSnapshot, WPhase, WirelessSnapshot,
};
use gdut_net::status::{session_en, status_line_zh, view, Light, Primary};

fn snap() -> StateSnapshot {
    StateSnapshot {
        status: SessionStatus::Connected,
        since_unix: None,
        ip: None,
        last_drop_reason: None,
        redial_attempts: 0,
        heartbeat: HeartbeatStatus::Off,
        mode: NetMode::WiredExclusive,
        wireless: WirelessSnapshot::default(),
        events: Default::default(),
    }
}

#[test]
fn no_snapshot_is_service_down() {
    let v = view(None);
    assert_eq!((v.primary, v.light), (Primary::ServiceDown, Light::Off));
    assert_eq!(v.primary.word_zh(), "服务未运行");
    assert_eq!(v.primary.egress_zh(), "—");
    assert_eq!(status_line_zh(None), "服务未运行");
}

#[test]
fn wired_connected_beats_wireless_online() {
    let mut s = snap();
    s.wireless.phase = WPhase::Online;
    let v = view(Some(&s));
    assert_eq!((v.primary, v.light), (Primary::Connected, Light::Wired));
    assert_eq!(v.primary.word_zh(), "已连接");
    assert_eq!(v.primary.egress_zh(), "有线");
}

#[test]
fn wireless_online_when_wired_not_connected() {
    let mut s = snap();
    s.status = SessionStatus::Backoff;
    s.wireless.phase = WPhase::Online;
    let v = view(Some(&s));
    assert_eq!((v.primary, v.light), (Primary::WirelessOnline, Light::Wireless));
    assert_eq!(v.primary.word_zh(), "无线接管");
    assert_eq!(v.primary.egress_zh(), "无线");
}

#[test]
fn in_progress_and_failure_map_to_busy_light() {
    for (st, primary, word) in [
        (SessionStatus::Dialing, Primary::Dialing, "拨号中"),
        (SessionStatus::Backoff, Primary::Backoff, "重拨中"),
        (SessionStatus::AuthFail, Primary::AuthFail, "认证失败"),
    ] {
        let mut s = snap();
        s.status = st;
        let v = view(Some(&s));
        assert_eq!((v.primary, v.light), (primary, Light::Busy));
        assert_eq!(v.primary.word_zh(), word);
        assert_eq!(v.primary.egress_zh(), "—");
    }
    let mut s = snap();
    s.status = SessionStatus::Idle;
    let v = view(Some(&s));
    assert_eq!((v.primary, v.light), (Primary::Idle, Light::Off));
}

#[test]
fn status_line_composes_wired_and_wifi() {
    let mut s = snap();
    assert_eq!(status_line_zh(Some(&s)), "有线：已连接 · WiFi：关闭");
    s.wireless.phase = WPhase::Online;
    assert_eq!(status_line_zh(Some(&s)), "有线：已连接 · WiFi：已接管");
}

#[test]
fn session_en_strings_are_frozen() {
    assert_eq!(session_en(SessionStatus::Idle), "Idle");
    assert_eq!(session_en(SessionStatus::Dialing), "Dialing");
    assert_eq!(session_en(SessionStatus::Connected), "Connected");
    assert_eq!(session_en(SessionStatus::Backoff), "Backoff (retrying)");
    assert_eq!(session_en(SessionStatus::AuthFail), "Auth failed");
}

#[test]
fn wireless_and_heartbeat_en_strings() {
    use gdut_net::status::{heartbeat_en, mode_en, wireless_en};
    let mut w = WirelessSnapshot::default();
    assert_eq!(wireless_en(&w), "Off");
    w.phase = WPhase::Online;
    w.ip = Some("10.0.3.7".into());
    assert_eq!(wireless_en(&w), "Online 10.0.3.7");
    w.ip = None;
    w.last_error = Some("portal timeout".into());
    assert_eq!(wireless_en(&w), "Error (portal timeout)");
    assert_eq!(heartbeat_en(&HeartbeatStatus::Off), "Off");
    assert_eq!(
        heartbeat_en(&HeartbeatStatus::Error("seed 校验失败".into())),
        "Error (seed 校验失败)"
    );
    assert_eq!(mode_en(NetMode::WiredExclusive), "Wired only (auto wireless takeover)");
    assert_eq!(mode_en(NetMode::WiredPlusStandby), "Wired + wireless standby");
    assert_eq!(gdut_net::status::wphase_zh(WPhase::Joining), "连接中");
}
```

- [ ] **Step 2: 跑测试确认失败** — `cargo test --test status`，预期 `unresolved import gdut_net::status`。

- [ ] **Step 3: 实现 `src/status.rs`**（全文）：

```rust
//! 快照 → 人类可读状态的唯一转换器（托盘 / 日常界面 / CLI / setup 共用）。
//!
//! 本模块拥有：读卡灯语义（Light）与主状态（Primary）的优先级判定、
//! 中文词表（托盘状态行 / 日常窗口 / setup 等待文案）、
//! 英文词表（CLI `status` 输出与模式确认）。
//! 任何状态变体只在此处新增；渲染端（egui / tray-icon / stdout）只做适配。

use crate::ipc::protocol::{
    HeartbeatStatus, NetMode, SessionStatus, StateSnapshot, WPhase, WirelessSnapshot,
};

/// 读卡灯语义：托盘图标与界面状态灯的共用判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Light {
    Wired,
    Wireless,
    Busy,
    Off,
}

/// 页级主状态（日常界面大字 / 托盘状态行的第一语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Primary {
    ServiceDown,
    Connected,
    WirelessOnline,
    Dialing,
    Backoff,
    AuthFail,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusView {
    pub primary: Primary,
    pub light: Light,
}

/// 优先级：有线已连接 > 无线已接管 > 有线进行中/失败/空闲；无快照 = 服务未运行。
pub fn view(s: Option<&StateSnapshot>) -> StatusView {
    let Some(s) = s else {
        return StatusView {
            primary: Primary::ServiceDown,
            light: Light::Off,
        };
    };
    if s.status == SessionStatus::Connected {
        return StatusView {
            primary: Primary::Connected,
            light: Light::Wired,
        };
    }
    if s.wireless.phase == WPhase::Online {
        return StatusView {
            primary: Primary::WirelessOnline,
            light: Light::Wireless,
        };
    }
    let primary = match s.status {
        SessionStatus::Dialing => Primary::Dialing,
        SessionStatus::Backoff => Primary::Backoff,
        SessionStatus::AuthFail => Primary::AuthFail,
        SessionStatus::Idle => Primary::Idle,
        SessionStatus::Connected => unreachable!("connected handled above"),
    };
    let light = match primary {
        Primary::Idle => Light::Off,
        _ => Light::Busy,
    };
    StatusView { primary, light }
}

impl Primary {
    /// 中文短语（GUI 侧）。
    pub fn word_zh(self) -> &'static str {
        match self {
            Primary::ServiceDown => "服务未运行",
            Primary::Connected => "已连接",
            Primary::WirelessOnline => "无线接管",
            Primary::Dialing => "拨号中",
            Primary::Backoff => "重拨中",
            Primary::AuthFail => "认证失败",
            Primary::Idle => "空闲",
        }
    }

    /// 出口说明（小票字段）。
    pub fn egress_zh(self) -> &'static str {
        match self {
            Primary::Connected => "有线",
            Primary::WirelessOnline => "无线",
            _ => "—",
        }
    }
}

/// 会话状态中文（托盘状态行 / setup 等待文案）。
pub fn session_zh(s: SessionStatus) -> &'static str {
    match s {
        SessionStatus::Connected => "已连接",
        SessionStatus::Dialing => "拨号中",
        SessionStatus::Backoff => "重拨中",
        SessionStatus::AuthFail => "认证失败",
        SessionStatus::Idle => "空闲",
    }
}

/// 会话状态英文（CLI `status`；字符串逐字冻结，见 tests/status.rs）。
pub fn session_en(s: SessionStatus) -> &'static str {
    match s {
        SessionStatus::Idle => "Idle",
        SessionStatus::Dialing => "Dialing",
        SessionStatus::Connected => "Connected",
        SessionStatus::Backoff => "Backoff (retrying)",
        SessionStatus::AuthFail => "Auth failed",
    }
}

/// 无线相位中文。
pub fn wphase_zh(phase: WPhase) -> &'static str {
    match phase {
        WPhase::Off => "关闭",
        WPhase::Joining => "连接中",
        WPhase::Authing => "认证中",
        WPhase::Online => "已接管",
        WPhase::Error => "错误",
    }
}

/// 无线相位英文（CLI `status`；逐字冻结）。
pub fn wphase_en(phase: WPhase) -> &'static str {
    match phase {
        WPhase::Off => "Off",
        WPhase::Joining => "Joining",
        WPhase::Authing => "Authenticating",
        WPhase::Online => "Online",
        WPhase::Error => "Error",
    }
}

/// 心跳英文（CLI `status`；逐字冻结）。
pub fn heartbeat_en(h: &HeartbeatStatus) -> String {
    match h {
        HeartbeatStatus::Off => "Off".to_string(),
        HeartbeatStatus::Running => "Running".to_string(),
        HeartbeatStatus::Error(e) => format!("Error ({e})"),
    }
}

/// 模式英文（CLI `status` 与模式确认；逐字冻结）。
pub fn mode_en(m: NetMode) -> &'static str {
    match m {
        NetMode::WiredExclusive => "Wired only (auto wireless takeover)",
        NetMode::WiredPlusStandby => "Wired + wireless standby",
    }
}

/// 无线链路英文（相位 + IP / 错误后缀；逐字冻结）。
pub fn wireless_en(w: &WirelessSnapshot) -> String {
    let phase = wphase_en(w.phase);
    match (&w.ip, &w.last_error) {
        (Some(ip), _) => format!("{phase} {ip}"),
        (None, Some(e)) => format!("{phase} ({e})"),
        _ => phase.to_string(),
    }
}

/// 托盘状态行（菜单首项 + tooltip 共用）：None = 无快照按"服务未运行"。
pub fn status_line_zh(s: Option<&StateSnapshot>) -> String {
    let Some(s) = s else {
        return "服务未运行".to_string();
    };
    format!(
        "有线：{} · WiFi：{}",
        session_zh(s.status),
        wphase_zh(s.wireless.phase)
    )
}
```

- [ ] **Step 4: 跑测试确认通过** — `cargo test --test status`。

- [ ] **Step 5: 迁移调用点（行为冻结）**
  - `src/ipc/protocol.rs`：删除 `impl StateSnapshot` 中的 `status_text/heartbeat_text/mode_text/wireless_text`，仅保留 `uptime_text`；清理因此未用的 `use`（`WirelessSnapshot` 仍被 struct 用）。
  - `src/ipc/client.rs:95-105`：
    `s.status_text()` → `crate::status::session_en(s.status)`；
    `s.heartbeat_text()` → `crate::status::heartbeat_en(&s.heartbeat)`；
    `s.mode_text()` → `crate::status::mode_en(s.mode)`；
    `s.wireless_text()` → `crate::status::wireless_en(&s.wireless)`。
  - `src/cli.rs:156`：`s.mode_text()` → `crate::status::mode_en(s.mode)`。
  - `src/setup/work.rs:439`：`s.status_text()` → `crate::status::session_en(s.status)`。
  - `tests/ipc_protocol.rs`：删除 `snapshot_texts`（其英文断言已迁入 `tests/status.rs`）；保留 `format_uptime_segments`。

- [ ] **Step 6: 迁移托盘与日常界面**
  - `src/tray/mod.rs`：`icon_kind` 改为 `match crate::status::view(s).light { Light::Wired => IconKind::WiredUp, Light::Wireless => IconKind::WirelessUp, Light::Busy => IconKind::Backoff, Light::Off => IconKind::Down }`；`status_line` 改为 `crate::status::status_line_zh(s)`；删除文件中重复的中文词表与不再使用的 `use`。
  - `src/tray/gui.rs`：
    - `status_view` 保留颜色映射，但 `word/egress` 从 `crate::status::view(s)` 派生：
      `ServiceDown → (VERMILION, PAPER_WHITE, VERMILION, None)`；`Connected / WirelessOnline → (READER_GREEN, INK_BLACK, READER_GREEN, Some(READER_GREEN))`；`Dialing / Backoff → (INK_BLACK, PAPER_WHITE, INK_BLACK, Some(PAPER_WHITE))`；`AuthFail → (VERMILION, PAPER_WHITE, VERMILION, Some(VERMILION))`；`Idle → (PAPER_WHITE, INK_BLACK, INK_BLACK, None)`。
      `word = primary.word_zh()`、`egress = primary.egress_zh()`；本地 `match s.status` 词表删除。
    - 无线相位词表（gui.rs 约 665-671）改用 `crate::status::wphase_zh(phase)`。
    - **视觉冻结**：既有 `#[cfg(test)] mod tests`（gui.rs 864+）必须逐个断言不改仍通过。

- [ ] **Step 7: 全量 gates**（Linux 真跑；Windows 交叉）：

```bash
cargo test && cargo clippy -- -D warnings && cargo fmt --check
cargo check --target x86_64-pc-windows-msvc
cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings
```

- [ ] **Step 8: Commit** — `refactor(status): single status-view module for tray, GUI and CLI`

---

### Task 2: 盘上布局与日志策略单一来源（paths + logging）

**Files:**
- Create: `src/paths.rs`
- Modify: `src/lib.rs`（`pub mod paths;`）
- Modify: `src/logging.rs`（策略常量 + `init_tray_logging` 消费）
- Modify: `src/config.rs:88,111`、`src/cli.rs:17`、`src/service.rs:470,691`、`src/tray/mod.rs:374`、`src/tray/gui.rs:20-21`、`src/setup/mod.rs:17-30`、`src/setup/ui.rs:34`
- Test: `tests/paths.rs`

**Interfaces:**
- Consumes: `crate::logging::{LOG_MAX_SIZE_MB, LOG_KEEP_FILES}`（本任务新增）。
- Produces:

```rust
pub const DATA_DIR: &str = r"C:\ProgramData\gdut-net";
pub const CONFIG_PATH: &str = r"C:\ProgramData\gdut-net\config.toml";
pub const PBK_PATH: &str = r"C:\ProgramData\gdut-net\gdut.pbk";
pub const LOGS_DIR: &str = r"C:\ProgramData\gdut-net\logs";
pub const INSTALL_DIR: &str = r"C:\Program Files\gdut-net";
pub fn install_dir() -> std::path::PathBuf;   // %ProgramFiles%\gdut-net（环境变量优先）
pub fn install_exe() -> std::path::PathBuf;   // install_dir()/gdut-net.exe
pub fn setup_exe() -> std::path::PathBuf;     // install_dir()/gdut-net-setup.exe
```

```rust
// logging.rs
pub const LOG_MAX_SIZE_MB: u64 = 5;
pub const LOG_KEEP_FILES: usize = 5;
```

- [ ] **Step 1: 写失败测试** — `tests/paths.rs`：

```rust
use gdut_net::config::LogCfg;
use gdut_net::logging::{LOG_KEEP_FILES, LOG_MAX_SIZE_MB};
use gdut_net::paths;

#[test]
fn layout_constants_are_consistent() {
    assert_eq!(format!("{}\\config.toml", paths::DATA_DIR), paths::CONFIG_PATH);
    assert_eq!(format!("{}\\logs", paths::DATA_DIR), paths::LOGS_DIR);
    assert_eq!(format!("{}\\gdut.pbk", paths::DATA_DIR), paths::PBK_PATH);
    assert!(paths::install_dir().ends_with("gdut-net"));
    assert!(paths::install_exe().ends_with("gdut-net.exe"));
    assert!(paths::setup_exe().ends_with("gdut-net-setup.exe"));
}

#[test]
fn log_policy_has_one_source() {
    let cfg = LogCfg::default();
    assert_eq!(cfg.max_size_mb, LOG_MAX_SIZE_MB);
    assert_eq!(cfg.rotate_keep as usize, LOG_KEEP_FILES);
    assert_eq!(cfg.dir, paths::LOGS_DIR);
}
```

- [ ] **Step 2: 跑测试确认失败** — `cargo test --test paths`，预期 unresolved import。

- [ ] **Step 3: 实现 `src/paths.rs`**：

```rust
//! 盘上布局的唯一来源（安装态；开发态由 setup 的 payload 回退单独处理）。
//!
//! `--config` 显式覆盖仍以参数为准；这里只定义缺省布局。

use std::path::PathBuf;

/// ProgramData 根。
pub const DATA_DIR: &str = r"C:\ProgramData\gdut-net";
/// 缺省配置路径（CLI `--config` 默认值与服务无参回退共用）。
pub const CONFIG_PATH: &str = r"C:\ProgramData\gdut-net\config.toml";
/// 拨号电话簿路径（config.rs 的 `dial.pbk_path` 缺省）。
pub const PBK_PATH: &str = r"C:\ProgramData\gdut-net\gdut.pbk";
/// 日志目录（服务、托盘、安装器共用）。
pub const LOGS_DIR: &str = r"C:\ProgramData\gdut-net\logs";
/// 安装目录（发布形态；环境变量缺省值）。
pub const INSTALL_DIR: &str = r"C:\Program Files\gdut-net";

/// 安装目录：`%ProgramFiles%\gdut-net`（环境变量缺失时回退缺省值）。
pub fn install_dir() -> PathBuf {
    std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"))
        .join("gdut-net")
}

/// 安装后的主程序。
pub fn install_exe() -> PathBuf {
    install_dir().join("gdut-net.exe")
}

/// 安装后的安装器。
pub fn setup_exe() -> PathBuf {
    install_dir().join("gdut-net-setup.exe")
}
```

- [ ] **Step 4: 实现日志策略常量** — `src/logging.rs` 顶部加：

```rust
/// 文件日志策略（GUI 进程与服务一致；AGENTS/CONTEXT 文档口径）。
pub const LOG_MAX_SIZE_MB: u64 = 5;
pub const LOG_KEEP_FILES: usize = 5;
```

`init_tray_logging` 内 `Criterion::Size(5 * 1024 * 1024)` → `Criterion::Size(rotation_bytes(LOG_MAX_SIZE_MB))`、
`Cleanup::KeepLogFiles(2)` → `Cleanup::KeepLogFiles(LOG_KEEP_FILES)`（D5：GUI 与服务同策略）。
`src/config.rs` 的 `LogCfg::default()` 改用 `crate::logging::{LOG_MAX_SIZE_MB, LOG_KEEP_FILES}` 与 `crate::paths::LOGS_DIR`。

- [ ] **Step 5: 替换全部字面量调用点**
  - `src/config.rs:88` → `crate::paths::PBK_PATH.into()`；`:111` → `crate::paths::LOGS_DIR.into()`。
  - `src/cli.rs:17` → `default_value = crate::paths::CONFIG_PATH`（若 clap derive 拒绝非常量表达式，退回 `default_value_t` 方案前先验证；预期 `default_value = CONST` 合法）。
  - `src/service.rs:470` → `PathBuf::from(crate::paths::CONFIG_PATH)`；`:691`（`program_data_dir` 回退）→ `PathBuf::from(crate::paths::DATA_DIR)`。
  - `src/tray/mod.rs:374` → `crate::logging::init_tray_logging(crate::paths::LOGS_DIR, "tray")`。
  - `src/tray/gui.rs:20-21` → 删除本地 `CFG_PATH/LOG_DIR`，读写处改用 `crate::paths::{CONFIG_PATH, LOGS_DIR}`。
  - `src/setup/mod.rs:17-30` → 删除本地 `DATA_DIR`，`config_path()` 改 `PathBuf::from(crate::paths::CONFIG_PATH)`，`install_dir()` 委托 `crate::paths::install_dir()`。
  - `src/setup/ui.rs:34` → `crate::paths::LOGS_DIR`。
  - 复核：`grep -rnF 'ProgramData\gdut-net' src` 除 `paths.rs` 外为空（`shell.rs` 的 `%ProgramData%` 环境变量是开始菜单路径，不属本布局）。

- [ ] **Step 6: 全量 gates**（同 Task 1 Step 7）。

- [ ] **Step 7: Commit** — `refactor(paths): single source for layout and log policy`

---

### Task 3: 死接口清理

**Files:**
- Modify: `src/ipc/protocol.rs:135-140`（`ServerMsg::Ack`）、`src/ipc/client.rs:54-63`
- Modify: `src/service.rs:56-61,211`（`InstallOutcome.student_id`）
- Modify: `src/eventlog.rs:169-178`（`ensure_dir` 定义与导出）
- Test: 不新增；既有测试 + clippy 即验证

**Interfaces:**
- Consumes: 无。
- Produces: `ServerMsg` 仅剩 `State`；`InstallOutcome` 仅剩 `cfg_path, service_refreshed`（字段名以现状为准，删 `student_id`）；`eventlog` 不再导出 `ensure_dir`。

- [ ] **Step 1: 删除 `ServerMsg::Ack`** — `protocol.rs` 中 `pub enum ServerMsg { State {...}, Ack }` 删 `Ack`；`client.rs` 的 `Ok(ServerMsg::Ack) => continue,` 删除并同步注释（"Ack 或非法帧"→"非法帧"）。若 `serde` 枚举编译报警，检查 `tests/ipc_protocol.rs` 是否有 Ack 往返测试（已确认无）。

- [ ] **Step 2: 删除 `InstallOutcome.student_id`** — 删字段定义（service.rs:57）与赋值（:211）；`grep -rn "\.student_id" src/service.rs` 应只剩 `cfg.account.student_id` 的正常用法；`install()` 中局部 `student_id`（:111-121）保留（用于提示与写配置）。

- [ ] **Step 3: 删除 `eventlog::ensure_dir`** — 删函数与 `pub use` 列表中的名字；`grep -rn "ensure_dir" src` 为空。

- [ ] **Step 4: 全量 gates**（同 Task 1 Step 7）。clippy `-D warnings` 会兜住任何残留引用。

- [ ] **Step 5: Commit** — `refactor: drop dead protocol/service/eventlog interfaces`

---

### Task 4: Win32 原语（wide + reg 写入）

**Files:**
- Create: `src/win32.rs`
- Modify: `src/lib.rs`（`pub mod win32;`）
- Modify: `src/tray/mod.rs`（AUMID `register_aumid`、autostart `register_autostart`、模块级 `wide`）
- Modify: `src/shell.rs`（`write_uninstall_key` 的 sz/dword 写入、`wide`）
- Modify: `src/eventlog.rs`（`wide`）、`src/crypto.rs`（`wide`）、`src/setup/mod.rs`（`wide`）
- Test: Create `tests/win32.rs`

**Interfaces:**
- Consumes: windows 0.62 registry API。
- Produces:

```rust
/// str → UTF-16 + NUL（Win32 宽字符参数）。
pub fn wide(s: &str) -> Vec<u16>;

#[cfg(windows)]
pub mod reg {
    use windows::Win32::Foundation::HKEY;
    /// 建/开键（HKLM/HKCU 均可）后写一个 REG_SZ；name=None 写默认值。
    pub fn set_string(root: HKEY, subkey: &str, name: Option<&str>, value: &str) -> anyhow::Result<()>;
    /// 写一个 REG_DWORD。
    pub fn set_dword(root: HKEY, subkey: &str, name: &str, value: u32) -> anyhow::Result<()>;
}
```

- [ ] **Step 1: 写失败测试** — `tests/win32.rs`：

```rust
use gdut_net::win32::wide;

#[test]
fn wide_appends_nul() {
    assert_eq!(wide(""), vec![0]);
    assert_eq!(wide("A"), vec![0x41, 0]);
    assert_eq!(wide("中"), vec![0x4E2D, 0]);
    assert_eq!(wide("ok").last(), Some(&0));
}
```

- [ ] **Step 2: 跑测试确认失败** — `cargo test --test win32`。

- [ ] **Step 3: 实现 `src/win32.rs`**（全文）：

```rust
//! Win32 原语：宽字符串与注册表写入的唯一实现。
//!
//! `wide` 是纯函数（Linux 可测）；注册表部分仅 Windows，错误信息带值名与
//! 返回码，便于真机排障。

/// str → UTF-16 + NUL（Win32 宽字符参数）。
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
pub mod reg {
    use anyhow::{bail, Result};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{ERROR_SUCCESS, HKEY};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE,
        REG_SZ,
    };

    use super::wide;

    fn create(root: HKEY, subkey: &str) -> Result<HKEY> {
        let subkey_w = wide(subkey);
        let mut hkey = HKEY::default();
        let ret = unsafe {
            RegCreateKeyExW(
                root,
                PCWSTR(subkey_w.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut hkey,
                None,
            )
        };
        if ret != ERROR_SUCCESS {
            bail!("RegCreateKeyExW({subkey}) failed: {}", ret.0);
        }
        Ok(hkey)
    }

    fn set_value(
        root: HKEY,
        subkey: &str,
        name: Option<&str>,
        value_type: windows::Win32::System::Registry::REG_VALUE_TYPE,
        bytes: &[u8],
    ) -> Result<()> {
        let hkey = create(root, subkey)?;
        let name_w = name.map(wide);
        let ret = unsafe {
            RegSetValueExW(
                hkey,
                name_w
                    .as_ref()
                    .map_or(PCWSTR::null(), |w| PCWSTR(w.as_ptr())),
                None,
                value_type,
                Some(bytes),
            )
        };
        let _ = unsafe { RegCloseKey(hkey) };
        if ret != ERROR_SUCCESS {
            bail!(
                "RegSetValueExW({}\\{}) failed: {}",
                subkey,
                name.unwrap_or("(default)"),
                ret.0
            );
        }
        Ok(())
    }

    /// 建/开键后写一个 REG_SZ；`name=None` 写默认值。
    pub fn set_string(root: HKEY, subkey: &str, name: Option<&str>, value: &str) -> Result<()> {
        let bytes: Vec<u8> = wide(value).iter().flat_map(|c| c.to_le_bytes()).collect();
        set_value(root, subkey, name, REG_SZ, &bytes)
    }

    /// 写一个 REG_DWORD。
    pub fn set_dword(root: HKEY, subkey: &str, name: &str, value: u32) -> Result<()> {
        set_value(root, subkey, Some(name), REG_DWORD, &value.to_le_bytes())
    }
}
```

- [ ] **Step 4: 跑测试确认通过** — `cargo test --test win32`。

- [ ] **Step 5: 替换 `wide` 调用点** — `grep -rn "fn wide\|let wide" src` 后逐一替换为 `crate::win32::wide` 并删除本地定义/闭包：
  `src/tray/mod.rs`（模块级 fn 与 `register_aumid` 内闭包）、`src/eventlog.rs`、`src/setup/mod.rs`、`src/shell.rs`、`src/crypto.rs`。
  **不替换** `src/ras.rs` 的 `wide(s, len)`（定长截断语义不同）与 `src/ipc/server.rs` 的内联 SDDL 转换（单点）。

- [ ] **Step 6: 替换注册表写入**
  - `src/tray/mod.rs::register_aumid` → `crate::win32::reg::set_string(HKEY_CURRENT_USER, r"Software\Classes\AppUserModelId\gdut-net", None, "GDUT Net")`（保留函数注释与失败不阻断语义）。
  - `src/tray/mod.rs::register_autostart` → `crate::win32::reg::set_string(HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Run", Some("gdut-net-tray"), &value)`；`unregister_autostart` 的 OpenKey+DeleteValue 保留不动。
  - `src/shell.rs::write_uninstall_key` 的 `set_sz` 闭包 → `reg::set_string(HKEY_LOCAL_MACHINE, UNINSTALL_SUBKEY, Some(name), value)`；DWORD 循环 → `reg::set_dword(...)`，删掉本地闭包与重复的 `wide`/`RegSetValueExW` 样板（`RegCreateKeyExW`/`RegCloseKey` 归纳进 `reg::create/set_value` 后，`write_uninstall_key` 里不再直接出现）。

- [ ] **Step 7: 全量 gates**（同 Task 1 Step 7）；Windows 交叉 clippy 必须干净。

- [ ] **Step 8: Commit** — `refactor(win32): single wide() and registry write helpers`

---

### Task 5: HTTP 原语与 portal/probe 收敛

**Files:**
- Create: `src/http.rs`
- Modify: `src/lib.rs`（`pub mod http;`）
- Modify: `src/probe.rs`（`parse_http_probe_target` 复用 `http::parse_url`；删 win::`parse_status_code`/`is_auth_redirect`；`http_get_probe` 改用 `http::get`；`verdict_from_http` 调用点用 `crate::http::is_auth_redirect`）
- Modify: `src/wireless/portal.rs`（`portal_get` 改用 `http::get_async`；删本地 URL 拆分与 socket 代码）
- Test: Create `tests/http.rs`

**Interfaces:**
- Consumes: socket2（已有依赖）。
- Produces:

```rust
// cfg-free
pub struct Target { pub host: String, pub port: u16, pub path: String, pub host_header: String }
pub fn parse_url(url: &str) -> Option<Target>;
pub fn parse_status_code(status_line: &str) -> Option<u16>;
pub fn is_auth_redirect(location_lower: &str) -> bool;

// #[cfg(windows)]
pub struct Request<'a> { pub url: &'a str, pub bind_ip: Ipv4Addr, pub user_agent: &'a str,
                         pub timeout: Duration, pub max_bytes: u64 }
pub struct Response { pub status: u16, pub location_lower: String, pub body: String }
pub fn get(req: &Request<'_>) -> anyhow::Result<Response>;
pub async fn get_async(url: String, bind_ip: Ipv4Addr, user_agent: &'static str,
                       timeout: Duration, max_bytes: u64) -> anyhow::Result<Response>;
```

- [ ] **Step 1: 写失败测试** — `tests/http.rs`：

```rust
use gdut_net::http::{is_auth_redirect, parse_status_code, parse_url};

#[test]
fn parse_url_splits_host_port_path() {
    let t = parse_url("http://10.0.3.2:801/eportal/portal/login").unwrap();
    assert_eq!(t.host, "10.0.3.2");
    assert_eq!(t.port, 801);
    assert_eq!(t.path, "/eportal/portal/login");
    assert_eq!(t.host_header, "10.0.3.2:801");

    let t = parse_url("http://223.5.5.5").unwrap();
    assert_eq!((t.host.as_str(), t.port, t.path.as_str(), t.host_header.as_str()),
               ("223.5.5.5", 80, "/", "223.5.5.5"));

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
```

- [ ] **Step 2: 跑测试确认失败** — `cargo test --test http`。

- [ ] **Step 3: 实现 `src/http.rs`** — 纯解析部分：

```rust
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
```

Windows 部分（同文件追加，接口见上）：

```rust
#[cfg(windows)]
mod win {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    use std::time::Duration;

    use anyhow::{anyhow, Context, Result};
    use socket2::{Domain, Protocol, Socket, Type};

    use super::{parse_status_code, parse_url};

    pub struct Request<'a> {
        pub url: &'a str,
        pub bind_ip: Ipv4Addr,
        pub user_agent: &'a str,
        pub timeout: Duration,
        pub max_bytes: u64,
    }

    #[derive(Debug, Clone)]
    pub struct Response {
        pub status: u16,
        pub location_lower: String,
        pub body: String,
    }

    /// 同步 GET：socket2 绑源 IP、connect/read/write 用同一 timeout、读上限 max_bytes。
    pub fn get(req: &Request<'_>) -> Result<Response> {
        let target =
            parse_url(req.url).ok_or_else(|| anyhow!("Unsupported URL (http only)"))?;
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
        stream.take(req.max_bytes).read_to_end(&mut buf)?;
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
    ) -> Result<Response> {
        tokio::task::spawn_blocking(move || {
            get(&Request {
                url: &url,
                bind_ip,
                user_agent,
                timeout,
                max_bytes,
            })
        })
        .await
        .map_err(|e| anyhow!("http task panicked: {e}"))?
    }
}

#[cfg(windows)]
pub use win::{get, get_async, Request, Response};
```

- [ ] **Step 4: probe 收敛**
  - `parse_http_probe_target` 改为：

    ```rust
    pub fn parse_http_probe_target(url: &str) -> Option<(Ipv4Addr, String)> {
        let t = crate::http::parse_url(url)?;
        let ip = t.host.parse::<Ipv4Addr>().ok()?;
        Some((ip, format!("{}:{}", t.host, t.port)))
    }
    ```

    既有 `probe.rs` 单元测试（第 67-143 行）不改仍须全过。
  - 删除 `win::parse_status_code` 与 `win::is_auth_redirect`。
  - `win::http_get_probe` 改为 `crate::http::get` 的薄包装（保持 `Option<(u16, String)>` 与 debug 日志）：

    ```rust
    fn http_get_probe(src_ip: Ipv4Addr, http_url: &str) -> Option<(u16, String)> {
        let req = crate::http::Request {
            url: http_url,
            bind_ip: src_ip,
            user_agent: "gdut-net-probe",
            timeout: HTTP_TIMEOUT,
            max_bytes: HTTP_MAX_RESPONSE,
        };
        match crate::http::get(&req) {
            Ok(r) => Some((r.status, r.location_lower)),
            Err(e) => {
                log::debug!("Probe: HTTP request failed: {e:#}");
                None
            }
        }
    }
    ```
  - `verdict_from_http(status == 302, is_auth_redirect(&location))` 调用点 → `crate::http::is_auth_redirect(&location)`；清理未用 import（`Read/Write/TcpStream/socket2` 视剩余用途而定）。

- [ ] **Step 5: portal 收敛**
  - 删除 `win::parse_status` 与 `win::portal_get_blocking`；`portal_get` 改为：

    ```rust
    pub async fn portal_get(src_ip: Ipv4Addr, url: &str) -> Option<(u16, String)> {
        crate::http::get_async(
            url.to_string(),
            src_ip,
            PORTAL_UA,
            TIMEOUT,
            MAX_RESPONSE,
        )
        .await
        .ok()
        .map(|r| (r.status, r.body))
    }
    ```
  - `TIMEOUT`（8s）/`MAX_RESPONSE`（64K）/`PORTAL_UA`（`python-requests/2.31.0`）参数保持不变；`build_login_url`、`parse_portal_reply`、`redact_query` 不动。

- [ ] **Step 6: 全量 gates**（同 Task 1 Step 7）。

- [ ] **Step 7: Commit** — `refactor(http): single HTTP/1.0 primitive; probe and portal consume it`

---

## Self-Review

- **Spec coverage:** W1→Task 1；W2→Task 2；W3→Task 3；W4→Task 4；W5→Task 5。W6–W8 明确留给第二波计划（spec §执行）。
- **Placeholder scan:** 无 TBD/TODO；所有新 module 给出全文或完整步骤；机械替换点均带文件:行。
- **Type consistency:** `StatusView{primary,light}`、`Primary::word_zh/egress_zh`、`paths::*` 常量、`win32::wide/reg::*`、`http::{Target,Request,Response,parse_url,...}` 在任务间与调用点名称一致；`Light` 与 tray `IconKind` 的映射在 Task 1 内闭环。
