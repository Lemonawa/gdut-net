# 桌面工具包（Desktop Kit）

`C:\Users\Lemonawa\Desktop\gdut-net\` 是这台机器的操作入口：exe、切换/回滚脚本、快捷 bat 全在这里。保持精简——只留下面这些文件，其余（尤其诊断脚本）用完即删。

## 文件清单

| 文件 | 用途 |
|---|---|
| `gdut-net.exe` | 当前版本（无线接管 + 托盘/egui 面板 + 管道 DACL + 无控制台黑框） |
| `gdut-net-bak.exe` | 回滚件（2026-09-03 稳定版，无无线功能）。出问题时换回去就能活，别删 |
| `gdut-net-new.exe` | （按需出现）下次要部署的新版。`switch-v4.ps1` 的 A0 步会自动把它扶正 |
| `rollback.bat` / `rollback-v4.ps1` | 一键换回**旧版 gdut-net**（不是 Dr.COM）+ 重起服务。switch 失败后的第二道防线 |
| `switch-v4.ps1` / `switch-v4.log` | gdut-net ↔ Dr.COM 完整切换（全自动+失败自回滚）。详见 AGENTS.md |
| `一键切换.bat` | 触发计划任务 `gdut-switch` 跑 `switch-v4.ps1`（免 UAC），随后 tail 日志 |
| `一键回校.bat` | （管理员）服务设自动+启动，等 `Dial succeeded`，30s 稳定检查；失败自动回滚到停止+手动 |
| `一键回家.bat` | （管理员）在家用：停服务+设手动+杀托盘，防无效重拨和 toast 轰炸；不碰代理 |
| `tray.bat` | 起托盘（模式切换/Redial/Details 面板） |
| `status.bat` | 打印一次服务状态（含 Mode/Wireless/Events） |
| `proxy-check.bat` | 打印系统代理状态，期望 `ProxyEnable=0` |
| `tun-watch.ps1` / `tun-watch.log` | TUN 断网飞行记录仪（只读，15s 一拍记录代理/TUN/路由/DNS/网页状态） |
| `说明.txt` | 给桌面用户的速查说明（中文，文件阅读无乱码问题） |

所有 `.bat` 内容保持纯英文（中文控制台 GBK 会乱码），文件名中文无妨。

## 场景

- **在校开机**：服务自启拨号，托盘自启（`HKCU\...\Run\gdut-net-tray`）。`status.bat` 确认 Connected。
- **离校回家**：双击 `一键回家.bat`（管理员）。回家后普通网络即用，gdut-net 静默。
- **返校**：插上网线，双击 `一键回校.bat`（管理员）。拨号失败会自动停服务并告诉你下一步（跑 `一键切换.bat` 走完整流程）。
- **拔线改无线**：服务自动接管（默认 exclusive）；托盘/面板可切 "Wired + wireless standby" 常备无缝。拔线期间程序不会拨号（link gate 防端口卡死），插回网线 1~2 秒自动拨上。
- **现场排障**：管理员 `.\gdut-net.exe wireless test` 打一次真实 portal 回包（自回滚，不留状态）。
- **网络炸了且 AI 不可达**：`switch-v4.log` 尾巴 → `rollback.bat`（回旧版 gdut-net）→ 最后 Dr.COM（`C:\Drcom\DrUpdateClient\DrMain.exe` 或 `rasdial 'Dr.COM'`）。

## 代理 / TUN 铁律（校园网双出口）

1. 查代理只信注册表（`proxy-check.bat`），不信任何 GUI 开关。
2. FlClash 的 HelperService 会把代理写回 1——不要开 FlClash。
3. **不要给 Mihomo 设 `interface-name`**（2026-09-10 实证）：无线接管期间该接口不存在，mihomo 每个出站都硬错 `interface not found`，无回退 → Clash 全 timeout。mihomo 的 auto-detect 按"非虚拟 up 接口中总 metric 最低者"选路，本机有线自动选 `gdut`(PPP)、无线自动选 `WLAN`，四组合（有/无线 × TUN 开/关）均实测正确。TUN MTU≤1400。
4. Verge 配置注入点是 `profiles/Merge.yaml`，别手改生成的 `clash-verge.yaml`；改完必须**完整退出并重启 Verge 进程**才重新合并。
5. WSL 是 mirror 模式跟主机路由；直连走 TUN 即可（fake-ip 已退役为 redir-host）。

## 部署新版

1. 把新 `gdut-net.exe` 放到本文件夹，改名 `gdut-net-new.exe`。
2. 下次跑 `switch-v4.ps1`（或 `一键切换.bat`）时 A0 步自动部署；或手动：停服务→备份旧 exe→Move 覆盖→起服务→等 `Dial succeeded`。
3. 部署失败：`rollback.bat`（回旧版）或把 `gdut-net-bak.exe` 移回去覆盖，起服务。
