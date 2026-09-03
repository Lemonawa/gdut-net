# 桌面工具包（Desktop Kit）

`C:\Users\Lemonawa\Desktop\gdut-net\` 是这台机器的操作入口：exe、切换脚本、快捷 bat 全在这里。保持精简——只留下面这些文件，其余一律删除。

## 文件清单

| 文件 | 用途 |
|---|---|
| `gdut-net.exe` | 当前版本（管道 DACL + MessageBox 面板 + 无控制台黑框） |
| `gdut-net-bak.exe` | 回滚件。出问题时换回去就能活，别删 |
| `gdut-net-new.exe` | （按需出现）下次要部署的新版。`switch-v4.ps1` 的 A0 步会自动把它扶正 |
| `switch-v4.ps1` / `switch-v4.log` | 唯一的模式切换入口（gdut-net ↔ Dr.COM），全自动+失败自回滚。详见 AGENTS.md |
| `一键切换.bat` | 触发计划任务 `gdut-switch` 跑 `switch-v4.ps1`（免 UAC），随后 tail 日志 |
| `一键回校.bat` | （管理员）服务设自动+启动，等 `Dial succeeded`，30s 稳定检查；失败自动回滚到停止+手动 |
| `一键回家.bat` | （管理员）在家用：停服务+设手动+杀托盘，防无效重拨和 toast 轰炸；不碰代理 |
| `tray.bat` | 起托盘（读服务状态，右键 Details 看详情） |
| `status.bat` | 打印一次服务状态 |
| `proxy-check.bat` | 打印系统代理状态，期望 `ProxyEnable=0` |

所有 `.bat` 内容保持纯英文（中文控制台 GBK 会乱码），文件名中文无妨。

## 场景

- **在校开机**：服务自启拨号，托盘自启（`HKCU\...\Run\gdut-net-tray`，新版无黑框）。`status.bat` 确认 Connected。
- **离校回家**：双击 `一键回家.bat`（管理员）。回家后普通网络即用，gdut-net 静默。
- **返校**：插上网线，双击 `一键回校.bat`（管理员）。拨号失败会自动停服务并告诉你下一步（跑 `一键切换.bat` 走完整流程）。
- **网络炸了且 AI 不可达**：每个动网络的操作都自带回滚块，直接照着跑，不用等我。回滚件 `gdut-net-bak.exe` + Dr.COM（`rasdial 'Dr.COM'`）是最后两道防线。

## 代理 / TUN 铁律（校园网双出口）

1. 查代理只信注册表（`proxy-check.bat`），不信任何 GUI 开关。
2. FlClash 的 HelperService 会把代理写回 1——不要开 FlClash。
3. TUN/代理出站必须走 PPP 口：Mihomo `interface-name: gdut`，TUN MTU≤1400。Verge 里改注入点 `profiles/Merge.yaml`，别手改生成的 `clash-verge.yaml`。
4. WSL 是 mirror 模式跟主机路由；TUN 开 fake-ip 时直连失败是预期，走 TUN/代理即可。

## 部署新版

1. 把新 `gdut-net.exe` 放到本文件夹，改名 `gdut-net-new.exe`。
2. 下次跑 `switch-v4.ps1`（或 `一键切换.bat`）时 A0 步自动部署；或手动：停服务→备份旧 exe→Move 覆盖→起服务→等 `Dial succeeded`。
3. 部署失败：把 `gdut-net-bak.exe` 移回去覆盖，起服务。
