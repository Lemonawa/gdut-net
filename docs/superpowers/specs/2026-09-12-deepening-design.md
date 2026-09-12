# Deepening 设计规格（2026-09-12）

> 来源：`improve-codebase-architecture` 走查报告（7 个候选）+ 主会话 grilling 决策。
> 本规格是两波执行计划的权威设计；第一波计划见 `plans/2026-09-12-deepening-mechanical.md`，
> 第二波（结构深挖）计划待第一波落地后另写（含 #2 的 design-it-twice）。

## 目标

按 codebase-design 的深挖标准（深 module = 小 interface + 大 implementation；seam 在 interface 处；interface 即测试面），
消化走查发现的浅 module 与漂移，使变更局部化、测试可覆盖核心决策。

**词汇**：module / interface / implementation / depth / deep / shallow / seam / adapter / leverage / locality。
域内名称以 `CONTEXT.md` 为准（托盘、日常界面、安装器、守护、探针、无线接管……）。

## 决策记录（用户已授权代理决定技术问题）

| # | 决策 |
|---|------|
| D1 | 7 个候选全做，合并为 8 个任务；其中 #7（死接口）拆开：`Watchdog::snapshot` 占位数据并入 #2（runtime 组合缝），其余死接口独立成机械任务 |
| D2 | 两波执行：机械波（任务 1–5）先落地；结构波（任务 6–8）另写计划，避免大改与小改在同批评审里互相遮蔽 |
| D3 | 顺序：状态渲染 → 路径/日志 → 死接口 → Win32 原语 → HTTP/portal → IPC 会话 → 安装生命周期 → runtime 组合缝 |
| D4 | 交付流程沿用 SDD：分支 worktree + 每任务实现/评审 + 双轨 gates + 整支终审 + 合并 main；机械任务单轮评审，结构任务完整评审轮 |
| D5 | 日志策略统一 **5MB × keep5**（GUI 进程与服务同策略，与 AGENTS/CONTEXT 文档一致）；`logging` module 持有策略常量 |
| D6 | #2（runtime 组合缝）先做 design-it-twice（并行子代理给 2–3 个 interface 设计再对比选型）；其余任务直接设计 |
| D7 | 真机验证：机械波只跑 gates；结构波完成后在本机跑一次 silent 安装循环 + 服务健康检查（走 `gdut-switch` 预授权通道） |

## 工作项

### W1 状态呈现 module（#1）

**问题**：snapshot → 人话/图标的同一规则有 4 份实现（`tray/mod.rs` 图标与中文表、`gui.rs` 文案与色板、`ipc/protocol.rs` 英文表、CLI 消费），唯一无测试覆盖的是托盘路径。

**设计**：新 cfg-free `src/status.rs` 拥有：
- `Light { Wired, Wireless, Busy, Off }`、`Primary { ServiceDown, Connected, WirelessOnline, Dialing, Backoff, AuthFail, Idle }`、`view(Option<&StateSnapshot>) -> StatusView`；
- 中文词表（`Primary::word_zh/egress_zh`、`session_zh`、`wphase_zh`、`status_line_zh`）；
- 英文词表（`session_en`、`wphase_en`、`heartbeat_en`、`mode_en`、`wireless_en`），逐字保持 CLI 现状（`Connected` / `Backoff (retrying)` / `Auth failed` …）。
渲染端只做适配：托盘把 `Light` 映射到 `IconKind`，GUI 把 `Primary` 映射到色板与印章，CLI/setup 取英文/中文词。
`ipc/protocol.rs` 只留线上格式与 `uptime_text/format_uptime`。

**验收**：新 `tests/status.rs` 穷举优先级与词表；GUI 既有 `status_view` 测试不改仍过（视觉冻结）。

### W2 路径与日志策略单一来源（#5）

**问题**：`C:\ProgramData\gdut-net` 字面量散落 10 处；日志轮转三方不一致（GUI 硬编码 5MB×2、`LogCfg` 默认 5MB×5、文档 5MB×5）。

**设计**：新 cfg-free `src/paths.rs` 持有布局常量与派生函数（`DATA_DIR/CONFIG_PATH/PBK_PATH/LOGS_DIR/INSTALL_DIR/install_dir()/install_exe()/setup_exe()`）；
`logging.rs` 持有 `LOG_MAX_SIZE_MB = 5`、`LOG_KEEP_FILES = 5`，GUI 与服务同时消费；`setup::config_path()/install_dir()` 退化为对 paths 的委托。
不改 `--config` 覆盖语义（服务仍从参数解析配置路径）。

**验收**：`tests/paths.rs` 固定布局；`LogCfg::default()` 与 logging 常量一致；全库不再出现裸字面量（clippy/grep 检查）。

### W3 死接口清理（#7 其余）

**问题**：`ServerMsg::Ack` 无人实现（client 反而要吞）、`InstallOutcome.student_id` 写入无读者、`eventlog::ensure_dir` 零调用。

**设计**：删除三处 + 连带注释/导出；`restore_service_path`/`start_service`/`delete_service` 有调用者，保留。

**验收**：gates 全绿；`grep` 无残留引用。

### W4 Win32 原语（#6a）

**问题**：`wide()` 在 7 个 module 各写一遍；注册表 create/set/close 样板在托盘 AUMID、托盘自启、卸载键三处重复；stop-poll 循环 4 处。

**设计**：新 `src/win32.rs`：cfg-free `wide()`；`#[cfg(windows)] mod reg` 提供 `set_string(root, subkey, name, value)` 与 `set_dword(root, subkey, name, value)`，错误信息带值名与注册表码。
替换点：`tray/mod.rs`（AUMID、autostart）、`shell.rs`（卸载键的 sz/dword 写入）、全部 `wide()` 调用点（tray/shell/eventlog/crypto/setup/ras）。
不碰 `crypto.rs` 的 DPAPI 读写路径（只换 `wide`）；不合并 stop-poll（语义差异大，留给 W8 的 runtime 决策层裁决）。

**验收**：`tests/win32.rs` 固定 `wide` 行为；Windows 交叉 clippy 全绿；注册表行为不变（真机由结构波末尾的安装循环覆盖）。

### W5 HTTP 原语与 portal 流程（#6b）

**问题**：手写 HTTP/1.0 GET 两份（probe / portal）；`parse_status_code` 与认证跳转判定锁在 Windows 半边；portal 的 URL 拆分与 `probe::parse_http_probe_target` 重复。

**设计**：新 `src/http.rs`：
- cfg-free 纯解析：`parse_url(&str) -> Option<Target{host,port,path}>`、`parse_status_code(&str) -> Option<u16>`、`is_auth_redirect(&str) -> bool`（`wlanacip|nexturl|portal`）；
- `#[cfg(windows)]`：`Request{url, bind_ip, user_agent, timeout, max_bytes}` + `get() -> Result<Response{status, location_lower, body}>` + `get_async()`（spawn_blocking），源地址绑定是 interface 参数（物理网卡 vs WLAN 源 IP）。
probe 与 portal 改为消费该 module（探针保留 Option 语义与脱敏日志；portal 保留 `python-requests/2.31.0` UA 与 8s/64K 参数——设备计数与 SYN 重传实证，不得更改）。
**非目标**：manager 的重试策略归 Brain（5/15/30），CLI `wireless test` 的 3 连试是现场便利，两者不合并。

**验收**：`tests/http.rs` 覆盖 URL/状态行/认证跳转；probe 既有纯测试不改仍过。

### W6 IPC 会话 module（#3，第二波）

**问题**：三个调用者（tray/work/cli）手搓 runtime + connect + "连上先收首帧 snapshot" 的隐式协议；executor 内 sleep 的 hazard 转嫁调用者；`Ack`（已由 W3 删除）。
**设计**：一个深 module 拥有连接、握手、命令/应答与超时；调用者只见 snapshot 与命令结果。具体 interface 在第二波计划中设计。

### W7 安装生命周期 module（#4，第二波）

**问题**：`install_core` 的部分失败面只有 `setup/work` 包回滚（CLI 裸调）；报告渲染 3 份；`install_state` 把"查询失败"说成 `NotInstalled` 并驱动用户可见决策。
**设计**：core 拥有完整生命周期（含回滚）与三态真实状态；报告行收进单一 module，UI/CLI/silent 做 adapter。第二波设计。

### W8 runtime 组合缝（#2 + `Watchdog::snapshot`，#7a，第二波）

**问题**：历史事故全在组合层 ordering invariants（无载波 756、去抖、让位、verdict 清理、snapshot 单一出口、绝对唤醒），而这些只由注释维持；组合层没有 seam 与 harness。
**设计**：先 design-it-twice 产出 2–3 个 interface 设计（决策 D6），选定后把"事件 → 动作"决策收进 cfg-free module（沿用 wireless Brain 的既有模式），Windows 侧留 adapter；`Watchdog::snapshot` 的占位字段随之削平。第二波设计。

## 执行

- 第一波计划：`plans/2026-09-12-deepening-mechanical.md`（任务 1–5），分支 `refactor/deepening`，worktree 由 using-git-worktrees 建。
- 每任务：TDD + 双轨 gates（`cargo test && cargo clippy -- -D warnings && cargo fmt` +
  `cargo check --target x86_64-pc-windows-msvc && cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings`），
  实现者 → 评审者（机械任务单轮，结构任务完整轮），任务间提交。
- 整支终审 + 合并 main（沿用 SDD；机械波不推远端，结构波合并时统一推）。
- 真机验证按 D7。

## 非目标

- 不合并 manager 与 CLI 的 portal 重试策略（语义不同，见 W5）。
- 不改 CLI/协议输出文本（W1 逐字冻结）；不动 GUI 视觉（W1 只换数据来源）。
- 不重构 `crypto.rs` 的 DPAPI 路径（W4 只换 `wide`）。
- stop-poll 合并推迟到 W8（由决策层统一裁决）。
- 不引入新依赖。
