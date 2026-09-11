# ADR-0007：安装器（单文件 setup）+ 中文日常 GUI，以及发布/语言策略

日期：2026-09-11
状态：已接受（2026-09-11 本机迁移完成并核验）

## 背景

到 2026-09-10，产品只有 CLI 安装路径（管理员 PowerShell 跑 `gdut-net.exe install`），发布物是 `gdut-net-x86_64.zip`；托盘只有英文原生菜单 + 一个告警用的面板。用户（陌生 GDUT 学生）需要：不用命令行就能装；日常有一个正式的中文界面；有开始菜单/卸载入口；一个文件下载即用。作者本机则把 `C:\Users\Lemonawa\Desktop\gdut-net\` 桌面工具包当部署入口（服务 PathName、HKCU Run、计划任务全指桌面路径），迁移必须一并解决。

既有约束：egui 0.36 默认字体无 CJK 字形（ADR-0006 的教训）；中文 Windows 控制台 GBK 乱码，因此"一切面向用户输出英文"是旧规则；`switch-v4.ps1` 内嵌明文密码配合 `pw.txt` 管道安装；主程序 `gdut-net.exe` 必须保持 portable（拷贝能跑、不联网、不下载）。spec：`docs/superpowers/specs/2026-09-10-installer-gui-design.md`。

## 决策

1. **单文件发布物**：新增 `gdut-net-setup`（Windows 自提权安装/维护 GUI）与 `gdut-net-pack`（host 打包工具）。`gdut-net-pack` 把 `gdut-net.exe` 与 `packaging/payload/` 的脚本追加到 setup 尾部，产出唯一资产 `gdut-net-setup.exe`；容器为自定义格式（`src/payload.rs`）：`[setup][files][TOC][24B footer]`，footer magic `GDUTPAK1` + u32 版本 + u32 条目数 + u64 TOC 偏移，TOC 每项 {文件名, offset, len, sha256}。截断/坏 magic/校验失败 = 明确报错拒绝安装。开发态未打包的 setup 回退读自身旁边 `payload/` 目录。CI/release 从 zip 改为单文件。
2. **安装布局**：`C:\Program Files\gdut-net\`（主程序、setup 副本、产品脚本、`说明.txt`）；`C:\ProgramData\gdut-net\` 不变（config/pbk/logs）。开始菜单 `%ProgramData%\Microsoft\Windows\Start Menu\Programs\GDUT Net\` 10 项（管理员项带 `SLDF_RUNAS_USER` 盾牌）；"应用和功能"键 `HKLM\...\Uninstall\gdut-net`（`UninstallString` → 安装目录 setup `--uninstall`）。安装/卸载幂等，失败走回滚（绝不留下服务指向半拷贝目录的状态）。
3. **安装器流程**：自提权（`ShellExecuteExW "runas"`，拒绝 UAC 有中文提示）；向导 4 屏（欢迎/账号/进度/完成）；维护页支持修复（默认"使用现有密码"，可改密）与卸载两档（保留/彻底删除配置与日志）；`--silent [--keep-password]` 无界面模式，英文输出、供脚本/高级用户，安装核心与 CLI 共用（`install_core`/`uninstall_core`）。CLI `install`/`uninstall`/`status`/`tray`/`wireless` 的行为、英文输出与退出码不变；`install` 仅新增 `--keep-password`。
4. **日常 GUI 与单实例**：托盘进程常驻一扇中文 egui 窗口（状态卡/立即重拨/模式切换/修改账号密码/最近事件/打开日志）。**左键**托盘打开窗口，**右键**原生中文菜单；关窗 = 隐藏（`ViewportCommand::Visible(false)`），进程不退。单实例用命名 mutex `gdut-net-tray-singleton` + 命名事件 `gdut-net-tray-show`：二次启动只唤出已有窗口后退出，绝不出现第二个图标。开窗期间仍经 IPC 拉 `SharedSnapshot`，GUI 不新增任何网络变更路径。
5. **语言策略（规则变更）**：**GUI 中文，控制台英文**。GUI（安装器向导/维护页、日常窗口、托盘菜单、`说明.txt`）面向学生，经系统字体（`msyh.ttc`，回退 `simhei.ttf`/`simsun.ttc`）渲染没有 GBK 问题；CLI/日志/脚本/`.bat`/`.ps1` 回显继续英文（中文 Windows 控制台 GBK 会乱码）。egui 找不到 CJK 字体时显示错误页，不静默方块化。
6. **个人脚本与产品脚本同居安装目录**：`switch-v4.ps1`、`rollback.bat`、`rollback-v4.ps1`、`一键切换.bat`、`tun-watch.ps1`、`gdut-net-bak.exe` 迁入 `C:\Program Files\gdut-net\`（不进公开发布 payload）。`switch-v4.ps1` 去掉明文密码，迁移/安装改走 `gdut-net-setup.exe --silent --keep-password`（复用已存 DPAPI 密文重新 `set_credentials`）。
7. **本机迁移**：走既有预授权计划任务 `gdut-switch`（免 UAC）：停服务/杀托盘 → silent 安装 → 拷个人脚本 → 任务改指安装目录 → 等拨号成功 + 75s 稳定检查 → 失败自动回滚桌面 exe。桌面工具包保留作 fallback。

## 后果

- 陌生学生零键盘安装；卸载有两个明确的清除档位；修复/改密 = 重跑安装器。
- 发布物约 17.5MB 单文件（主程序 + 8 个 payload 文件）；无自动更新——新版本 = 下载新 setup 重跑，默认保留密码。
- 作者本机 2026-09-11 迁移完成并核验：服务 PathName、HKCU Run 托盘自启、`gdut-switch` 任务均指向 `C:\Program Files\gdut-net`；开始菜单 10 项；Apps & Features 键在；dial 2s 成功 + 75s 稳定。
- setup/托盘是 GUI 子系统（无 stderr）：silent 输出必须经 PowerShell 管道捕获；新增文件日志 `tray_r*.log`/`setup_r*.log`（ProgramData logs）。
- 语言规则文档化进 `CONTEXT.md`/`AGENTS.md`/`PRODUCT.md`；真机验收新增安装器/GUI 条目（`docs/acceptance.md` 13–24）。
- 安装目录删除需延迟重试助手（cmd 循环 + `DETACHED_PROCESS`）以避开窗口/快捷方式占用。
