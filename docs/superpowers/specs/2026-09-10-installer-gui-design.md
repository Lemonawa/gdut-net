# 安装器与日常 GUI（Installer & Tray GUI）设计

日期：2026-09-10
状态：已获口头批准，待 spec 评审
关联：ADR-0001（服务/托盘分离）、ADR-0004 / ADR-0006（托盘 egui）、ADR-0007（本设计定案后记录）

## 1. 目标与非目标

**目标**：

- 陌生学生无需命令行即可安装：双击 `gdut-net-setup.exe` → 输入学号密码 → 完成。
- 日常使用有正式 GUI：左键托盘打开窗口式控制台（中文）——状态、立即重拨、模式切换、最近事件、修改账号密码、打开日志目录。
- 产品安装进 `C:\Program Files\gdut-net\`，有开始菜单文件夹（含"卸载"入口），注册"应用和功能"。
- 只发布 `gdut-net-setup.exe` 一个文件（主程序内嵌；主程序保持 portable：拷走能跑、不联网、不下载）。
- 作者本机从桌面工具包迁移到安装形态，含个人运维脚本搬家与计划任务改指。

**非目标**：

- 不做自动更新（新版本 = 下载新 setup 重跑，保留密码）。
- 不引入 WebView2 / Tauri；GUI 继续 egui（glow + default_fonts，ADR-0006）。
- 不做便携/无服务模式；不做多语言（GUI 中文，控制台英文）。
- 不改拨号 / 探测 / 无线核心逻辑；不改 CLI 既有行为与输出语言。
- 不内嵌图标资源（保持代码生成风格）；主程序与安装器用默认 exe 图标。

## 2. 背景事实（2026-09-10 查证）

- `src/main.rs` 的 release 构建已是 `windows_subsystem = "windows"`：开机托盘无黑窗（原设想的子系统切换作废，不改）。
- 本机耦合点（迁移必须改）：服务 PathName = `C:\Users\Lemonawa\Desktop\gdut-net\gdut-net.exe`；HKCU Run `gdut-net-tray` 同指桌面目录；计划任务 `gdut-switch` 跑桌面 `switch-v4.ps1`。
- `switch-v4.ps1` 第 61 行明文写死校园网密码，配合 `pw.txt` 管道安装；迁移必须改为 `--keep-password`——复用已存 DPAPI 密文（`crypto::unprotect` 已存在）重新 `set_credentials`，全程不碰明文。
- `wireless-test.bat` 注释证实：GUI 子系统 exe 在裸 cmd 下无输出、PowerShell 管道捕获可用——控制台相关行为改动必须谨慎，验收覆盖。
- egui 0.36 默认字体无 CJK 字形；GUI 必须加载系统字体（msyh.ttc 等），否则中文全变方块。
- 桌面工具包个人脚本清单（现状）：`switch-v4.ps1`、`rollback.bat`、`rollback-v4.ps1`、`一键切换.bat`、`tun-watch.ps1`、`gdut-net-bak.exe`、`说明.txt`。

## 3. 发布物与构建

- 新增两个 bin：
  - `gdut-net-setup`（`src/bin/gdut-net-setup.rs`，windows 子系统、自提权）：安装 / 维护 GUI，仅 Windows 目标有意义（非 Windows 编译为明确报错的空壳）。
  - `gdut-net-pack`（host bin）：把 payload 目录追加到 `gdut-net-setup.exe` 尾部，产出单文件发布物；本机（Linux，`cargo xwin` 构建后）与 CI（windows-latest）都能跑，只做文件 IO。
- payload 容器（纯逻辑 `src/payload.rs`，Linux 可测）：
  - 尾部 footer：magic `GDUTPAK1` + u32 版本 + u64 TOC 偏移（定长，从文件尾读）。
  - TOC：条目数 + 每项 {文件名（UTF-8，禁止路径分隔符与 `..`）、偏移、长度、sha256}。
  - 安装器读 `current_exe()` 尾部解析；截断 / 坏 magic / 校验失败 = 明确报错，拒绝部分安装。
  - 开发与测试：setup 未打包时读自身旁边 `payload/` 目录（同一文件清单），不依赖容器。
- 发布：`.github/workflows/release.yml` 产出 `gdut-net-setup.exe` 单资产，替换现在的主程序 zip。
- `gdut-net.exe` 仍随安装包落盘（portable 本体），但不再是发布入口。

## 4. 安装布局

```
C:\Program Files\gdut-net\
  gdut-net.exe            主程序（服务 + 托盘 + 日常 GUI + CLI）
  gdut-net-setup.exe      安装器副本（日后修复 / 卸载入口）
  *.bat                   产品脚本 + 个人运维脚本（纯英文、位置无关）
  gdut-net-bak.exe        回滚件
  说明.txt                中文速查
C:\ProgramData\gdut-net\  config.toml、gdut.pbk、logs\（不变）
```

- 开始菜单：`%ProgramData%\Microsoft\Windows\Start Menu\Programs\GDUT Net\`（所有用户），10 项：GDUT Net（日常界面）、状态查看、回校模式、回家模式、启动托盘、无线体检、打开日志、代理检查、说明、卸载 GDUT Net。
- 管理员类快捷方式带"以管理员身份运行"标志（IShellLinkDataList `SLDF_RUNAS_USER`，盾牌图标）。
- "应用和功能"：`HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\gdut-net`，`UninstallString` 指向 `gdut-net-setup.exe --uninstall`。
- 个人运维脚本不进入公开发布 payload；本机迁移时复制进安装目录并改写绝对路径。

## 5. 安装器（gdut-net-setup.exe）

- 入口 → `gdut_net::setup::main()`；非管理员 → `ShellExecuteW "runas"` 以同参数自提权重启；用户拒绝 UAC → 中文提示后退出。
- 参数：无参（未安装进向导 / 已安装进维护页）、`--repair`（维护页）、`--uninstall`（维护页定位卸载）、`--silent [--keep-password]`（无 UI、英文输出，供迁移脚本与高级用户，走同一核心）。
- 向导 4 屏（中文）：
  1. **欢迎**：一句话产品说明 + "不影响代理 / VPN / 其它网卡"承诺。
  2. **账号**：学号（已有配置则预填）+ 密码（掩码）；检测到有效密文时提供"使用现有密码"（默认选中，零输入）。
  3. **进度**：停旧服务 / 托盘 → 解包文件 → `install_core`（配置 / pbk / 服务注册 / 托盘自启指向安装目录 / 事件源）→ 开始菜单 + 卸载项 → 启动服务。每步可见；任何失败走回滚（§9）。
  4. **完成**：尝试连 IPC ≤20s 显示实时状态；失败则给原因、日志入口、重试按钮；默认勾选"打开日常界面"。
- **维护页**：修复（可"保留现有密码"或换新密码；重解包 + 重注册）、卸载（保留配置 / 彻底清除两档）。
- 中文字体：启动加载 `C:\Windows\Fonts\msyh.ttc`（回退 `simhei.ttf`、`simsun.ttc`）注入 egui 字体表；找不到则错误页（不静默方块化）。
- 与 CLI 的共同核心（`service.rs` 重构）：
  - `install_core(InstallRequest { cfg_path, student_id, credential: Plain(..) | KeepExisting, service_exe, tray_exe }) -> InstallOutcome`（显式接收 exe 路径：从 setup 拉起时 `current_exe()` 是 setup 自己）。
  - `uninstall_core(cfg_path, purge)`：含开始菜单 / 卸载项 / 安装目录清理；目录被占用时用延迟删除助手（临时 cmd 等待后删目录并自删）。
  - `install_state() -> NotInstalled | Installed { version, service_exe }`。
  - CLI `install` / `uninstall` 变薄壳：提示语、英文输出、`--password-stdin` 管道行为、退出码全部不变；新增 `install --keep-password`。
  - `tray::register_autostart(exe_path)` 参数化（自启注册的必须是安装目录的 `gdut-net.exe`，不是 setup）。

## 6. 日常 GUI（托盘窗口）

- **生命周期**：托盘进程常驻一扇 egui 窗口。首次打开创建；关窗 = 隐藏（`close_requested` → `ViewportCommand::Visible(false)`），不退出进程；左键托盘 = 显示并聚焦。右键菜单保留：打开主界面 / 模式两项 / 立即重拨 / 退出托盘。
- **单实例**：命名互斥体。已在运行时，再启动（双击 exe、点开始菜单）经 `\\.\pipe\gdut-net-tray-show` 发单字节"显示"信号后退出，不产生第二个托盘图标。无参数双击且未安装 → 中文提示引导去 `gdut-net-setup.exe`；无参数且从控制台启动 → 维持现状（clap 行为），以 `GetConsoleWindow()` 是否为 NULL 区分。
- **单页信息架构**（中文）：
  - 状态卡：状态词（已连接 / 重拨中 / 认证失败 / 无线接管中 / 服务未运行）+ 出口（有线 / 无线）+ IP + 在线时长 + 上次掉线原因 + 心跳状态；语义配色沿用托盘图标四色。
  - 操作：立即重拨；模式二选一（有线优先自动接管 / 有线+无线备用）。
  - 账号卡：学号 + "修改账号密码"（ShellExecute 唤起安装目录的 setup 维护页，UAC 由 setup 承担）。
  - 最近事件：滚动时间线（截断上限）。
  - 底部：打开日志目录、版本号。
  - 服务未运行：整页切换为"服务未运行"+ 启动按钮，不显示假数据。
- **IPC**：沿用现有模式——GUI 经 mpsc 把命令交泵线程统一发送；快照经 `SharedSnapshot` 每帧拉取。

## 7. 脚本、开始菜单与运维

- 产品脚本（进 payload，纯英文、位置无关 `%~dp0`）：`status.bat`、`campus.bat`、`home.bat`、`tray.bat`、`wireless-test.bat`、`open-logs.bat`、`proxy-check.bat`（中文名 bat 退役）。`说明.txt`（中文快查）同属产品 payload。
- 个人运维脚本（迁入安装目录，不进发布）：`switch-v4.ps1`（改写：去掉明文密码，A0 步改 `install --keep-password`；部署新版 = 跑新 setup）、`rollback.bat` / `rollback-v4.ps1`、`一键切换.bat`、`tun-watch.ps1`、`gdut-net-bak.exe`。
- 计划任务 `gdut-switch` 改指新路径。
- `说明.txt` 按新布局重写；`docs/desktop-kit.md` 同步重写。

## 8. 作者本机迁移（本次实现内执行）

1. 构建 + 打包新 setup（本机 `cargo xwin`）。
2. 提权迁移脚本（桌面 `迁移.bat` 自提权，双击触发一次 UAC；脚本自包含、幂等、带失败回滚）：
   停服务 / 杀托盘 → 建安装目录、拷贝文件（新 exe、脚本、bak、说明）→ 改写个人脚本绝对路径 → 更新 Run / 计划任务 / 开始菜单 / 卸载项 → `gdut-net-setup.exe --silent --keep-password` → 起服务、等拨号 → 写迁移日志。
3. 验证：`status`、托盘与 GUI、跑一次切换脚本（拨号 + 75s 稳定检查）。
4. 桌面目录保留观察；用户确认稳定后再删。

## 9. 错误处理与回滚

- 安装失败：绝不留下"服务指向半拷贝目录"的状态。顺序固定为：停旧服务 → 写文件 → 注册服务；注册失败则恢复旧 exe 路径注册（本次新建则删服务）；UI 给原因 + 日志 + 重试。
- setup 全程英文日志写 `%ProgramData%\gdut-net\logs\setup.log`（UI 中文、日志英文）。
- 卸载：幂等宽容（沿用 CLI 语义）；开始菜单 / 注册表 / 目录清理互不依赖，单步失败不阻断其它步。
- 托盘：单实例冲突 / GUI 崩溃不影响服务；GUI 崩溃只记日志。
- 网络变更仍全部由服务承担并有自动回滚（既有铁律），GUI 不新增网络变更路径。

## 10. 测试策略

- Linux 纯逻辑单测：payload 容器（round-trip、截断、坏 magic、空 payload、非法文件名）、快捷方式清单与 payload 一致性（每个目标文件存在、文件名 ASCII）、脚本文本断言（无 `pw.txt`、无明文密码模式、含 `--keep-password`）。
- `cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings` 必过。
- 真机验收新增：零键盘安装；修复（保留 / 改密）；卸载两档与残留检查；十个开始菜单项逐一点开；开机（无窗 + 服务 + 托盘 + GUI）；管道安装 `--password-stdin` 不回归；`install --keep-password`；单实例双击唤出；GUI 中文无方块；关窗 = 隐藏。
- 视觉验证：真机截图（PowerShell `CopyFromScreen` → PNG → 审阅），默认尺寸与自定义尺寸两档。

## 11. 文档与视觉

- 视觉方向：实现前用 impeccable 决策页选定（安装器 + 日常 GUI 共用一个视觉世界；决策页在浏览器由用户挑卡）；动 UI 前读 craft-floor；完成时出 DESIGN.md（含 `.impeccable/design.json`）与 finish review。
- 文档：`README.md`（安装 / 卸载改为 setup 流程）、`AGENTS.md`（新 bin、命令）、`CONTEXT.md`（语言规则改为"控制台英文，GUI 中文"；桌面工具包章节改安装布局）、`docs/desktop-kit.md` 重写、`docs/acceptance.md` 新增验收、ADR-0007。
- `PRODUCT.md`：更新发布渠道、语言规则、安装形态（platform 保持 `windows-desktop`，如实反映 Windows 原生）。

## 12. 不可变项（兼容性清单）

- CLI 子命令与行为：`run` / `install`（含 `--password-stdin`）/ `uninstall [--purge]` / `status` / `tray` / `wireless test|off|standby`；输出语言英文；退出码语义不变；新增 `install --keep-password`。
- 命名管道协议与 DACL（`D:(A;;GRGW;;;AU)`）、服务名 `gdut-net`、`config.toml` 结构（新增字段必须 `serde(default)`）、pbk 路径与内容语义。
- 服务仍以 SYSTEM / Session 0 运行；托盘仍为用户会话进程，可独立退出。
