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
| 4 | 关心跳 72h 不掉线；开心跳现场验证 | `heartbeat.enabled=false` 挂机 72h，日志无"判定掉线"（或仅极少数且自动恢复）；需要验证兼容模式时 `enabled=true`，在**物理网卡**上核对端口 61440 出现 20s 周期 keepalive |
| 5 | 卸载干净 | `.\gdut-net.exe uninstall --purge` 后：`sc.exe query gdut-net` → 1060（不存在）；`reg query "HKLM\SYSTEM\CurrentControlSet\Services\EventLog\Application\gdut-net"` → 拒绝访问/不存在；`reg query "HKLM\SOFTWARE\gdut-net"` → 不存在；`C:\ProgramData\gdut-net` 已删除 |

## 原生 IPv6（2026-09-23，v0.4.1）

- 拨号条目：`C:\ProgramData\gdut-net\gdut.pbk` 的 `[gdut]` 段应为 `ExcludedProtocols=0`
  （旧版本写成 `8` = 排除 `RASNP_Ipv6` → IPV6CP 不协商，链路永远没有 v6）。
- 拨号成功后：`Get-NetIPAddress -InterfaceAlias gdut -AddressFamily IPv6` 出现
  `PrefixOrigin=RouterAdvertisement` 的全局地址；`Get-NetRoute -AddressFamily IPv6 -DestinationPrefix ::/0`
  有 gdut 条目；`Get-NetConnectionProfile` 中 gdut 项 = `Internet/Internet`。
- PPP 接口不承载 DNS：`Get-DnsClientServerAddress` 里 gdut 的 IPv4 列表应为空（DNS 由物理口提供），
  服务日志出现 `PPP interface 'gdut' DNS detached (physical NIC serves DNS; AAAA ok)`。
  反例（必现）：PPP 带 DNS 时 Windows 不向应用交付 AAAA —— `ping -6 域名`/`curl -6 域名` 全空而 `nslookup` 正常。
- 直连冒烟：`curl --noproxy "*" -6 -k -o NUL -w "%{http_code}" https://www.baidu.com/` = 200；
  `ping -6` 到 CN v6 目标 0% 丢包。
- 判据提醒：本机 TUN 路径下 ICMP 会被本地合成（不存在的地址也"0% 丢包"），可用性一律用 TCP/TLS 判。

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

## 2026-09-17 架构加固（待真机复验）

- [x] Linux 全套 `cargo test`（含 Wireless Egress 5 项接口测试、Supervisor 26 项编排测试）。
- [x] Linux / Windows cross-target `clippy -D warnings` 与 `fmt` 通过。
- [ ] 安装新版后重复无线接管条目 6–10，确认 eportal /32、metric 恢复、拔线不拨号和让位行为不变。

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
| 11 | TUN 共存（四组合） | Mihomo TUN 开/关 × 有/无线，各跑一次 `curl -x http://127.0.0.1:7890 https://www.gstatic.com/generate_204`（期望 204）；Clash Verge 延迟测试有数字。前置：Merge.yaml 无 `interface-name`；Verge ≥2.5.4 另需在 TUN GUI 弹窗确认 MTU=1480 与排除网段未丢（2026-09-20 实测保存热生效）。再跑一次浏览器打开 Google/Cloudflare；若 Google 局部卡住，禁 Chrome QUIC 后复测 |
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

## 深挖结构波真机验收（2026-09-13，安装形态）

新 runtime（supervisor 编排 + 双车道执行器，`95a3727`）在本机完整走查：

| 项 | 结果 |
|---|---|
| 部署（`gdut-switch`，`--silent --keep-password`） | setup exit=0；75s 稳定 `http=204` |
| 服务/托盘/IPC | Running/自启；托盘连上管道；`status` 全绿 |
| 自动拨号 + 两级探测 | `Dial succeeded`；Probe Alive 周期正常 |
| 手动重拨（IPC Redial） | `IPC command: manual redial` → ~2s 拨回，`Drop reason: Manual redial` |
| 模式切换（exclusive ↔ standby） | 双向生效并落盘；standby 下 WLAN 认证成功、/32 路由 + metric 100 就位 |
| 停服务清理 | /32 清空、metric 还原、WLAN Disconnected（三条出口） |
| 物理拔线 → 无线接管 | 2s 发现；期间无任何 Dial（link gate）；≈14s portal Online |
| 插回网线 → 恢复与让位 | `Ethernet link restored, redialing immediately` → 1.8s `Dial succeeded` → metric 还原 → WLAN 让位断开 |

## 回家/回校一键切换真机走查（2026-09-25，安装形态）

前置：校园网在线（`gdut` 拨号 10.30.194.204、服务 Running/Automatic）；开始菜单"回家模式"/"回校模式"逐项打开（管理员项由 Shell 弹 UAC）。

| 项 | 结果 |
|---|---|
| 回家模式 | exit 0 + `HOME MODE OK`；SCM 事件 7040 `自动→按需`（12:49:43）；服务日志 `Stop signal received, hanging up and exiting`；PPP `gdut` 摘除；停机 65s 内服务日志零新增（无无效重拨）。托盘进程按设计一并退出 |
| 回校模式 | SCM 事件 7040 `按需→自动`（12:50:46）；服务启动 → `Dial succeeded` 2.0s（12:50:48）；30s 稳定检查窗口内无 `considered dropped`/`Probe failed`；`status` = Connected、Redial 0 |
| 托盘接力（修复后复测） | 回家退出托盘 → 回校成功路径 `start "" explorer.exe "%~dp0gdut-net.exe"` 经 explorer 去提权拉起：新托盘完整性级别 **Medium**（非管理员）、`gdut-net-tray-singleton` mutex 与 `gdut-net-tray-show` 事件均 `ERROR_ALREADY_EXISTS`；复测全程服务会话未断（uptime 连续、Redial 0、IP 不变） |

复测方法（不碰服务）：`Stop-Process` 杀掉 session 1 的托盘 → 以管理员上下文执行 campus.bat 新增行 → 校验上述三项；WM_CLOSE 收起弹出的日常窗口（关窗 = 隐藏，托盘进程仍在）。

## 深挖两波遗留事项（deferred minors，2026-09-12）

> 来源：两波深挖（机械波 + 结构波）逐任务评审；完整上下文在本地归档
> `.superpowers/sdd/2026-09-12-deepening-{mechanical,structural}/progress.md`。
> 全部为评审 Minor 级、已逐条判定"可延后"；标 ⚠️ 的三条修复价值最高。

### 已修复（2026-09-13，`fix/minors-cleanup`：6fc25b6 + d48b89d）
- ✅ `rollback_install` 现按请求的配置路径回滚（CLI `install --config <自定义>` 不再指回缺省路径）。
- ✅ 快照 `ppp_ip` 随每次链路采样（2s 一拍）刷新，不再滞后一个探测周期。
- ✅ `RouteGuard::set_standby_metric` 仅在保存缺失或 ifindex 变化时读取原值；释放失败后的同接口重试不覆盖原值（含 WLAN 重连换 ifindex 的修正）。
- ✅ 无效无线配置的 "manager disabled" 不再重复打两条（壳侧降为 debug）。
- ✅ 核心 `SetMode` 中的死写 `cfg.wireless.mode` 已删除。

### 其余（清理 / 文档 / 测试）
- `ipc/session.rs::send_and_confirm` 未在发送前校验 `max_frames >= 1`；帧预算耗尽时返回未命中谓词的帧（有文档、唯一调用方自检）。
- `SyncSession` 未注明"仅限同步上下文"（现有调用点本就同步）。
- `tray/mod.rs` AUMID 写入的 `RegCloseKey` 失败告警被简化掉（close 失败无实际意义）。
- 卸载键现在 8 次 create/close 循环（安装期一次性，可忽略）。
- `Unknown` 回滚文案沿用 "Service existed but its path was unreadable"，SCM 不可达时也会打这句（字符串被冻结）。
- `setup/work.rs::label_zh` 兜底直接回显 key（当前固定 4 行不可能触发；增行时需补标签）。
- 失败路径日志的模块路径从 `setup::work` 变为 `service`（文案相同）。
- `supervisor.rs` 重拨失败 toast 的拔线豁免比旧逻辑多一条 `eth_link == Some(false)`（更保守，接受）。
- 首次链路采样为 `None` 后再拔线会打 `Ethernet link down at startup`（仅措辞，接受）。
- `WatchdogStepped` 无 in-flight 护栏（Main 车道 FIFO 前提下不可达）。
- Stop 握手前 `LaneMsg::Shutdown` 的发送无超时（队列 ≤3/64，理论项）。
- 从 cmd 通道关闭退出时缺 `Stop signal received` 日志（行为更干净，仅日志口径）。
- `runtime.rs::Shell` 与 `supervisor.rs` 各持一份 `Config`（核心只读构造时的副本、不再写回；壳负责持久化；扩展时注意）。
- `tests/status.rs` 未钉：`wphase_zh(Authing/Error)`、`session_zh(Idle)`、`heartbeat_en(Running)`、`(ip, error)` 优先级。
- 既有（两波之前）：debug 构建 `gdut-net` 会触发 clap debug-assert panic（`password_stdin` 的 `requires = "cmd"` 无对应参数），release 与测试不受影响。
