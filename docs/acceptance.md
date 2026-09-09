# 真机验收清单

> 原 README 内附清单，随英文版重组迁出至此。

前提：Windows 11 x64 物理机，校园网有线接入。

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
sc.exe qc gdut-net               # AUTO_START，binPath 含 --config 与 run
reg query "HKLM\SYSTEM\CurrentControlSet\Services\EventLog\Application\gdut-net"
                                 # EventMessageFile=%SystemRoot%\System32\netmsg.dll（REG_EXPAND_SZ）
                                 # TypesSupported=0x7
```

`log.event_log = true` 时：事件查看器 → Windows 日志 → 应用程序，来源 `gdut-net`，warn/error 应同步出现（如"认证失败(691)"）。

## 托盘与状态（用户会话）

```powershell
.\gdut-net.exe tray      # 托盘青色图标；菜单 Status / Redial now / Details / Exit
.\gdut-net.exe status    # 终端打印：状态、在线时长、IP、掉线原因、重拨次数、心跳
```

## 无线接管（v0.3）

前置：`gdut-net.exe install` 过一次；WLAN profile `gdut` 存在（手工连过一次校园 WiFi）。

```powershell
.\gdut-net.exe wireless test    # 一次性现场验证：应打印 WLAN IP/gw、HTTP 200、RESULT: SUCCESS，结束自动断开
.\gdut-net.exe status           # Mode: ... / Wireless: ... 两行出现
```

| # | 标准 | 验证方法 |
|---|---|---|
| 6 | exclusive：拔线 ≤15s 无线可用 | `Get-Content ...log -Wait` 观察 "associating" → "portal login success"；插回线 ≤20s 出现 "releasing"；`netsh wlan show interfaces` 回 Disconnected |
| 7 | standby：拔线零感知 | 托盘切 "Wired + wireless standby"，常驻 ping 窗口拔线观察丢包 ≤2 个；插回线 WLAN 不断（仍 Online） |
| 8 | 模式持久化 | 切 standby → `net stop/start gdut-net` → status 的 Mode 仍为 standby |
| 9 | 路由无残留 | 服务停止后 `route print` 无 `10.0.3.2 /32`、`223.5.5.5 /32`；WLAN metric 还原（`Get-NetIPInterface -InterfaceAlias WLAN`） |
| 10 | TUN 共存 | Mihomo TUN 开着跑 6/7 两项（/32 由服务自管，ADR-0005） |
