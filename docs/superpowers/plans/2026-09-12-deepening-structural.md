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
- Task 8: 待 design-it-twice 选型确定（见 Task 8）。

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

### Task 8: runtime 组合缝（W8）

> 本任务在 design-it-twice 选型后由控制器补全（设计输出见 SDD 工作区 `w8-design-*.md`）。
> 约束（来自 spec W4/W8 与本节决策记录）：
> - 决策 core cfg-free、Linux 可测；OS 效果（RAS/WLAN/routes/portal/probe）经既有或新增 port 注入（生产 + mock 两 adapter 才建 port）。
> - 行为冻结：拔线不拨号、去抖/让位、/32 路由与 metric 三出口、绝对唤醒（事件唤醒不跑状态机）、快照单出口、IPC 命令与事件环/通知钩子。
> - `Watchdog::snapshot` 的占位字段（heartbeat/mode/wireless/events）纳入本任务：组合责任收进新 module，占位假数据消失。
> - `cargo test` 新增的 harness 走 Linux（mock adapter 驱动多步时间线），与 `tests/watchdog.rs`、`tests/wireless_brain.rs` 同风格。

---

## Self-Review

- **Spec coverage:** W6→Task 6；W7→Task 7；W8→Task 8（选型后补全）。D1–D7 约束贯穿。
- **Placeholder scan:** Task 8 为显式外部依赖（design-it-twice 输出），将在执行前补全；其余任务无可变占位。
- **Type consistency:** `Session/SyncSession/status_snapshot/set_mode_confirmed`；`InstallState::Unknown`、`PrevService`、`rollback_install`、`InstallFailure`、`rollback_line_en`、`UninstallReport::rows`、四个 `STEP_UNINSTALL_*` 常量名在任务间一致。
