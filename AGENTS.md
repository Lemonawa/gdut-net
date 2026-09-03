# AGENTS.md — gdut-net

> 单 Rust 二进制（`lib gdut_net` + `bin gdut-net`），Windows 服务守护 PPPoE 拨号。详见 `CONTEXT.md`（领域词汇）与 `docs/adr/`（决策）。

## 架构

- **lib+bin**：`src/lib.rs` 暴露全部模块，`src/main.rs` 仅 `fn main(){ gdut_net::cli::dispatch() }`。新增模块需在 `lib.rs` 注册。
- **平台分层**：纯逻辑（`backoff`/`config`/`heartbeat::spec`/`ipc::protocol`/`watchdog`/`probe` 判定函数）无 `windows::` 依赖，Linux 可跑；Win32 胶水（`ras`/`adapter`/`service`/`eventlog`/`notify`/`tray`）仅 `cfg(windows)`，靠交叉编译验证。
- **服务/托盘分离**（ADR-0001）：服务 `SYSTEM`（Session 0，无 UI）+ 托盘用户会话进程，命名管道 `\\.\pipe\gdut-net` JSON-line 通信。托盘崩溃不影响拨号。

## 关键约束（违反即错）

- **物理适配器绑定**（`CONTEXT.md` Rules）：心跳/探测/拨号一切发包显式绑物理网卡，绝不走 TUN/wintun。`adapter::physical_adapter()` 找 `IF_TYPE_ETHERNET_CSMACD+Up+非虚拟`，有网关者优先。心跳 `bind` 本地 `0.0.0.0:61440` 失败 → `CompatModeUnavailable` 报错而非静默（端口被官方客户端占用）。
- **掉线判定**（ADR-0003）：以流量探测为准，不单看 `RASCS_Connected`。两级：网关 ICMP（`IcmpSendEcho2Ex` 绑源 IP，1500ms，失败 `probe_interval/4` 复核）→ 连续 2 次 `LinkDown/Kicked` 才判 `Drop` 触发重拨。僵死会话（RAS 显示已连但被踢）必须能检出。
- **术语**：拨号条目≠宽带连接/VPN；掉线≠断线；兼容模式≠保活模式；探针≠ping 检测。见 `CONTEXT.md`。
- **不做**：不扫描虚拟网卡、不装 LSP/驱动/WinPcap、不做无线网页认证、不做限速绕过。
- **密码**：`config.toml` 中 `password_blob = GDUT1:<hex>:<base64>`，DPAPI 机器级 `CRYPTPROTECT_LOCAL_MACHINE` + `HKLM\SOFTWARE\gdut-net\entropy`（32B，REG_BINARY）。`unwrap_blob` 需判 `GDUT1` 前缀与 hex/base64 合法性。
- **心跳**：默认 `heartbeat.enabled=false`。GDUT 变体（ADR-0002）无需登录，四报文 20s 周期打 `server:61440`，`seed[0]&3` 选校验模式，从抓包规格洁净室实现，**禁止逐行翻译** `drcom-generic`(AGPL)/`gdut-drcom`(GPL) 源码。
- **重拨**：指数退避 `1s→300s` 封顶，稳定 `300s` 重置；`691` 认证失败固定 `600s`（`backoff::AUTH_FAIL_DELAY`），不进快退避。
- **日志**：`flexi_logger` 按大小滚动 `5MB×5`（`Criterion::Size`+`KeepLogFiles`），`config` 事件日志无。`cargo clippy` 含 `chunks_exact_to_as_chunks` 等严格 lint。
- **防乱码**：所有面向用户的输出（CLI 提示、日志、脚本回显）必须英文。Windows 控制台默认 GBK 936，UTF-8 中文必乱码；`.bat`/`.ps1` 保持纯 ASCII/英文，`switch-v4.ps1` 日志 `Out-File -Encoding utf8`，PowerShell 管道避免 `*>&1 | Out-File`（见陷阱）。
- **失败自动回退**：任何改网络的操作必须自包含且带自动回退，断网窗口内无人工可达也须自愈。`switch-v4.ps1` 为唯一切换入口：预检查→部署新 `exe`→`install`（幂等）→杀官方 `DrMain/rasdial Dr.COM /d`→起 `gdut-net`→180s 内等 `Dial succeeded`→75s 内无 `dropped` 才算成功，任一步失败立即 `Stop-Service gdut-net`→`Start-Process DrMain.exe` 回滚旧模式；`install 816`、`service 1073` 已容错，`service_run` 失败 `exit(1)` 触发 SCM 重启。

## 命令

```bash
cargo test                                        # 纯逻辑（Linux 可跑，43+ 用例）
cargo test --test heartbeat_spec -v               # 单集成测试
cargo clippy -- -D warnings                       # Linux 侧
cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings  # 必跑：Win32 胶水
cargo fmt --check && cargo fmt
cargo check --target x86_64-pc-windows-msvc       # Linux 上验证 Windows 代码（本机 stable 1.98.0，需该 target 已安装）
cargo build --release                             # Windows 上产出 gdut-net.exe
```

CI：`linux-test`（test/clippy/fmt）+ `windows-build`（test --release/clippy --all-targets/build --release→`gdut-net-x86_64.zip`），`push→main` 与 PR 触发。

本地交叉构建（比等 CI 快）：`mise exec -- cargo xwin build --target x86_64-pc-windows-msvc --release`（需 `cargo-xwin`）。

## 陷阱

- `RasEnumConnectionsW` 缓冲元素 `dwSize` 必须预置 `sizeof(RASCONNW)`，否则 `632 ERROR_INVALID_SIZE`；`pbk` 比较需大小写不敏感（`pbk_eq_ci`）。
- `RasSetEntryPropertiesW 816`（端口占用）视为成功（端口释放后可拨），勿当硬错。
- `*>&1 | Out-File` 会把 `log::info!` 的英文 `WARN` 当 `NativeCommandError`；`switch-v4.ps1` 改用 `cmd /c "type pw.txt | exe install ... >> log 2>&1"`。
- `switch-v4.ps1` 成功检测搜英文 `Dial succeeded` / `dropped`，中文匹配永不命中（英文化后遗留坑）。
- `service` 停止后进程可能残留致重装 `1073`，`service_run` 显式 `std::process::exit` 兜底；`install` 已幂等（`1073` → `change_config`）。
- `probe` 的 `http_probe_url` 仅接受 `http://`+IPv4 字面量（`parse_http_probe_target` 单实现复用），`9.9.9.9` 被校园网墙，默认 `223.5.5.5`；`gateway 0.0.0.0` 时 ICMP 退化为 `223.5.5.5`。
- 校园网 DHCP 口与 PPP 口隔离：物理口默认路由 metric 0 优先于 PPP（metric 1），TUN 类软件 `auto-detect-interface` 会把代理出站绑到物理口被墙——须指定 `interface-name=gdut` 走 PPP，且 TUN MTU≤1400（PPPoE 1480 减开销），否则 WSL（mirror 模式跟随主机路由）开 TUN 即断网，家中单出口无此问题。
- `UAC ConsentPromptBehaviorAdmin=0 + EnableLUA=1` 会让 `Start-Process -Verb RunAs` 静默失败；免 UAC 一键切换靠计划任务 `gdut-switch`（`SYSTEM`，`AllowStartIfOnBatteries`，10min 超时），触发 `schtasks /Run /TN gdut-switch`。
- WSL 里 `powershell.exe` 不在 PATH，调 Windows 侧走绝对路径：WinPS 5.1 用 `/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe`，ps7（应用商店版）在 `/mnt/c/Users/Lemonawa/AppData/Local/Microsoft/WindowsApps/pwsh.exe`；读注册表/查自启动只读操作可直接跑。
- `ProxyEnable` 存 HKCU 重启不清零；重启后代理是否翻回只看自启动项（`HKCU/HKLM\...\Run` 应无 FlClash/Verge/clash 系），验证一行：`(Get-ItemProperty 'HKCU:\...\Internet Settings').ProxyEnable` 为 0 即关着；回写是事件驱动（FlClash HelperService 定时写回，GUI 开关脱节），高频轮询定格翻转时刻再对日志找触发动作。
- 命名管道默认 DACL 会拒绝用户会话：服务（SYSTEM/Session 0）建管必须挂 SDDL（`D:(A;;GRGW;;;AU)`，经 `create_with_security_attributes_raw`），否则托盘/`status` 报 `os error 5` 拒绝访问；改动只在服务重启后生效。
- 桌面 `gdut-net` 文件夹只留：`gdut-net.exe`、`gdut-net-bak.exe`（回滚件）、`switch-v4.ps1`+`switch-v4.log`、`一键切换.bat`、`tray.bat`、`status.bat`、`proxy-check.bat`；`switch-v4.ps1` A0 只从 `gdut-net-new.exe` 部署（旧 zip 流已退役，其构建早于管道 DACL/MessageBox 修复）；`pw.txt` 用后即删（`switch-v4` 下次运行会重建），明文密码不落盘。
- 给用户的断网窗口操作必须自带回滚块（能独立执行、不写“报给我”），网络炸了用户侧无 AI 可达。
