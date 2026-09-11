# gdut-net

广东工业大学（大学城校区 = Higher Education Mega Center）有线网第三方认证客户端，替代 Dr.COM 官方客户端。核心是 PPPoE 拨号守护，可选的 Dr.COM 心跳兼容模式，且与 TUN 类虚拟网卡完全共存。

## Language

### 网络与认证

**拨号条目 (Dial Entry)**:
Windows RAS 电话簿中的一个命名条目（如 `gdut`），承载 PPPoE 宽带连接。
_Avoid_: 宽带连接、VPN、adapter

**会话 (Session)**:
一次从拨号成功到断开为止的 PPPoE 连接。

**掉线 (Drop)**:
会话不再通流量。包括 RAS 层断开，以及"RAS 显示已连接但实际被服务器踢掉"的僵死会话。
_Avoid_: 断线（口语）

**重拨 (Redial)**:
掉线后按指数退避自动重建会话。

**守护 (Watchdog)**:
持续监测会话健康并触发重拨的循环。

**物理适配器 (Physical Adapter)**:
有线网卡，会话与心跳的一切流量必须显式绑定到它，绝不走虚拟网卡（TUN/wintun）。

**探针 (Probe)**:
判定会话是否真正通流量的周期性探测：网关 ICMP 探链路，异常时 HTTP 探测复核是否被服务器踢；均绑物理适配器。
_Avoid_: ping 检测、保活探测

**多设备判定 (Device Limit)**:
学校按 MAC + HTTP User-Agent 判定设备数（1 有线 + 2 无线）；DHCP/随机 MAC 会误判超限。

**统一身份认证**:
无线网网页认证（Dr.COM eportal，`10.0.3.2:801`）所用账号体系，与有线同一套学号+密码；无线接管功能由本客户端的 wireless 模块自动完成。

### 无线接管（Wireless Takeover）

**无线接管 (Wireless Takeover)**:
有线失联后自动关联 SSID `gdut` 并完成 eportal 认证、由 wireless manager 执行的整套动作。
_Avoid_: WiFi 切换（口语）、回退（fallback）

**模式 (Net Mode)**:
`wired_exclusive`（平时 WLAN 断开，失联去抖后接管，恢复稳定后让位）或 `wired_plus_standby`（WLAN 常连常认证，OS 路由瞬间接替）。运行时经 IPC `SetMode` 切换并持久化。
_Avoid_: 双模（口语）

**让位 (Release)**:
exclusive 模式下有线恢复稳定后断开 WLAN 关联、撤销 /32 路由、还原 metric 的动作。
_Avoid_: 注销（logout 接口已知不可用，不用）

**eportal 认证 (Portal Auth)**:
绑 WLAN 源 IP 的 HTTP GET 登录请求（参数含学号/密码/wlan_ac_ip），回包 JSONP `dr1004({"result":"1",...})` 即成功。含密码的 URL 永不落日志。

**接管去抖 (Takeover Debounce)**:
以太网 link down 或有线会话失联持续 `takeover_after_secs` 才启动接管，防抖动误切。

### 心跳（兼容模式）

**心跳 (Heartbeat)**:
发往认证服务器的 Dr.COM keepalive 报文，用于服务器开启校验时维持会话；默认关闭。

**兼容模式 (Compatibility Mode)**:
心跳功能开启的状态。默认关。
_Avoid_: 保活模式

**兼容模式模块 (Heartbeat Module)**:
可插拔的心跳实现，由配置选择，绑定物理适配器发包。

**Seed**:
心跳握手时服务器下发的 4 字节数值，决定报文校验模式并参与后续报文。

**文件报文 (File Packet)**:
服务器下发的特殊响应，携带协议 flag/版本信息，客户端需从中学习参数。

**托盘 (Tray)**:
用户会话内的常驻 UI 进程，展示会话状态并在守护异常时弹系统通知；与服务 IPC，不参与拨号。左键弹出中文日常窗口，右键原生中文菜单；单实例（再启动只唤出已有窗口）。
_Avoid_: 界面（泛称）

**日常界面 (Daily GUI)**:
托盘进程内的常驻 egui 窗口（中文）：状态卡、立即重拨、模式切换、修改账号密码、最近事件、打开日志。关窗=隐藏，进程不退。ADR-0007。

**安装器 (Setup)**:
`gdut-net-setup.exe`，单文件发布物（尾部内嵌载荷容器）。自提权、中文向导（欢迎/账号/进度/完成）+ 维护页（修复/卸载）。`--silent [--keep-password]` 为英文控制台的无界面模式。ADR-0007。

**载荷容器 (Payload Container)**:
setup 文件尾部追加 `[files][TOC][footer]` 的自定义容器：footer 24B，magic `GDUTPAK1` + u32 版本 + u32 条目数 + u64 TOC 偏移；每项 {文件名, offset, len, sha256}。截断/坏包/校验失败 = 拒绝安装，绝不半装。`src/payload.rs`。

**语言策略 (Language Policy)**:
控制台英文（CLI/日志/脚本/`.bat`/`.ps1` 回显——中文 Windows 控制台 GBK 乱码）；GUI 中文（安装器/日常窗口/托盘菜单/`说明.txt`——系统字体渲染，面向学生用户）。ADR-0007。

**双出口 (Dual Egress)**:
校园网同时存在 DHCP 物理口（172.17.x.x）与 PPP 会话口（`gdut`，10.30.x.x）；两者隔离，互联网出站必须走 PPP，家中单出口无此问题。注意 Windows 的**有效 metric = RouteMetric + InterfaceMetric**（本机实测：PPP 1+25=26，物理 0+4250=4250，WLAN 0+4270=4270）——只看 RouteMetric 会得出"物理口优先"的错误结论。

## Rules

- 无线接管的一切发包（portal 登录、ICMP/HTTP 探针）显式绑 WLAN 适配器源 IP；WLAN 会话存活期间服务自管两条 /32 主机路由（portal 主机 + HTTP 探测目标，via WLAN 网关），否则 Mihomo TUN 覆盖路由下绑源 socket `ENETUNREACH`。增删与让位/切模式/服务停止三条出口绑定，启动清残留。
- 含密码的 portal URL 永不落日志/事件尾巴（打码只留 host+path）。
- **拔线期间绝不拨号**：无载波拨号会把 PPPoE 端口卡在 dialing 态，之后所有拨号返回 756 且重试无法清除（实测只能重启 RasMan/系统）。服务已内建：link gate（链路 down 时 5s 轮询不碰端口）+ 插线瞬间 `request_redial` + 连续 3 次 756/813 自动重启 RasMan。
- **不得给 Mihomo 设 `interface-name`**（2026-09-10 实证）：显式绑 `gdut` 在无线接管时全超时（该接口不存在 → 每个出站硬错 `interface not found`，无回退）；mihomo auto-detect（sing-tun 按"非虚拟 up 接口中总有效 metric 最低者"）在本机自动正确选 `gdut`/`WLAN`。Merge.yaml 保持无此键，改后需完整重启 Verge（重合并只在进程重启时发生）。
- WLAN 接口 metric 压制（standby 与 exclusive 接管期）：目标 100 —— 低于物理口 4250、高于 PPP 26，保证有线健康时有线优先、有线路径消失瞬间无线接替。切走/让位/停止时还原。
- 心跳相关的一切发包绑定物理适配器，绑定失败（端口 61440 被官方客户端占用）视为兼容模式不可用，报错而非静默。
- "掉线"以流量探测为准，不单看 RAS 状态。
- WSL 为 mirror 模式，跟随主机路由表；直连走 TUN 即可（fake-ip 已退役为 redir-host）。
- 查系统代理只信注册表 `HKCU\...\Internet Settings\ProxyEnable`，不信 GUI 开关（前后端脱节）；该值重启不清零；FlClashHelperService（SYSTEM 常驻，FlClash 关了也可能活着）会把它写回 1；Verge 守卫在 OFF 时已停可排除；`clash-verge-service` 不是 SCM 服务（sc 1060），只跑内核不管代理。
- Verge 运行时配置注入点是 `profiles/Merge.yaml`（全局拓展配置），别手改生成的 `clash-verge.yaml`；回滚=删段后完整重启 Verge。

## 实测陷阱（2026-09-10 真机会战）

- **`SOCKADDR_IN.S_addr` 必须网络序**：`u32::from(ip)` 直接存入 LE 内存是字节反序，`CreateIpForwardEntry2` 会把 10.0.3.2 写成 2.3.0.10（route print 实锤）。用 `wireless::ipv4_to_s_addr`/`s_addr_to_ipv4`（`to_be()` 换算，两端各一次）。
- **`MIB_*_TABLE2` 的定长 `[T; 1]` 字段不可索引**：>1 条记录即越界 panic（本机 ~100 条路由，manager 启动即死且 tokio 静默吞掉）。一律 `from_raw_parts(Table.as_ptr(), NumEntries)` 变长视图；同教训见 `WLAN_INTERFACE_INFO_LIST.InterfaceInfo`。
- **事件唤醒不得驱动状态机**：快照推送等事件若让主循环顺带跑 `run_once`，退避 sleep 被 2s 一切碎（实测 7 分钟重拨 79 次、探针 30s→2s）。主循环用"绝对唤醒时刻 + 剩余时长"并只在计时到点/显式命令时跑状态机。
- **eportal 回包语义**：`result:1`=成功；`result:0`+`ret_code:2`=该 IP 已在线（视为成功，别重试）；`result:0`+`ret_code:1`=密码错。按 `ret_code` 区分，勿匹配中文 msg。
- **绑源 SYN 偶发被丢**（校园 AC 对未认证 MAC 的限流，官方客户端同样受）：单次 socket 8s 超时（覆盖 Windows SYN 重传 1s/2s/4s），CLI 三连试，manager 侧 5/15/30 退避重试兜底。
- **无线接管窗口实测**：拔线 → `Ethernet link down, dial paused` → portal 登录 ≈13s；插线 → 1.5s 拨上 → 10s 后 WLAN 让位，/32 清 0、metric 还原。
- **mihomo 接口缓存 TTL 20s**（sing-tun `singledo.NewSingle(20s)`）：接口消失后旧绑定最长 20s 内自愈，无需重启内核。

## 工程与运维陷阱（真机实战）

### RAS / 拨号
- `RasEnumConnectionsW` 缓冲元素 `dwSize` 必须预置 `sizeof(RASCONNW)`，否则 `632 ERROR_INVALID_SIZE`；pbk 比较用 `pbk_eq_ci`（大小写不敏感）。
- `RasSetEntryPropertiesW` 返回 `816`（端口占用）视为成功（端口释放后可拨），勿当硬错。
- 无载波拨号会把 PPPoE 端口卡在 dialing 态 → 之后所有拨号 `756`，重试无法清除（详见上文 Rules + link gate 设计）。
- `service` 停止后进程可能残留致重装 `1073`；`service_run` 显式 `std::process::exit` 兜底；`install` 幂等（`1073` → `change_config`）。

### 探针 / 配置
- `http_probe_url` 仅接受 `http://` + IPv4 字面量（`probe::parse_http_probe_target` 单一实现复用）；`9.9.9.9` 被校园网墙，默认 `223.5.5.5`；gateway `0.0.0.0` 时 ICMP 目标退化为 `223.5.5.5`。

### IPC / 服务
- 命名管道默认 DACL 拒绝用户会话：服务（SYSTEM/Session 0）建管必须挂 SDDL `D:(A;;GRGW;;;AU)`（经 `create_with_security_attributes_raw`），否则托盘/`status` 报 `os error 5`；改动只在服务重启后生效。
- 托盘单实例 = 命名 mutex `gdut-net-tray-singleton` + 唤出命名事件 `gdut-net-tray-show`（自动复位）。二次启动：抢 mutex 失败 → `SetEvent` 唤出已有窗口 → 本进程直接退出，不产生第二个图标。

### 安装器 / GUI 陷阱
- setup 与托盘都是 **GUI 子系统（`windows_subsystem`），没有 stdout/stderr**：silent 模式输出必须经 PowerShell 管道捕获（`& $setup --silent --keep-password 2>&1 | Out-String` + `$LASTEXITCODE`；裸 cmd 重定向会丢输出——`wireless-test.bat` 同一模式）。GUI 模式启动失败走原生 `MessageBoxW` 弹窗，否则窗口一闪而逝。
- 文件日志：托盘 `tray_r*.log`、setup `setup_r*.log`（滚 5MB×2），都在 `C:\ProgramData\gdut-net\logs\`；服务日志仍是 `gdut-net_r*.log`。GUI 进程崩溃只记日志，不影响服务。
- setup 载荷容器：`[setup][files][TOC][24B footer]`，magic `GDUTPAK1`；从文件尾读 footer，逐项 sha256 校验。开发态（未打包）回退读自身旁边 `payload/` 目录；发布物被截断/篡改 = 明确报错拒绝安装。
- 安装目录删除要延迟重试（`schedule_install_dir_removal`，cmd 循环 90×1s + `CREATE_NO_WINDOW|DETACHED_PROCESS`）：setup 窗口/开始菜单快捷方式会占用目录；`.arg()` 会把引号转义成 `\"` 而 cmd 不认——必须 `raw_arg` 原样传。
- 开始菜单快捷方式工作目录 = 安装目录；管理员项（campus/home/无线体检/卸载）带 `SLDF_RUNAS_USER`（盾牌）。
- egui 默认字体无 CJK 字形：GUI 必须加载系统字体（`msyh.ttc`，回退 `simhei.ttf`/`simsun.ttc`），找不到报错页而不是静默方块（ADR-0006 同源教训）。

### 脚本 / 部署（Windows 侧）
- `*>&1 | Out-File` 会把英文 `WARN` 当 `NativeCommandError`；GUI 子系统 exe（setup/tray）直接重定向丢输出——`switch-v4.ps1` 用 `& $setup --silent --keep-password 2>&1 | Out-String` + `$LASTEXITCODE` 捕获。
- `switch-v4.ps1` 成功检测搜英文 `Dial succeeded` / `dropped`——中文匹配永不命中。
- `UAC ConsentPromptBehaviorAdmin=0 + EnableLUA=1` 下的 RunAs 行为（2026-09-11 修正）：**可能自动提权成功，也可能失败，取决于策略**——2026-09-11 实测 `ShellExecuteExW "runas"`（setup 自提权）在本机直接成功、无提示；旧记录"RunAs 静默失败"不再是当前行为。免 UAC 的**预授权通道**仍是计划任务 `gdut-switch`（实测身份：`Lemonawa`/交互式/最高权限——所以不弹 UAC；`AllowStartIfOnBatteries`，10min 超时），触发 `schtasks /Run /TN gdut-switch`。**任务窗口可见**：中途关掉窗口 = Ctrl+C 杀掉脚本（退出码 `0xC000013A`）——换装和拨号通常已完成，但最后 75s 稳定性检查与 `SUCCESS` 日志会缺失（无实质影响）。要隐藏窗口/改任务指向，别用 `schtasks /TR` 拼引号（`\"` 不是 PowerShell 转义，R8 实测 ParserError）：管理员用 `Set-ScheduledTask -TaskName gdut-switch -Action (New-ScheduledTaskAction -Execute 'powershell.exe' -Argument '-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File "C:\Program Files\gdut-net\switch-v4.ps1"')`，再用 `schtasks /Query /TN gdut-switch /V /FO LIST` 核对。
- 部署形态（2026-09-11）：产品安装到 `C:\Program Files\gdut-net\`（服务路径、HKCU Run 托盘自启、计划任务 `gdut-switch` 全指向这里），开始菜单 `GDUT Net` 10 项 + 应用和功能条目就位；dial + 75s 稳定性验证通过。桌面工具包（`C:\Users\Lemonawa\Desktop\gdut-net\`）保留作 fallback，不再日常使用。
- 个人运维脚本（`switch-v4.ps1`、`rollback.bat`、`rollback-v4.ps1`、`一键切换.bat`、`tun-watch.ps1`）与产品脚本同居安装目录；公开发布 payload 不含它们。
- `switch-v4.ps1` 不再内嵌明文密码：安装改走 `gdut-net-setup.exe --silent --keep-password`（复用已存 DPAPI 密文重新 `set_credentials`）。
- `pw.txt` 用后即删；明文密码不落盘。

### WSL（从 Linux 侧操作这台机器）
- `powershell.exe` 不在 PATH：WinPS 5.1 = `/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe`；ps7（应用商店版）= `/mnt/c/Users/Lemonawa/AppData/Local/Microsoft/WindowsApps/pwsh.exe`。
- 从 WSL 提权不可靠（历史上 `-Verb RunAs` 被 UAC 静默失败；2026-09-11 本机 `ShellExecuteExW runas` 又实测可直接成功），别赌：改网络/装服务一律走预授权计划任务 `gdut-switch` 或让用户执行。
- `/mnt/c/ProgramData/**` 等受保护路径对 WSL 只读——改配置走程序自身（`install` 幂等重写 config）或 Windows 管理员侧。

### 代理 / Verge 排障
- `ProxyEnable` 存 HKCU 重启不清零；是否翻回只看自启动项（`HKCU/HKLM\...\Run` 应无 FlClash/Verge/clash 系）。回写是事件驱动的（FlClash HelperService 定时写回，GUI 开关脱节）；深挖用注册表审计（中文系统 auditpol 子类别"注册表"）+ `Get-WinEvent ID=4657` 看进程名；历史定案是改 `Connections\gdut` blob（flags bit1）+ `MigrateProxy` 置 0，而非杀进程。
- Verge 改 `Merge.yaml` 必须**完整退出并重启 Verge 进程**才重新合并；TUN 状态看 `Get-NetAdapter Mihomo` + `0.0.0.0/0` 路由在不在。
- fake-ip 已退役为 redir-host（频繁重启内核 + 系统 DNS 缓存下，旧映射进缓存即 RST）；国外慢先换节点再怪内核（固定 5.1s×N 次 = 节点晚高峰）。
- **拨 TUN 开关必重启 opencode/长连接进程**（TCP 无迁移，SSE 静默死亡）；判新老连接用 `curl ai.lma.moe/v1/models`（401 = 新连接活）。
- Tailscale 家↔校不能直连（校园 CGNAT = 对称 NAT + 端口重写 + 多 ISP 池；家路由器按远端过滤）；修复在家侧：开 UPnP 或转发 UDP 41641→192.168.5.11；全案 `docs/tailscale-p2p.md`。

### 给用户的断网窗口操作
- 必须自带回滚块（能独立执行；网络炸了用户侧无 AI 可达）。
