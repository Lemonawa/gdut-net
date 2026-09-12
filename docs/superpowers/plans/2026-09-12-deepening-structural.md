# Deepening（结构波）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 按 `2026-09-12-deepening-design.md` 的第二波，把 IPC 会话生命周期、安装生命周期、runtime 组合层三处深挖成单一 interface：调用者不再各记协议/回滚/调度，OS 效果留在 adapter，决策进 cfg-free（可 Linux TDD）module。

**Architecture:** W6 把"连接 + 握手 + 命令确认"收进 `ipc::session`；W7 让 `service` 拥有完整安装生命周期（含回滚）与三态真实状态，报告行共用一份枚举；W8 按 design-it-twice 选定方案把 runtime 的"事件 → 动作"决策收进 cfg-free module，Windows 侧只留效果 adapter。

**Tech Stack:** Rust 1.97, tokio 1.53（current-thread 同步外观）, windows-service 0.8, windows 0.62；不新增依赖。

**Spec:** `docs/superpowers/specs/2026-09-12-deepening-design.md`（W6/W7/W8 + D1–D7）。开工前读 `CONTEXT.md` Rules 与陷阱。

## Global Constraints

- 控制台输出（CLI/日志/脚本）**英文**；GUI 文案**中文**；源码注释中英文皆可。
- CLI 行为冻结：子命令、提示、退出码不变；`status` 输出文本逐字不变；silent 步骤行（`== step`/`ok step`）逐字不变。
- 纯逻辑（cfg-free）必须 Linux 可测；Win32/egui 胶水仅 `cfg(windows)`，靠交叉编译与评审验证。
- 每个任务收尾必须全绿：`cargo test && cargo clippy -- -D warnings && cargo fmt --check` 以及
  `cargo check --target x86_64-pc-windows-msvc && cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings`。
- 不新增依赖；不改 `Cargo.toml`。
- 无线接管 Runtime Rules（CONTEXT.md）在 W8 是绑定约束：拔线不拨号、去抖/让位、/32 路由与 metric 三出口、绝对唤醒节奏。
- 提交风格：`feat:/fix:/refactor:/test:/docs:`。

## File Map

- Task 6: `src/ipc/client.rs` → 重命名 `src/ipc/session.rs`；`src/ipc/mod.rs`、`src/cli.rs`、`src/setup/work.rs`、`src/tray/mod.rs`。
- Task 7: `src/service.rs`、`src/setup/work.rs`、`src/setup/silent.rs`、`src/setup/ui.rs`、`src/cli.rs`、`src/tray/mod.rs`。
- Task 8: 新建 `src/supervisor.rs` + `tests/supervisor.rs`；`src/lib.rs`、`src/watchdog.rs`（`view()`）。
- Task 9: 重写 `src/runtime.rs`；`src/watchdog.rs`（删 `snapshot()`）、`tests/watchdog.rs`。

## 决策记录（本波）

- W6 不做"可测试性缝"的空转：管道只有 Windows 一种传输，**不引入 port**（一个 adapter = 假设的缝）；验收 = 交叉 gates + 行为冻结评审。
- W7 的 `InstallState` 增加 `Unknown`；三个消费者各自显式映射（托盘提示、安装器维护页、回滚判定）。
- W7 回滚语义：`capture_prev_service` / `rollback_install` 提升到 `service`；`install_with_rollback(req, &prev)` 供 CLI 与 setup 共用；setup 的 core 之外的失败（解包/快捷方式/起服务）仍用 `rollback_install(prev)`。
- W7 英文回滚行收进 `rollback_line_en`（CLI 与 silent 共用）；silent 输出逐字不变。

---

### Task 6: IPC 会话 module（session）

**Files:**
- Delete: `src/ipc/client.rs`
- Create: `src/ipc/session.rs`
- Modify: `src/ipc/mod.rs`（导出改名）
- Modify: `src/cli.rs`（`status` 与 `wireless off/standby`）
- Modify: `src/setup/work.rs`（`query_status_once`）
- Modify: `src/tray/mod.rs`（`ipc_loop`、`send_cmd_logged`）

**Interfaces:**
- Consumes: `crate::ipc::protocol::{Command, FrameDecoder, ServerMsg, StateSnapshot}`；tokio named pipe。
- Produces:

```rust
/// 与服务管道的一条连接：重试策略、握手（连接即收首帧快照）与命令确认
/// 都是本 module 的 implementation。
pub struct Session { /* pipe, decoder, buffered frames */ }

impl Session {
    /// async 连接（须在 tokio 上下文；重试 20×250ms，用 tokio sleep）。
    pub async fn connect() -> anyhow::Result<Session>;
    /// 读下一帧快照（跳过非法帧）。
    pub async fn next_snapshot(&mut self) -> anyhow::Result<StateSnapshot>;
    /// 发送命令（不等回显）。
    pub async fn send(&mut self, cmd: Command) -> anyhow::Result<()>;
    /// 发送命令并读回确认快照：读帧直到谓词为真或读满 `max_frames` 帧。
    pub async fn send_and_confirm<F>(
        &mut self,
        cmd: Command,
        confirm: F,
        max_frames: usize,
    ) -> anyhow::Result<StateSnapshot>
    where
        F: Fn(&StateSnapshot) -> bool;
}

/// 同步外观（CLI/安装器无线程 runtime）：内部自建 current_thread runtime。
pub struct SyncSession { /* rt, inner */ }

impl SyncSession {
    pub fn connect() -> anyhow::Result<SyncSession>;
    pub fn next_snapshot(&mut self) -> anyhow::Result<StateSnapshot>;
    pub fn send(&mut self, cmd: Command) -> anyhow::Result<()>;
    pub fn send_and_confirm<F>(
        &mut self,
        cmd: Command,
        confirm: F,
        max_frames: usize,
    ) -> anyhow::Result<StateSnapshot>
    where
        F: Fn(&StateSnapshot) -> bool;
}

/// `status`：连一次读一帧（不再自建 runtime；打印留在 CLI）。
pub fn status_snapshot() -> anyhow::Result<StateSnapshot>;

/// `wireless off/standby`：发 SetMode 并等回显（消费首帧 → 发 → ≤5 帧），
/// 确认失败返回与现状相同的错误文本。
pub fn set_mode_confirmed(mode: NetMode) -> anyhow::Result<StateSnapshot>;
```

- [ ] **Step 1: 迁移实现** — 把 `src/ipc/client.rs` 的 `PipeClient` 重写为 `src/ipc/session.rs`：
  - `connect` 的重试常量与逻辑保留（`CONNECT_RETRIES=20`、`250ms`），`std::thread::sleep` 改为 `tokio::time::sleep`（hazard 注释删除）。
  - `next_state` → `next_snapshot`（Ack 已删，非法帧照旧 debug 跳过）。
  - `send_cmd` → `send`。
  - 新增 `send_and_confirm`（谓词 + 帧数上限）。
  - 删除 `status_once`（打印移去 CLI）；新增 `status_snapshot`。
  - `SyncSession`：`tokio::runtime::Builder::new_current_thread().enable_all()`，方法 `self.rt.block_on(self.inner.xxx())`。
  - `set_mode_confirmed`：`SyncSession::connect()` → `next_snapshot()`（消费首帧）→ `send_and_confirm(Command::SetMode{mode}, |s| s.mode==mode, 5)` → 失败 `bail!("Service did not confirm mode switch (check `status` output)")`。

- [ ] **Step 2: 更新 `src/ipc/mod.rs`** — `pub mod client;` → `pub mod session;`（cfg 条件保持原样）。

- [ ] **Step 3: 更新 `src/cli.rs`**
  - `Cmd::Status`：`crate::ipc::session::status_snapshot()` → 新 `fn print_status(s: &StateSnapshot)`（把原 `client.rs::status_once` 的 `println!` 行逐字搬来：`Status:/Uptime:/IP:/Drop reason:/Redial attempts:/Heartbeat:/Mode:/Wireless:/Events:`，文本不变）。
  - `wireless_set_mode`：调用 `crate::ipc::session::set_mode_confirmed(mode)`，匹配处 `println!("Mode set: {}", crate::status::mode_en(s.mode))`（文本不变）；删除函数内自建 runtime。

- [ ] **Step 4: 更新 `src/setup/work.rs::query_status_once`** — 改为 `crate::ipc::session::status_snapshot()`（其余逻辑不动）。

- [ ] **Step 5: 更新 `src/tray/mod.rs`**
  - `ipc_loop`：`PipeClient::connect()` → `Session::connect()`、`client.next_state()` → `client.next_snapshot()`；重连等待仍用 `std::thread::sleep(CONNECT_RETRY)`（托盘线程语义不变）。
  - `send_cmd_logged`：改为 `SyncSession::connect()` + `send()`（删除本地 runtime 构建）。

- [ ] **Step 6: 全量 gates**（见 Global Constraints）。

- [ ] **Step 7: Commit** — `refactor(ipc): session module owns connect, handshake and command confirmation`

---

### Task 7: 安装生命周期（install/uninstall core）

**Files:**
- Modify: `src/service.rs`（三态 `InstallState`、`PrevService`/`capture_prev_service`/`rollback_install`/`install_with_rollback`/`rollback_line_en`、`UninstallReport::rows`、四个 `STEP_UNINSTALL_*` 常量）
- Modify: `src/setup/work.rs`（删除本地 `PrevService`/`rollback_for`，改用 service 的提升版；`run_uninstall` 用 `report.rows()`）
- Modify: `src/setup/silent.rs`（卸载行改用 `report.rows()` + `rollback_line_en`）
- Modify: `src/setup/ui.rs`（`InstallState::Unknown` 分支）
- Modify: `src/cli.rs`（`install` 走 `install_with_rollback` + `rollback_line_en`；`uninstall` 输出不变）
- Modify: `src/tray/mod.rs`（`double_click_entry` 显式处理 `Unknown`）

**Interfaces:**
- Consumes: 现有 `InstallRequest`/`InstallOutcome`/`UninstallReport`/`Step`；`windows_service` SCM 错误码。
- Produces:

```rust
/// 三态：查询失败不再冒充"未安装"。
pub enum InstallState {
    Installed { service_exe: Option<PathBuf>, version: Option<String> },
    NotInstalled,
    /// SCM 不可达或查询失败（非"不存在"）。
    Unknown,
}

/// 安装前服务状态（回滚依据）。
pub enum PrevService { None, Known(PathBuf), Unknown }
pub fn capture_prev_service() -> PrevService;

/// 失败回滚：恢复旧注册（或删除新建），回报回滚实情。
pub enum RollbackOutcome { NotNeeded, Restored, RestoredUnknown, Failed(String) }
pub fn rollback_install(prev: &PrevService) -> RollbackOutcome;

/// 安装 + core 失败自动回滚（CLI 与 setup 共用）。
pub struct InstallFailure { pub error: anyhow::Error, pub rollback: RollbackOutcome }
pub fn install_with_rollback(
    req: InstallRequest,
    prev: &PrevService,
) -> Result<InstallOutcome, InstallFailure>;

/// 控制台英文回滚行（CLI 与 silent 共用；None = 无需打印）。
pub fn rollback_line_en(outcome: &RollbackOutcome) -> Option<String>;

/// 四个卸载步骤键（GUI/silent/报告行共用；`setup::work` 重导出）。
pub const STEP_UNINSTALL_SERVICE: &str = "uninstall_service";
pub const STEP_UNINSTALL_EVENT_SOURCE: &str = "uninstall_event_source";
pub const STEP_UNINSTALL_ENTROPY: &str = "uninstall_entropy";
pub const STEP_UNINSTALL_AUTOSTART: &str = "uninstall_autostart";

impl UninstallReport {
    /// 规范行序（服务/事件源/密钥/自启），GUI 与 silent 共用。
    pub fn rows(&self) -> [(&'static str, Step); 4];
}
```

- [ ] **Step 1: `install_state` 三态化**
  - `Unknown` 变体 + 文档注释。
  - `ServiceManager::local_computer` 失败 → `Unknown`；`open_service` 失败：`windows_service::Error::Winapi(e) if e.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST.0 as i32)` → `NotInstalled`，其余 → `Unknown`（`ERROR_SERVICE_DOES_NOT_EXIST` 已在文件中被 `uninstall_core` 使用）。

- [ ] **Step 2: 三态消费者显式化**
  - `tray/mod.rs::double_click_entry`：`NotInstalled | Unknown => { message_box_install_hint(); Ok(()) }`（与现状同 UX；注释说明 Unknown 走同一提示的原因：查询失败时给安装提示无害、给"已安装"误判更糟）。
  - `setup/ui.rs::new`：`Mode::Uninstall` 匹配 `Installed => UninstallConfirm, NotInstalled | Unknown => Maintenance`；默认匹配 `Installed | Unknown => Maintenance, NotInstalled => Welcome`。
  - `setup/work.rs::capture_prev_service` 移入 `service.rs`：`Installed{Some(exe)} => Known(exe)`、`Installed{None} => Unknown`、`NotInstalled | Unknown => PrevService::Unknown`（查询失败按"不动注册、尽力启动"处理，与现有 `PrevService::Unknown` 的保守语义一致）。

- [ ] **Step 3: 回滚提升**
  - 把 `PrevService`、`capture_prev_service`、`rollback_for`（改名 `rollback_install`）从 `setup/work.rs` 移入 `service.rs`（文案不变）。
  - `work.rs` 顶部 `use crate::service::{capture_prev_service, rollback_install, PrevService, RollbackOutcome};`，并 `pub use crate::service::RollbackOutcome;`（UI 的 `work::RollbackOutcome` 引用不变）。
  - 新增 `install_with_rollback(req, prev)`：`install_core(req)`，Err → `rollback_install(prev)` → `InstallFailure{error, rollback}`。

- [ ] **Step 4: 英文回滚行单点**
  - `service.rs::rollback_line_en`（映射见 Interfaces；字符串与 `silent.rs` 现有 `eprintln!` 行逐字一致）。
  - `silent.rs::run_install` 的 `Err` 分支改用 `rollback_line_en` + `eprintln!`（NotNeeded 不打印；其余行逐字不变；`std::process::exit(1)` 保持）。

- [ ] **Step 5: CLI 走回滚**
  - `service::install`：`let prev = capture_prev_service();` → `match install_with_rollback(req, &prev)`；成功打印现状输出（逐字不变）；失败：`eprintln!("Install failed: {}", f.error)`（新行，仅失败路径）+ `rollback_line_en` 逐行 eprintln，最后 `Err(f.error)`（退出码非零不变）。
  - CLI `uninstall` 输出保持现状（逐字冻结；不强行改成 rows 渲染）。

- [ ] **Step 6: 卸载报告行单点**
  - 四个 `STEP_UNINSTALL_*` 常量移入 `service.rs`（值不变）；`work.rs` 改为 `pub use crate::service::{STEP_UNINSTALL_EVENT_SOURCE, STEP_UNINSTALL_ENTROPY, STEP_UNINSTALL_SERVICE, STEP_UNINSTALL_AUTOSTART};`（`STEP_UNINSTALL_STOP_SERVICE`/`STEP_UNINSTALL_PURGE`/`STEP_UNINSTALL_REMOVE_DIR` 留在 work，它们是 UI 流程步骤）。
  - `UninstallReport::rows()` 返回四行。
  - `work.rs::run_uninstall`：四行循环 `for (key, step) in report.rows() { step_finished(&tx, key, label_zh(key), outcome_of(step)) }`（中文标签映射留在 work）；purge 行保持现状。
  - `silent.rs::run_uninstall`：四行循环改用 `report.rows()`，输出逐字不变（`step failed: {step_label_en(key)}: {e}`）。

- [ ] **Step 7: 全量 gates**（见 Global Constraints）。

- [ ] **Step 8: Commit** — `refactor(service): install lifecycle owns rollback and honest state; shared report rows`

---

### Task 8: runtime 组合核心（`supervisor`，cfg-free）+ harness

**Files:**
- Create: `src/supervisor.rs`（cfg-free：无 `windows::`、无 tokio、无真实时间）
- Modify: `src/lib.rs`（`pub mod supervisor;`）
- Modify: `src/watchdog.rs`（新增 `SessionView` + `view()`；`snapshot()` 保留到 Task 9 才删）
- Create: `tests/supervisor.rs`（harness：真 `Watchdog` + mock 世界脚本 + 效果记录）
- Read: `.superpowers/sdd/2026-09-12-deepening-structural/w8-design-comparison.md`（选型 B 与三处收窄）

**Interfaces（Task 9 的 Windows 壳按此对接）:**

```rust
/// 进程内单调时间源：core 唯一的时间依赖。
pub trait Clock: Send { fn now_ms(&self) -> u64; fn wall_secs(&self) -> u64; }
pub struct SystemClock { start: std::time::Instant }
impl SystemClock { pub fn new() -> Self; }
impl Clock for SystemClock { /* now_ms = start.elapsed().as_millis(); wall_secs = UNIX 秒 */ }

/// 无线采样（cfg-free；禁止直接使用 adapter::AdapterInfo，那是 Windows 半边）。
pub struct WlanSample { pub ipv4: Ipv4Addr, pub gateway: Option<Ipv4Addr>, pub ifindex: u32 }
pub struct WirelessSample { pub associated: bool, pub wlan: Option<WlanSample> }

pub enum PortalAttempt {
    Replied { status: u16, body: String },
    NoReply,
    TimedOut,
    Failed(String),
}
pub struct WatchdogStep { pub delay: Duration, pub session: SessionView, pub ppp_ip: Option<String> }
pub enum ToastKey { HeartbeatError, RedialFailing }

pub enum Event {
    Started,                                   // 装配完成后恰好一次
    Wake,                                      // 执行器睡到 Reaction.wake_at 后触发
    Command(Command),                          // IPC Redial / SetMode
    LinkSampled(Option<bool>),                 // 2s 链路采样结果
    WatchdogStepped(WatchdogStep),             // Effect::StepWatchdog 的结果
    WirelessSampled(WirelessSample),           // 2s 无线采样结果
    AssociateFinished(Result<(), String>),
    PortalFinished(PortalAttempt),
    WlanProbeFinished(ProbeVerdict),
    Heartbeat(HeartbeatStatus),
    NotifyResult { key: ToastKey, delivered: bool },
    WirelessDied,                              // 无线 lane worker panic/退出
    Stop,
}

pub enum Effect {
    // 有线守护（Main lane 内联执行）
    StepWatchdog, RequestRedial, SetWatchdogLink(Option<bool>), Hangup,
    // 采样
    SampleLink, SampleWireless,
    // 无线接管（Wireless lane worker 串行执行）
    CleanupStaleRoutes,
    Associate(String),                         // profile
    Disassociate,
    EnsureRoutes { dests: Vec<Ipv4Addr>, gateway: Ipv4Addr, ifindex: u32 },
    SuppressMetric { ifindex: u32, target: u32 },
    ReleaseMetric,
    TeardownRoutes,
    Settle(Duration),                          // /32 路由生效等待（3s）
    PortalAuth { src_ip: Ipv4Addr, timeout: Duration },   // URL 由壳侧构建（含密码，绝不进 Effect/Debug/日志）
    WlanProbe { src_ip: Ipv4Addr, gateway: Option<Ipv4Addr>, url: String },
    // 钩子
    PersistMode(NetMode),
    Notify { key: ToastKey, title: String, body: String },
}
pub enum Lane { Main, Wireless }
impl Effect { pub fn lane(&self) -> Lane; }    // 纯分类函数，单测固定每条恰好一车道

pub struct Reaction {
    pub effects: Vec<Effect>,
    pub snapshot: Option<StateSnapshot>,       // Some ⇔ 组合快照变化（全服务唯一发布出口）
    pub wake_at: Option<u64>,                  // 绝对单调毫秒；None = 无定时器
}

/// 服务总控（守护 + 无线接管 + 事件环 + 快照的组合决策点）。
/// 不 derive Debug/Clone（内部持有密码明文）。
pub struct Supervisor { /* private */ }
impl Supervisor {
    pub fn new(cfg: Config, password: String, clock: impl Clock + 'static) -> Self;
    pub fn on(&mut self, event: Event) -> Reaction;   // 纯同步；任意序列不 panic、不返回 Err
    pub fn snapshot(&self) -> StateSnapshot;          // watch 通道初值
}
```

`src/watchdog.rs` 新增（`snapshot()` 暂留）：

```rust
/// 组合层组装 StateSnapshot 所需的有线会话事实（占位字段的替代）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionView {
    pub status: SessionStatus,
    pub since_unix: Option<u64>,
    pub last_drop_reason: Option<String>,
    pub redial_attempts: u32,
}
impl Watchdog { pub fn view(&self) -> SessionView; }
```

**Invariants（计划冻结；每条都要在 `tests/supervisor.rs` 有对应断言）:**

1. **I1 纯粹性**：`on` 无 I/O、只读 `Clock`；相同状态+事件+时钟 ⇒ 相同 `Reaction`；任意事件序（重复/迟到结果）惰性无害。
2. **I2 快照单出口**：快照只经 `Reaction.snapshot` 离开核；壳侧只有一个 `snap_tx.send` 调用点。
3. **I3 状态机节拍**：`Effect::StepWatchdog` 只可能来自 (a) 到点的 `Wake`、(b) `Command::Redial`、(c) 链路 false→true 边沿。心跳/无线/事件环类事件永不触发它（79 次重拨事故结构性不可能）。
4. **I4 车道**：同车道效果严格顺序执行；效果结果只经 Event 回灌；Main 车道与事件处理同步（等价今天的 run_once 独占），Wireless 车道在 worker 中执行（等价今天的 manager 任务）。
5. **I5 链路门控**：已知链路态只接受 `Some(_)` 覆盖（首次采样例外）；拔线期间（`Some(false)`）不产生 `StepWatchdog`；`Wake` 触发的步进在同一反应链里先 `SampleLink`（收窄：不等 2s 采样窗口）。
6. **I6 无线生命周期**：`EnsureRoutes` 只在 `PortalAuth` 且 wlan IP+网关齐备时、`Settle` 之前发出；`TeardownRoutes` 在 `Disassociate`、`Stop`（以及 worker 的 `RouteGuard::drop` 兜底）发出；metric 压制仅 standby 或 exclusive 且有线不健康，exclusive+有线健康时释放；`verdict` 在每次认证尝试后与 `Disassociate` 清空、只由 `WlanProbeFinished` 设置（过去的无限重认证 Critical）；Joining 超时 60s → `brain.restart()`。
7. **I7 绝对唤醒**：`wake_at`/`link_poll_at`/`wireless_tick_at` 都是绝对毫秒；`Wake` 只重新判定哪些 deadline 到点；事件频率不影响任何 deadline。
8. **I8 通知节流**：`Notify` 仅在 per-key 30 分钟窗口过期时发出；窗口只在 `NotifyResult{delivered:true}` 后开启；重拨失败 10 分钟阈值与拔线暂停豁免保持现状。
9. **I9 结果护栏**：`sample_in_flight`/`auth_in_flight`/`associate_in_flight`/`probe_in_flight` 使重复/迟到结果成为 no-op。
10. **I10 生命周期**：`Started` 恰好一次且最先；`Stop` 恰好一次且最后；`Stop` 反应的效果在壳侧不被取消（自包含铁律）。
11. **I11 降级**：结果缺席（执行器 bug）只会让该活动停步，绝不双发；丢失事件不产生额外拨号。

**Steps:**

- [ ] **Step 1: `Watchdog::view()`**（`src/watchdog.rs`）：实现 `SessionView` + `view()`（从现有字段直读）；在 `tests/watchdog.rs` 增一条 `view()` 与 `snapshot()` 有线字段一致的测试（`snapshot()` 保留）。
- [ ] **Step 2: 写 interface 骨架 + 最早 3 个场景**（`src/supervisor.rs` + `tests/supervisor.rs`）：协议类型 + `Supervisor::new/on/snapshot` 的签名与 `todo!()` 级实现；harness（`FakeClock`、脚本化采样、效果记录、内联执行 Main 车道并驱动真 `Watchdog`）；场景：
  1. `started_then_first_wake_dials`：`Started` → 首拍先 `SampleLink`，`Wake` 后 `[SampleLink, SetWatchdogLink?, StepWatchdog]`（顺序断言，I3/I5）；
  2. `cable_out_never_dials`：`LinkSampled(Some(false))` 后到点 `Wake` 无 `StepWatchdog`、`wake_at=+5s`（I5）；
  3. `link_restore_redials_immediately`：`Some(false)→Some(true)` 边沿 → `[SetWatchdogLink(Some(true)), RequestRedial, StepWatchdog]` + 事件环行（I3/I5）。
  跑测试：RED（未实现）→ 实现到绿。

- [ ] **Step 3: 实现有线调度与快照（I1–I5/I7/I10/I11）**：`next_wake`/`link_poll_at` 账本、`Wake` 到点判定、`Command::Redial/SetMode`（含 `PersistMode`、事件环、mode 广播）、`WatchdogStepped` 回灌（`wake_at=now+delay`、`ppp_ip`、failing-since 通知钩子）、`Stop` 收尾反应、`snapshot()` 组合（wired `SessionView` + ppp_ip + heartbeat + mode + wireless + 事件环）。
- [ ] **Step 4: 实现无线编排（I6 全量）**：`wireless_tick_at` 2s 绝对节拍 + in-flight 护栏；`Brain` 决策与 `Action` 四臂到效果的映射（关联/让位/认证/探测）；`EnsureRoutes+Settle+PortalAuth` 顺序；verdict 清空点；metric 压制/释放算术；join 超时；`WirelessDied` 降级（事件环 + 默认无线快照）。
- [ ] **Step 5: `Effect::lane()` 单测**：每条效果恰好一条车道（`tests/supervisor.rs` 或用 `const ALL: [EffectKind; N]` 表驱动）。
- [ ] **Step 6: 补齐 harness 场景（每条不变量至少一条）**：
  - `verdict_cleared_after_auth_then_probe_now`（I6；无限重认证回归）
  - `ensure_routes_only_on_portal_auth`（I6）
  - `teardown_on_release_and_stop`（I6/I10）
  - `takeover_debounce_and_release_after`（Brain 世界输入接线，8s/10s）
  - `metric_suppress_release_by_mode_and_wired_health`（I6）
  - `event_storms_never_fragment_backoff`（I3/I7：连续 `Heartbeat`/`WirelessSampled` 后 `wake_at` 间隔不变）
  - `notify_throttle_only_on_delivered`（I8）
  - `late_and_duplicate_results_are_inert`（I9/I11）
  - `join_timeout_restarts_association`（I6）
  全部基于 `FakeClock`；时间线以毫秒推进，测试总时长 < 1s。
- [ ] **Step 7: 全量 gates**（见 Global Constraints）。
- [ ] **Step 8: Commit** — `feat(supervisor): pure service composition core with scripted-timeline harness`

---

### Task 9: runtime 壳切换（Windows）+ 占位快照退场

**Files:**
- Modify: `src/runtime.rs`（重写 `run()`；删除 `wireless_manager`/`ManagerCfg`/四个内部通道/`compose`/`Notifier`/唤醒机制）
- Modify: `src/watchdog.rs`（删除 `snapshot()`（占位字段）与 `eth_link()`；`set_eth_link` 保留）
- Modify: `tests/watchdog.rs`（断言改用 `view()`；runtime 胶水相关的两段测试由 Task 8 harness 覆盖，删除）
- Read: `.superpowers/sdd/2026-09-12-deepening-structural/w8-design-comparison.md` §B（执行器义务、车道、停止握手）

**接口**: 见 Task 8 的 `Supervisor`/`Event`/`Effect`/`Lane`/`Clock` 与 `SessionView`。

**Steps:**

- [ ] **Step 1: 骨架**：`run()` 改为「构造 `Supervisor(cfg, pass, SystemClock)` + IPC server（`watch::channel(core.snapshot())`）+ 心跳 actor（现有装配逐字保留）+ 无线 worker 通道 + 主循环 select」；先不删旧 `wireless_manager`，两者并存过渡到绿。
- [ ] **Step 2: 执行器**：`apply_main(effect) -> Vec<Event>`（Windows 映射）：
  `StepWatchdog` → `watchdog.run_once().await`（独占）→ 采样 `adapter::ppp_adapter_ip()` → `WatchdogStepped{delay, view(), ppp_ip}`；
  `RequestRedial` → `watchdog.request_redial()`；`SetWatchdogLink` → `watchdog.set_eth_link(up)`；`Hangup` → `watchdog.shutdown().await`；
  `SampleLink` → `spawn_blocking(adapter::ethernet_link_up)` → `LinkSampled`；`SampleWireless` → 一次 `spawn_blocking`（`wlan::associated` + `adapter::wlan_adapter` 采样为 `WlanSample`）→ `WirelessSampled`；
  `PersistMode` → `cfg.wireless.mode` + `cfg.save`（失败仅 warning）；`Notify` → `notify::toast` → `NotifyResult{delivered}`。
- [ ] **Step 3: 无线 worker**：`LaneMsg::{Effect, Shutdown}`；拥有 `RouteGuard`；`Associate` → `wlan::associate(profile)` → `AssociateFinished`；`PortalAuth{src_ip,timeout}` → `portal::build_login_url`（cfg+密码）+ 脱敏日志 + `timeout` 包裹 `portal::portal_get` → `PortalFinished`（`Replied/NoReply/TimedOut/Failed`）；`WlanProbe` → `probe::probe_once` → `WlanProbeFinished`；`EnsureRoutes/SuppressMetric/ReleaseMetric/TeardownRoutes/CleanupStaleRoutes` → `RouteGuard`/`routes`；`Disassociate` → `wlan::disassociate`；`Settle` → `tokio::time::sleep`（与 stop 竞争）。worker 退出时 `RouteGuard::drop` 兜底。
- [ ] **Step 4: 主循环 select**（顺序即优先级）：`stop.cancelled()` → `Event::Stop` 并跳出；`cmd_rx.recv()` → `Command`；`hb_rx.changed()` → `Heartbeat`；`wl_rx.recv()` → worker 回灌事件（`None` → `WirelessDied`）；`sleep_until(wake_at)` → `Wake`。每轮：`let r = core.on(ev); wake_at = r.wake_at; if let Some(s)=r.snapshot { snap_tx.send(s) }`（唯一发布点）；Main 效果内联执行、结果事件同轮递推回灌（用 `VecDeque` 队列，避免递归）；Wireless 效果发 worker。
- [ ] **Step 5: 停止握手**：`core.on(Event::Stop)` → 发布（若有）→ Main 效果不可取消执行（`Hangup`）→ 向 worker 发 `Shutdown(teardown 效果)` 并等待其退出；`runtime.shutdown_timeout(10s)` 保持。
- [ ] **Step 6: 删除旧物**：`wireless_manager`、`ManagerCfg`、`mode_tx/wired_tx/wl_tx/ev_tx` 与对应 `rx`、`compose`、`Notifier`/`failing_since`（已进核心）、`wake_at`/`link_tick` 手摆、`mode_text`（若只被旧代码用）；`Watchdog::snapshot()`/`eth_link()`；同步 `tests/watchdog.rs`。
- [ ] **Step 7: 行为冻结评审清单**（评审者逐条对照）：拔线不拨号 + 5s 轮询；插线即拨；退避不被事件切碎；接管去抖/让位 10s；/32 路由三出口 + 启动清残留；verdict 清空；join 60s 超时；事件环文案（`Service started, mode …` / `Ethernet link restored, redialing` / `Wireless: …`）逐字；重拨失败 10 分钟 toast 与拔线豁免；SetMode 落盘 + 事件；快照单出口；停止时 hangup + teardown + disassociate；`start_all` 签名与 `service_main` 不变。
- [ ] **Step 8: 全量 gates**（含交叉编译）；`git grep` 确认无残留引用。
- [ ] **Step 9: Commit** — `refactor(runtime): supervisor executes decisions; delete placeholder snapshot`

> 真机验收属 D7，在合并后执行（部署最新构建 → 服务/托盘/拨号/75s 稳定 → 一次手动重拨 + 托盘状态核对；无线现场测试按需）。

---

## Self-Review

- **Spec coverage:** W6→Task 6；W7→Task 7；W8→Tasks 8/9（选型 B 已定，见 `w8-design-comparison.md`）。
- **Placeholder scan:** 无 TBD/TODO；Task 8/9 的接口、不变量、场景与执行器映射均为具体文本。
- **Type consistency:** `Session/SyncSession/status_snapshot/set_mode_confirmed`；`InstallState::Unknown`、`PrevService`、`rollback_install`、`InstallFailure`、`rollback_line_en`、`UninstallReport::rows`、四个 `STEP_UNINSTALL_*` 常量；`Supervisor/Event/Effect/Lane/Reaction/Clock/SystemClock/SessionView/WlanSample/WirelessSample/PortalAttempt/WatchdogStep/ToastKey` 在任务间一致。Task 8 用 `watchdog.view()`；Task 9 删 `snapshot()` 前先确认全库引用只剩 `runtime.rs` 与 `tests/watchdog.rs`。
- **W8 选型收窄记录:** ① watchdog 留在原位（不迁移 Dialer/Prober）；② 采纳"Wake 前新鲜链路采样"；③ 快照变化才推，但心跳状态变化必须覆盖；④ 不引入 `Effect::Log`（核心直接 `log::`）；⑤ `PortalAuth` 不携带 URL（壳侧构建，避免明文密码进 `Debug`）。
