# 桌面运维（Desktop Operations，安装形态）

2026-09-11 起，本机运行的是**安装形态**：程序与脚本都在 `C:\Program Files\gdut-net\`，配置/日志在 `C:\ProgramData\gdut-net\`，日常入口在开始菜单 **GDUT Net** 文件夹。桌面文件夹 `C:\Users\Lemonawa\Desktop\gdut-net\` 只作 fallback 保留（确认长期稳定后可删）。

个人运维脚本（不进公开发布 payload）与产品脚本同居安装目录——这是有意为之：回滚链在断网时无 AI 可达，脚本必须和 exe 在一起、位置无关。

## 开始菜单入口（10 项）

| 项 | 用途 |
|---|---|
| GDUT Net | 打开中文日常界面（左键托盘图标同效） |
| 状态查看 | 命令行看一次状态（`status.bat`）；`Connected` + IP 即在线 |
| 回校模式（管理员） | 服务设自动+启动，等拨号成功 + 30s 稳定检查；失败自动停服务并提示 |
| 回家模式（管理员） | 停服务+设手动，防无效重拨与 toast；不碰代理 |
| 启动托盘 | 托盘被杀后重新拉起 |
| 无线体检（管理员） | 一次性校园 WiFi 认证实测（`wireless test`，约 40s，自断开不留状态） |
| 打开日志 | 打开 `C:\ProgramData\gdut-net\logs\` |
| 代理检查 | 打印系统代理，期望 `ProxyEnable=0` |
| 说明 | 打开 `说明.txt`（中文速查，装机自带） |
| 卸载 GDUT Net（管理员） | `gdut-net-setup.exe --uninstall`，维护页可选"同时删除配置与日志" |

管理员类快捷方式带盾牌（`SLDF_RUNAS_USER`），点开由 Shell 弹 UAC。

## 安装目录文件清单

`C:\Program Files\gdut-net\`

| 文件 | 用途 |
|---|---|
| `gdut-net.exe` | 主程序（服务 + 托盘/日常 GUI + CLI），portable 本体 |
| `gdut-net-setup.exe` | 安装器副本（修复/卸载/改密入口；即发布物） |
| `status.bat` / `campus.bat` / `home.bat` / `tray.bat` / `wireless-test.bat` / `open-logs.bat` / `proxy-check.bat` | 产品脚本（payload，纯英文、`%~dp0` 位置无关） |
| `说明.txt` | 中文速查（装机自带） |
| `switch-v4.ps1` | **个人**：gdut-net ↔ Dr.COM 完整切换（全自动+失败自回滚）；日志写 `C:\ProgramData\gdut-net\logs\switch-v4.log` |
| `rollback.bat` / `rollback-v4.ps1` | **个人**：一键换回旧版 gdut-net（不是 Dr.COM）+ 重起服务；switch 失败后的第二道防线 |
| `一键切换.bat` | **个人**：触发计划任务 `gdut-switch` 跑 `switch-v4.ps1`（免 UAC），随后 tail 日志；**脚本会弹第二个窗口，别关它**（关=杀脚本，退出码 `0xC000013A`，换装通常已完成但收尾检查缺失） |
| `tun-watch.ps1` / `tun-watch.log` | **个人**：TUN 断网飞行记录仪（只读，15s 一拍记录代理/TUN/路由/DNS/网页状态） |
| `gdut-net-bak.exe` | **个人**：回滚件（2026-09-03 稳定版，无无线功能）。出问题换回去就能活，别删 |

所有 `.bat` 内容保持纯英文（中文控制台 GBK 会乱码），文件名中文无妨。

## 日常场景

- **在校开机**：服务自启拨号，托盘自启（`HKCU\...\Run\gdut-net-tray` 指向安装目录）。开始菜单"状态查看"确认 `Connected`。
- **离校回家**：开始菜单"回家模式"（管理员）。回家后普通网络即用，gdut-net 静默。
- **返校**：插上网线，开始菜单"回校模式"（管理员）。失败会自动停服务并提示下一步。
- **拔线改无线**：服务自动接管（默认"有线优先自动接管"）；日常界面可切"有线+无线备用"常备无缝。拔线期间程序不会拨号（link gate 防端口卡死），插回网线 1~2 秒自动拨上。
- **改账号密码 / 修复安装**：打开"GDUT Net" → 修改账号密码；或直接重跑 `gdut-net-setup.exe`（维护页，默认保留现有密码，也可换新）；或开始菜单安装器。
- **现场排障**：管理员 `.\gdut-net.exe wireless test` 打一次真实 portal 回包（自回滚，不留状态）；日志看 `C:\ProgramData\gdut-net\logs\`（服务 `gdut-net_r*.log`、托盘 `tray_r*.log`、安装器 `setup_r*.log`）。
- **网络炸了且 AI 不可达**：`switch-v4.log` 尾巴 → `rollback.bat`（回旧版 gdut-net）→ 最后 Dr.COM（`C:\Drcom\DrUpdateClient\DrMain.exe` 或 `rasdial 'Dr.COM'`）。

## 迁移（2026-09-11 已完成）

`switch-v4.ps1` 现在承担"迁移 + 切换"：停服务/杀托盘 → `gdut-net-setup.exe --silent --keep-password`（落 Program Files、重注册服务/Run/开始菜单/卸载项，复用 DPAPI 密文，**不再有明文密码**）→ 拷个人脚本与 `gdut-net-bak.exe` → 计划任务 `gdut-switch` 改指安装目录 → 等拨号成功 + 75s 稳定检查 → 失败自动回滚到桌面 exe。

迁移通道：双击桌面 `一键切换.bat`（触发预授权计划任务 `gdut-switch`，免 UAC）。用户只需这一下。已核验：服务 PathName、HKCU Run、任务全指向 `C:\Program Files\gdut-net`；开始菜单 10 项；应用和功能条目在；dial 2 秒成功、75s 稳定。

## 部署新版

1. 新 `gdut-net-setup.exe` 放桌面工具包（`C:\Users\Lemonawa\Desktop\gdut-net\`）。
2. 双击 `一键切换.bat` → `switch-v4.ps1` 自动安装（`--silent --keep-password`）并验证；也可直接双击新 setup 走向导。
3. 失败：`rollback.bat`（回旧版 gdut-net）→ 把 `gdut-net-bak.exe` 移回覆盖 `gdut-net.exe`，起服务。

## 代理 / TUN 铁律（校园网双出口）

1. 查代理只信注册表（开始菜单"代理检查"），不信任何 GUI 开关。
2. FlClash 的 HelperService 会把代理写回 1——不要开 FlClash。
3. **不要给 Mihomo 设 `interface-name`**（2026-09-10 实证）：无线接管期间该接口不存在，mihomo 每个出站都硬错 `interface not found`，无回退 → Clash 全 timeout。mihomo 的 auto-detect 按"非虚拟 up 接口中总 metric 最低者"选路，本机有线自动选 `gdut`(PPP)、无线自动选 `WLAN`，四组合（有/无线 × TUN 开/关）均实测正确。TUN MTU≤1400。
4. Verge 配置注入点是 `profiles/Merge.yaml`，别手改生成的 `clash-verge.yaml`；改完必须**完整退出并重启 Verge 进程**才重新合并。
5. WSL 是 mirror 模式跟主机路由；直连走 TUN 即可（fake-ip 已退役为 redir-host）。
