# 真机验收清单

> 原 README 内附清单，随英文版重组迁出至此。安装形态（setup）时代新增条目见文末"安装器与日常 GUI"。

前提：Windows 11 x64 物理机，校园网有线接入。部署形态二选一：从 Release 下载单文件 `gdut-net-setup.exe` 双击安装（推荐），或管理员 PowerShell 跑 `.\gdut-net.exe install`（面向高级用户）。

## 部署与基础功能

```powershell
# 管理员 PowerShell，gdut-net.exe 所在目录
.\gdut-net.exe install            # 输入密码（学号为空时也会提示输入学号）
net start gdut-net
Get-Content C:\ProgramData\gdut-net\logs\*.log -Wait   # expect "Dial succeeded, session established"
net stop gdut-net                 # graceful stop, log shows "Stop signal received, hanging up and exiting"
net start gdut-net                # 再次启动自动重拨成功
```

## 验收标准 1-5 对照

| # | 标准 | 验证方法 |
|---|---|---|
| 1 | 全新 Win11：安装 → 自动拨号 → 断网自动重连 | 上面部署步骤；然后拔网线 10s 再插回（或 `rasdial gdut /disconnect`），日志应出现"判定掉线"→ 指数退避重拨 →"重拨成功，会话恢复"，全程无人工干预 |
| 2 | TUN/Tailscale 共存，重启后重拨成功 | 安装 Clash TUN（TUN 模式）与 Tailscale 并确认 `route print` 出现虚拟路由；`net stop gdut-net && net start gdut-net`，重拨应成功且不干扰 TUN 路由；重启整机后服务自启、会话自动恢复 |
| 3 | 内存 <50MB / 24h 无泄漏 | 任务管理器观察 `gdut-net.exe`（服务进程）工作集 <50MB；挂机 24h 后工作集无明显增长（±5MB 内） |
| 4 | 关心跳 72h 不掉线；开心跳抓包验证 | `heartbeat.enabled=false` 挂机 72h，日志无"判定掉线"（或仅极少数且自动恢复）；需要验证兼容模式时 `enabled=true`，Wireshark 在**物理网卡**抓 `udp.port==61440`，应看到 20s 周期 keepalive 且端口/报文与官方客户端一致 |
| 5 | 卸载干净 | `.\gdut-net.exe uninstall --purge` 后：`sc.exe query gdut-net` → 1060（不存在）；`reg query "HKLM\SYSTEM\CurrentControlSet\Services\EventLog\Application\gdut-net"` → 拒绝访问/不存在；`reg query "HKLM\SOFTWARE\gdut-net"` → 不存在；`C:\ProgramData\gdut-net` 已删除 |

## 服务与事件日志检查

```powershell
sc.exe qfailure gdut-net         # 3 段恢复：5000/30000/60000 ms，重置期 86400 秒
sc.exe qc gdut-net               # AUTO_START，binPath 指向 C:\Program Files\gdut-net\gdut-net.exe，含 --config 与 run
reg query "HKLM\SYSTEM\CurrentControlSet\Services\EventLog\Application\gdut-net"
                                 # EventMessageFile=%SystemRoot%\System32\netmsg.dll（REG_EXPAND_SZ）
                                 # TypesSupported=0x7
reg query "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\gdut-net"
                                 # DisplayName=GDUT Net；UninstallString/QuietUninstallString 指向安装目录 setup
```

`log.event_log = true` 时：事件查看器 → Windows 日志 → 应用程序，来源 `gdut-net`，warn/error 应同步出现（如"认证失败(691)"）。

## 托盘与状态（用户会话）

```powershell
.\gdut-net.exe tray      # 托盘图标按状态变色：绿=有线通 / 蓝=无线在用 / 黄=退避 / 灰=断
                         # 左键：打开中文日常窗口（关窗=隐藏）；右键原生中文菜单：状态行 / 有线优先（自动无线接管）/ 有线+无线备用 / 立即重拨 / 打开主界面 / 退出托盘
                         # 二次启动（双击 exe / 开始菜单）：只唤出已有窗口，单实例
.\gdut-net.exe status    # 终端打印：状态、在线时长、IP、掉线原因、重拨次数、心跳、Mode、Wireless、Events（英文）
```

## 无线接管（v0.3）

前置：`gdut-net.exe install` 过一次；WLAN profile `gdut` 存在（手工连过一次校园 WiFi）。

```powershell
.\gdut-net.exe wireless test    # 一次性现场验证：应打印 WLAN IP/gw、HTTP 200、RESULT: SUCCESS，结束自动断开
.\gdut-net.exe status           # Mode: ... / Wireless: ... 两行出现
```

| # | 标准 | 验证方法 |
|---|---|---|
| 6 | exclusive：拔线无线接管 | 日志依次出现 `Ethernet link down, dial paused` → `Wireless: associating` → `Wireless: portal login success`（实测 ≈13s）；期间**不得出现** `Dial failed`（link gate 生效，拔线不碰 PPPoE 端口） |
| 7 | 插回有线立即恢复 | 日志 `Ethernet link restored, redialing immediately` → `Dial succeeded`（实测 1.5s）→ 10s 后 `Wireless: releasing`；`netsh wlan show interfaces` 回 Disconnected |
| 8 | standby：拔线零感知 | 托盘切 "Wired + wireless standby"，常驻 ping 窗口拔线观察丢包 ≤2 个；插回线 WLAN 不断（仍 Online） |
| 9 | 模式持久化 | 切 standby → `net stop/start gdut-net` → status 的 Mode 仍为 standby |
| 10 | 路由/指标无残留 | 服务停止后 `route print` 无 `10.0.3.2 /32`、`223.5.5.5 /32`；WLAN metric 还原 4270（`Get-NetIPInterface -InterfaceAlias WLAN`） |
| 11 | TUN 共存（四组合） | Mihomo TUN 开/关 × 有/无线，各跑一次 `curl -x http://127.0.0.1:7890 https://www.gstatic.com/generate_204`（期望 204）；Clash Verge 延迟测试有数字。前置：Merge.yaml 无 `interface-name`（2026-09-10 起，mihomo auto-detect 自动选 gdut/WLAN） |
| 12 | RasMan 卡死自愈（内建未实测） | 人为制造：无网线状态下让旧流程拨号 → 连续 756 时观察 `Dial port stuck (error 756 x3), restarting RasMan` 与 `RasMan restarted, port state cleared`；插线后能拨通。若服务无权限停/启 RasMan，日志为 warn 且不影响其他功能 |

## 安装器与日常 GUI（安装形态，ADR-0007）

前置：单文件 `gdut-net-setup.exe`（Releases 下载或本地 pack 产出）；已安装过一次的机器。

| # | 标准 | 验证方法 |
|---|---|---|
| 13 | 零键盘安装 | 双击 setup（自提权）→ 中文向导 4 屏：欢迎 → 账号（学号+密码）→ 进度（每步可见）→ 完成（≤20s 尝试连 IPC 显示实时状态）。全程不打开终端；UAC 弹出时确认一次即可 |
| 14 | 修复：保留密码 | 重跑 setup（或 `--repair`）→ 维护页"修复"→ 默认选中"使用现有密码"（零输入）→ 完成；服务/拨号继续正常，`password_blob` 不变 |
| 15 | 修复：改密码 | 维护页"修复"→ 输入新密码 → 完成；`config.toml` 的 `password_blob` 已变、拨号成功；日志/界面无明文密码 |
| 16 | 卸载两档 + 残留检查 | a) 开始菜单"卸载 GDUT Net"/应用和功能 → 默认档：`sc.exe query gdut-net` 1060、开始菜单文件夹消失、卸载键消失、安装目录消失，`C:\ProgramData\gdut-net` 保留。b) 勾"同时删除配置与日志"：ProgramData 目录也消失。两档均幂等，可重复执行 |
| 17 | 开始菜单 10 项 | `%ProgramData%\Microsoft\Windows\Start Menu\Programs\GDUT Net\` 恰好 10 个 `.lnk`：GDUT Net、状态查看、回校模式、回家模式、启动托盘、无线体检、打开日志、代理检查、说明、卸载 GDUT Net；逐项打开验证，管理员项带盾牌 |
| 18 | 开机形态 | 重启后：无控制台黑窗；`sc.exe query gdut-net` 运行且已拨号；托盘自启（Run 键指向安装目录）；双击"GDUT Net"（或左键托盘）弹出中文日常窗口 |
| 19 | 管道安装不回归（`switch` 依赖） | 管理员 PowerShell：`Get-Content pw.txt \| .\gdut-net.exe install --password-stdin` 成功、英文输出、退出码 0；DPAPI 密文写入 `config.toml`（switch-v4 迁移链依赖此行为） |
| 20 | `--keep-password` | `$out = & .\gdut-net-setup.exe --silent --keep-password 2>&1 \| Out-String; $LASTEXITCODE` → 0、英文输出、无需输入密码；无已存密文时报错并非零退出。`--keep-password` 单独用（不带 `--silent`）被拒绝 |
| 21 | 双击单实例 | 托盘已在运行时双击 `gdut-net.exe`（无参数）→ 只唤出已有窗口，不出现第二个托盘图标/进程；从控制台启动（有 console）仍走 CLI 行为 |
| 22 | GUI 中文无方块 | 安装器与日常窗口全部中文可读，无 □（tofu）；系统字体 `msyh.ttc` 加载成功；人为断字体路径时应显示错误页而非静默方块 |
| 23 | 关窗 = 隐藏 | 点日常窗口关闭 → 仅隐藏（进程/托盘不退）；左键托盘重新唤出并聚焦；右键原生中文菜单：打开主界面 / 有线优先 / 有线+无线备用 / 立即重拨 / 退出托盘 |
| 24 | GUI 文件日志 | `C:\ProgramData\gdut-net\logs\` 出现 `tray_r*.log`（和 `setup_r*.log`）；GUI 进程崩溃不影响服务运行 |
