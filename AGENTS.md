# AGENTS.md — gdut-net

> Rust 工程（`lib gdut_net` + 三个 bin）：Windows 服务守护 PPPoE 拨号 + 可选无线接管；用户会话托盘/中文日常 GUI；发布物是单文件 `gdut-net-setup.exe` 安装器。
> **领域词汇、Rules、真机实战陷阱全部在 [`CONTEXT.md`](CONTEXT.md)**；决策记录在 [`docs/adr/`](docs/adr/)；桌面运维手册在 [`docs/desktop-kit.md`](docs/desktop-kit.md)；真机验收单在 [`docs/acceptance.md`](docs/acceptance.md)；设计/计划历史在 `docs/superpowers/`。

## 架构

- **lib+bin**：`src/lib.rs` 暴露全部模块；三个 bin：`gdut-net`（`src/main.rs` → `cli::dispatch()`）、`gdut-net-setup`（`src/bin/gdut-net-setup.rs` → `setup::entry()`，Windows 安装/维护 GUI，非 Windows 编译为明确报错）、`gdut-net-pack`（host 打包工具，把 payload 追加到 setup 尾部）。新增模块需在 `lib.rs` 注册。
- **平台分层**：纯逻辑（`backoff`/`config`/`heartbeat::spec`/`ipc::protocol`/`watchdog`/`probe` 判定函数/`wireless` 的 Brain 与 portal 纯半/`payload`/`packaging`/`setup_args`/`shell_shortcuts`）无 `windows::` 依赖，Linux 可跑 TDD；Win32 胶水（`ras`/`adapter`/`service`/`runtime`/`eventlog`/`notify`/`tray`（`mod.rs` 托盘本体 + `gui.rs` 日常窗口）/`shell.rs`（开始菜单/卸载键）/`setup::{ui,work}`/`wireless::{routes,wlan,test}`）仅 `cfg(windows)`，靠交叉编译验证。
- **运行拓扑**（ADR-0001）：服务 `SYSTEM`（Session 0，无 UI）+ 托盘用户会话进程，命名管道 `\\.\pipe\gdut-net` JSON-line 通信。服务内三个 actor：watchdog（拨号/探测）、heartbeat（可选）、wireless manager（无线接管），快照经 IPC 广播。
- **状态机节奏**：runtime 主循环只在"绝对唤醒时刻到点/显式命令"时跑 `run_once`，无线快照等事件唤醒只推快照——否则退避 sleep 被 2s 切碎（见 CONTEXT.md 陷阱）。
- **发布形态**：单文件 `gdut-net-setup.exe` = setup 本体 + 载荷容器（`src/payload.rs`：`[setup][files][TOC][24B footer]`，magic `GDUTPAK1`，逐项 sha256；截断/坏包拒绝安装）。`gdut-net-pack`（`src/packaging.rs`）打包；开发态未打包的 setup 回退读自身旁边 `payload/` 目录；`packaging/payload/` 是公开发布文件清单（`shell_shortcuts` 一致性测试冻结），`packaging/personal/` 与 `packaging/dev/` 不进发布物。主程序 `gdut-net.exe` 保持 portable（不联网、不下载、不更新）。
- **安装与 GUI**（ADR-0007）：`setup` 自提权、4 屏中文向导 + 维护页（修复/卸载两档）；装到 `C:\Program Files\gdut-net\`，开始菜单 `GDUT Net` 10 项 + 应用和功能键；托盘单实例（mutex `gdut-net-tray-singleton` + 命名事件 `gdut-net-tray-show`），左键开常驻中文 egui 窗口（关窗=隐藏），右键原生中文菜单。

## 关键约束（违反即错）

- **物理适配器绑定**：心跳/探测/拨号一切发包显式绑物理网卡，绝不走 TUN/wintun；无线接管时一切发包绑 WLAN 源 IP。
- **无线接管全套规则**（/32 路由、metric 压制 100、拔线期间绝不拨号、Mihomo 不得设 `interface-name`、eportal 语义）：`CONTEXT.md` Rules 是唯一权威，改无线代码前先读。
- **掉线判定**（ADR-0003）：以流量探测为准，不单看 `RASCS_Connected`；两级网关 ICMP→HTTP 复核，连续 2 次失败才判掉线。
- **不做**：不扫描虚拟网卡、不装 LSP/驱动/WinPcap、不做限速绕过（无线网页认证由 wireless 模块承担，ADR-0005）。
- **密码**：`config.toml` 的 `password_blob = GDUT1:<hex>:<base64>` 为 DPAPI 机器级密文；含明文密码的 portal URL 永不落日志（打码只留 host+path）。
- **心跳**：默认 `heartbeat.enabled=false`；GDUT 变体从抓包规格洁净室实现，**禁止逐行翻译** `drcom-generic`(AGPL)/`gdut-drcom`(GPL)（ADR-0002）。
- **重拨**：指数退避 `1s→300s` 封顶，稳定 `300s` 重置；`691` 认证失败固定 `600s`（`backoff::AUTH_FAIL_DELAY`）。
- **日志**：`flexi_logger` 按大小滚动 `5MB×5`；`log.event_log=true` 时 warn/error 镜像 Windows 事件日志。
- **防乱码（语言策略，ADR-0007）**：**控制台输出必须英文**（CLI/日志/脚本/`.bat`/.ps1 回显）——中文 Windows 控制台 GBK 会乱码；**GUI 必须中文**（安装器向导/维护页、日常窗口、托盘菜单、`说明.txt`）——GUI 经系统字体渲染无乱码问题，面向学生用户。源码注释中英文皆可。
- **失败自动回退**：任何改网络的操作必须自包含、带自动回滚；用户断网窗口内无 AI 可达。
- **桌面运维（安装形态）**：产品脚本 + 个人运维脚本同居 `C:\Program Files\gdut-net\`；文件清单、场景、铁律见 `docs/desktop-kit.md`；随手堆诊断脚本是错（用完即删）。

## 命令

```bash
cargo test                                        # 纯逻辑套件（Linux 可跑）
cargo test --test wireless_brain -v               # 单集成测试（无线状态机）
cargo clippy -- -D warnings                       # Linux 侧
cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings  # 必跑：Win32 胶水
cargo fmt --check && cargo fmt
cargo check --target x86_64-pc-windows-msvc       # Linux 上验证 Windows 代码（需该 target）
cargo build --release                             # Windows 上产出 gdut-net/setup/pack 三个 exe
```

CI（`.github/workflows/`）：`linux-test`（test/clippy/fmt）+ `windows-build`（test --release/clippy --all-targets/build --release → pack 出单文件 `gdut-net-setup.exe`），`push→main` 与 PR 触发。

本地交叉构建 + 打包（比等 CI 快，需 `cargo-xwin`）：

```bash
mise exec -- cargo xwin build --target x86_64-pc-windows-msvc --release
cargo build --release --bin gdut-net-pack
./target/release/gdut-net-pack \
  --setup target/x86_64-pc-windows-msvc/release/gdut-net-setup.exe \
  --file target/x86_64-pc-windows-msvc/release/gdut-net.exe \
  --payload-dir packaging/payload \
  --out dist/gdut-net-setup.exe
```

## 真机操作（这台机器）

- 安装形态（2026-09-11 迁移完成）：程序与全部脚本在 `C:\Program Files\gdut-net\`（产品脚本 + 个人运维脚本同居），配置/日志在 `C:\ProgramData\gdut-net\`，入口在开始菜单 `GDUT Net` 文件夹。
- 部署新版：新 `gdut-net-setup.exe` 放桌面工具包 → 双击 `一键切换.bat`（计划任务 `gdut-switch`，脚本会弹第二个窗口，别关）→ 脚本内部走 `gdut-net-setup.exe --silent --keep-password`（不再有明文密码）；失败回滚链 `rollback.bat` → DrMain；桌面工具包保留作 fallback。
- 现场验证无线：管理员 `.\gdut-net.exe wireless test`（一次性，自动断开不留状态）。
- WSL 调 Windows PowerShell 的绝对路径、提权限制、脚本陷阱：见 `CONTEXT.md` 工程陷阱。
