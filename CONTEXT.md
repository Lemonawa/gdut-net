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

**无线出口 (Wireless Egress)**:
无线接管期间受管理的 WLAN 出口：通往 eportal 与无线探针目标的 /32 主机路由、接口 metric 压制/还原，以及获取、等待生效与释放的完整生命周期。
_Avoid_: 路由模块（只指其中一部分）

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
- **不得给 Mihomo 设 `interface-name`**（2026-09-10 实证）：显式绑 `gdut` 在无线接管时全超时（该接口不存在 → 每个出站硬错 `interface not found`，无回退）；mihomo auto-detect（sing-tun 按"非虚拟 up 接口中总有效 metric 最低者"）在本机自动正确选 `gdut`/`WLAN`。Merge.yaml 保持无此键。
- Clash Verge Rev ≥2.5.4 将 `tun.mtu` / `tun.route-exclude-address` 改为 GUI 设置优先，Merge 中的同名字段会被 GUI 值覆盖。两者必须在 Verge“系统设置 → 虚拟网卡模式”弹窗维护；本机 2026-09-20 实测保存后热生效，无需重启。本机路径 MTU 实测 1480，当前 TUN MTU=1480；排除段含 RFC1918、家宽公网段与 Parsec STUN /32。
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
- PPPoE 条目必须置 `RASNP_Ipv6`（=8）：`RASENTRYW.dwfNetProtocols` 只给 `RASNP_Ip|RASNP_Ipx|RASNP_NetBEUI`（值 7）时，RAS 会把补集写进 pbk 的 `ExcludedProtocols=8`，IPV6CP 根本不协商 → 链路永远没有 v6。2026-09-23 真机实测：宿舍 PPPoE 本身**支持** v6，置位后立刻拿到 RA 全局地址 `240c:cd22::/64` + `::/0`（网关 = 对端 link-local），直连 ping/curl v6 全通，`Get-NetConnectionProfile` = Internet/Internet；系统自带"宽带连接"条目为 `ExcludedProtocols=0` 可作对照。`Ipv6PrioritizeRemote` 0/1 都能起（与结论无关）。

### IPv6 / DNS 解析（2026-09-23 实测）
- 本机前缀策略表曾被改成 IPv4 优先（`netsh interface ipv6 show prefixpolicies` 只剩 `::ffff:0:0/96` @45）；已按 Windows 默认恢复 9 条（`::1/128 50 0`、`::/0 40 1`、`::ffff:0:0/96 35 4`、`2002::/16 30 2`、`2001::/32 5 5`、`fc00::/7 3 13`、`3ffe::/16 1 12`、`::/96 1 3`、`fec0::/10 1 11`）。恢复后 AAAA 问题依旧 → 不是真因，但该表被篡改本身就该修。
- **已定位（2026-09-23）**：只要 PPP 接口带着 RAS 下发的 v4 DNS，Windows DNS 客户端就不向应用层交付 AAAA —— `ping -6 域名`/`curl -6 域名`/`getaddrinfo` 全空，而 `nslookup -type=AAAA`、`Resolve-DnsName -Server <ip>` 正常，`Get-DnsClientCache` 里只有 A。对照实验：清空 PPP 接口 DNS（DNS 交给物理口）→ 立刻恢复；重拨后 RAS 重新下发 → 立刻再次失效。服务器与链路本身没问题（绑定 PPP 源地址的原生 UDP 查询实测 `www.qq.com/AAAA` rcode=0 an=1）。
- 产品对策：`adapter::detach_ppp_dns(entry_name)` 在**每次拨号成功后**把 PPP 接口 IPv4 DNS 置 none（`netsh interface ipv4 set dnsservers name=<entry> source=static address=none`），DNS 交给物理口。**清之前先确认别的 up 接口确实带 DNS**（`GetAdaptersAddresses`.`FirstDnsServerAddress`），没有就不动；清完只 `getaddrinfo` 复核并记日志（`DNS detached (physical NIC serves DNS; AAAA ok)`），**绝不回写**——2026-09-23 实测写回 `source=dhcp` 会触发 PPP 会话重协商，约 30s 后 `RAS session gone, considered dropped`，真机切换脚本随即判 unstable 并回滚到桌面版（v0.4.1 部署失败的真因）。真出问题只影响当前会话，下次拨号 RAS 重新下发 DNS 即自愈。
- 排查手法：`Get-NetConnectionProfile` 看 `IPv6Connectivity`；`netsh interface ipv6 show route` 看 `::/0`；**绑定 PPP 源地址（10.30.194.204）的原生 UDP 查询**（PowerShell `UdpClient` 绑该源地址）区分"服务器不答"与"客户端隐藏"；`pktmon filter add dns -p 53` + `pktmon start --capture --pkt-size 0` + `pktmon etl2pcap`（产物是 pcapng，不是 pcap）看客户端到底发没发。
- 反例备忘：Mihomo TUN 下 `ping -6 2001:da8:ffff::dead:beef`（不存在的地址）也 0% 丢包 ~1ms —— 该路径 ICMP 由本地 TUN 合成，**ping 通不代表 v6 可用**。

### 探针 / 配置
- `http_probe_url` 仅接受 `http://` + IPv4 字面量（`probe::parse_http_probe_target` 单一实现复用）；`9.9.9.9` 被校园网墙，默认 `223.5.5.5`；gateway `0.0.0.0` 时 ICMP 目标退化为 `223.5.5.5`。

### IPC / 服务
- 命名管道默认 DACL 拒绝用户会话：服务（SYSTEM/Session 0）建管必须挂 SDDL `D:(A;;GRGW;;;AU)`（经 `create_with_security_attributes_raw`），否则托盘/`status` 报 `os error 5`；改动只在服务重启后生效。
- 托盘单实例 = 命名 mutex `gdut-net-tray-singleton` + 唤出命名事件 `gdut-net-tray-show`（自动复位）。二次启动：抢 mutex 失败 → `SetEvent` 唤出已有窗口 → 本进程直接退出，不产生第二个图标。

### 安装器 / GUI 陷阱
- setup 与托盘都是 **GUI 子系统（`windows_subsystem`），没有 stdout/stderr**：silent 模式输出必须经 PowerShell 管道捕获（`& $setup --silent --keep-password 2>&1 | Out-String` + `$LASTEXITCODE`；裸 cmd 重定向会丢输出——`wireless-test.bat` 同一模式）。GUI 模式启动失败走原生 `MessageBoxW` 弹窗，否则窗口一闪而逝。
- 文件日志：托盘 `tray_r*.log`、setup `setup_r*.log`（滚 5MB×5），都在 `C:\ProgramData\gdut-net\logs\`；服务日志仍是 `gdut-net_r*.log`。GUI 进程崩溃只记日志，不影响服务。
- setup 载荷容器：`[setup][files][TOC][24B footer]`，magic `GDUTPAK1`；从文件尾读 footer，逐项 sha256 校验。开发态（未打包）回退读自身旁边 `payload/` 目录；发布物被截断/篡改 = 明确报错拒绝安装。
- 安装目录删除要延迟重试（`schedule_install_dir_removal`，cmd 循环 90×1s + `CREATE_NO_WINDOW|DETACHED_PROCESS`）：setup 窗口/开始菜单快捷方式会占用目录；`.arg()` 会把引号转义成 `\"` 而 cmd 不认——必须 `raw_arg` 原样传。
- 开始菜单快捷方式工作目录 = 安装目录；管理员项（campus/home/无线体检/卸载）带 `SLDF_RUNAS_USER`（盾牌）。
- egui 默认字体无 CJK 字形：GUI 必须加载系统字体（以 `src/fonts.rs` 的候选顺序为准：`Deng.ttf` → `simhei.ttf` → `msyh.ttc` → `simsun.ttc`），找不到报错页而不是静默方块（ADR-0006 同源教训）。

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
- Verge 改 `Merge.yaml` 必须**完整退出并重启 Verge 进程**才重新合并；TUN GUI 弹窗保存则热生效（2.5.4 实测）。TUN 状态看 `Get-NetAdapter Mihomo` + `0.0.0.0/0` 路由在不在。
- Google 类站点在 Chrome 里可能拿到 `alt-svc: h3` 后尝试 QUIC；2026-09-20 本机 TUN MTU=1480 时 TCP/H2 已恢复且 Google ≈0.5s，但 `curl --http3-only` 对 Google 仍超时（H3 握手能到 2.3s，后续无响应）。若浏览器偶发局部卡住，先在 `chrome://flags/#enable-quic` 禁用 QUIC，不要把 TCP 恢复误判成 MTU 未生效。
- fake-ip 已退役为 redir-host（频繁重启内核 + 系统 DNS 缓存下，旧映射进缓存即 RST）；国外慢先换节点再怪内核（固定 5.1s×N 次 = 节点晚高峰）。
- **拨 TUN 开关必重启 opencode/长连接进程**（TCP 无迁移，SSE 静默死亡）；判新老连接用 `curl ai.lma.moe/v1/models`（401 = 新连接活）。 微信同样受影响：TUN 重建后它会静默 stall，直到自身超时重连才把积压消息一次性补齐（2026-09-23 实测"喷水"），
  TUN 动过之后重启微信可立即恢复；判断标准是同机 curl/其他短连接一切正常而它"什么都出不来"。- Tailscale 家↔校不能直连（校园 CGNAT = 对称 NAT + 端口重写 + 多 ISP 池；家路由器按远端过滤）；修复在家侧：开 UPnP 或转发 UDP 41641→192.168.5.11；全案 `docs/tailscale-p2p.md`。

### Verge 2.5.5 增强配置挂载机制（2026-09-23 拉源码核对）
- `enhance/merge.rs::use_merge` 只做 deep_merge（`prepend-rules` 之类不会被识别，写进 Merge.yaml 就是死键——CONTEXT 旧说法在 2.5.5 仍成立）。
- 规则/代理/组/合并/脚本五类增强项是**按订阅条目挂载**的：`profiles.yaml` → `items[].option.{merge,script,rules,proxies,groups}` = 对应 uid；默认 uid 是 `Merge`/`Script`/`Rules`/`Proxies`/`Groups`（找不到就是空实现，静默不生效）。
- 所以"编辑规则"= 改该订阅 `option.rules` 指向的那个文件（`profiles/<uid>.yaml`）里的 `prepend`/`append`/`delete`（结构见 `enhance/seq.rs::use_seq`：prepend 拼在订阅规则前）。**光往 `profiles/*.yaml` 里写不改 `option.rules` 不会生效**；改完要完整退出并重启 Verge 进程。
- `tun.*` 的 GUI 键（MTU/route-exclude 等）另由 `enhance/tun.rs::enforce_tun` 在最后覆盖，仍以 GUI 为准。

### 浏览器侧 `ERR_CONNECTION_CLOSED`（2026-09-23 实测）
- 症状：微信/公众号页在 Chrome 报 `ERR_CONNECTION_CLOSED`，而 `curl` v4/经代理均 200、独立 Chromium 也能开。
- 排查路径（系统层先自证清白）：`Resolve-DnsName` 看该域名 AAAA 是否为空 → `curl -4/-6/经代理` 三通路 → 开一个
  **独立/无痕**浏览器对照 → 若独立浏览器正常，就是该 Chrome 实例的站点状态（缓存/连接复用/扩展）：先 `Ctrl+Shift+N` 无痕对照，
  再清该站站点数据（`chrome://settings/content/all`）；仍不行用 `chrome://net-export/` 抓一份日志定位。
- 注意：FlushDNS 不会清 Chrome 自己的缓存（Chrome 重启一次才会丢内存态）。

### 校内域名走校内解析器（2026-09-23 实测，含更正）
- 现象：镜像站 `mirrors.gdut.edu.cn` 右侧「域名选择」组件探测失败/空白（`mirrors4/6.gdut.edu.cn`）。
- 处置：`Merge.yaml` → `dns.nameserver-policy` 加 `'+.gdut.edu.cn': ['10.1.3.38']`（10.1.3.38 在 TUN 排除段 10/8 内），
  组件随即恢复；镜像可走原生 v6 `2001:da8:2018:f666::6666`（curl -6 200 / 18ms）。
- **更正**（当晚 dnspyre + 绑源直查，临时关 TUN `dns-hijack` 复测）：AliDNS `223.5.5.5`、DNSPod `119.29.29.29`、
- **测完必须 `ipconfig /flushdns`**：关劫持的窗口里系统会向校园 DNS 查到 `*.qq.com` 的 AAAA 并缓存，
  浏览器随后去撞走不通的腾讯 v6 → 页面报 `ERR_CONNECTION_CLOSED`（2026-09-23 实测复发一次）。
  处理：`ipconfig /flushdns` + 重启浏览器（Chrome 另有自己的 DNS/连接缓存：`chrome://net-internals/#dns` 清 host cache、`#sockets` 清 socket pools）。
- 测量口径（2026-09-23 晚，dnspyre）：明文 UDP 对比**必须临时把 TUN `dns-hijack` 置空**（该键属 Verge GUI 托管，
  改 Merge 无效；改 `…clash-verge-rev\config.yaml` 后重启 Verge，测完还原），否则打到的是 mihomo 自己（0ms/答案同源）；
  dnspyre 无绑源选项（绑 PPP 源地址的垫层对公网 IP 不生效）。加密路径（DoT 853/850）不经 :53，不受劫持，可直接测。
  udns `42.194.232.31` 的**明文**查询也能解出 `mirrors4/6.gdut.edu.cn` —— 早先"校内记录只有校园 DNS 有"**不成立**；
  当时症状更像 mihomo 的 DoT/DoH 解析链路或其 DNS 缓存问题（重启 + 策略后恢复）。
  保留该条策略的理由改为"显式归属"：校内域名固定用校内解析器，不依赖公网解析器对校内记录的态度。
- 测速对照（每台 360 查询，A+AAAA×9 域名×10 轮×2 并发）：明文 UDP 七台都 0–1ms（校内 4 台 / AliDNS / DNSPod / udns）；
  实际加密链路 AliDNS DoT:853 ≈6ms、udns DoT:850 ≈8ms（p99 13/91ms）——差距主要是 TLS 开销。
  被墙域名的 A 各家返回**不同**的污染 IP（google: 185.45.x/157.240.x/69.171.x…），都不可信。
### 微信卡顿 = CN v6 路由错配（2026-09-23 实测）
- 现象：微信（`Weixin`/`WeChatAppEx`）连接源地址是 TUN 的 `fdfe:dcba:9876::1`，目标是腾讯 v6 `2402:4e00:a2:f0::9:443`。
- 实测三条路径：直连腾讯 v6 = 3/3 超时（ICMP 100% 丢，校园 v6 到不了该段）；走节点 = 200 但 TLS 82ms/TTFB 169ms；CN 直连基准（百度 v6/v4）= TLS 30ms / TTFB 41ms。即"兜底 MATCH,Final 把不可达的 CN v6 丢给节点"，微信因此长轮询全程 169ms。
- 处置（最终）：`Merge.yaml` → `dns.nameserver-policy` 把腾讯/微信域名指向校园 DNS `10.1.3.38`；实测（2026-09-23 晚）**四台校园 DNS 都会返回 AAAA**，所以压制并非来自'服务器不回 AAAA'——更正早先记录。效果仍已验证：mihomo 与系统层对 `*.qq.com` 的 AAAA 为空、微信走 v4（20-40ms）。机制疑为 mihomo `fallback-filter`（本地 geodata 未把 `2402:4e00::` 标为 CN → 视为污染答案 → 走 fallback，fallback 无 AAAA）→ **待查**。教育网段在订阅 `option.rules`（`rkSjO3zIps3Q.yaml`）里保持 DIRECT。
- 判据备忘：`curl -6 --resolve <域名>:443:[<v6>]` 看 TLS/TTFB 区分"直连 / 走节点 / 不通"（CN 直连 ~30ms TLS，走节点 ~80ms TLS）；`Get-NetTCPConnection -OwningProcess <verge-mihomo>` 看 mihomo 出站到底连的是目标 IP（DIRECT）还是固定境外 IP（代理）。
### NCSI 网络徽标与热点反制（2026-09-16 实测）
- 物理以太网永远显示"无法访问 Internet"（`Get-NetConnectionProfile` → `LocalNetwork`）是双出口拓扑的**真实判定**，不是故障：校园有线 L3 需 PPPoE，物理口直连只有内网。Win11 的 NCSI 由 `netprofm`（Network List Service）承载；"到 Internet 的下一跳"是**全系统选举**，PPP 会话（有效 metric 26）一上线就夺走它。
- `NlaSvc\Parameters\Internet` 的 `ActiveWebProbeHost` 指到本机 + 本机 80 应答 `Microsoft Connect Test`，以太网确能拿到 `ActiveHttpProbeSucceeded`，但 **1–7 秒内必被 `NoRoute` 降回 LocalNetwork**；此后探测持续成功（20s 一次 ×5）也无法恢复。故不伪造徽标（ADR-0008）。NCSI 探测报文特征：`GET /connecttest.txt HTTP/1.1`、`User-Agent: Microsoft NCSI`、`Cache-Control: no-cache`、`Pragma: no-cache`。
- `Microsoft-Windows-NCSI/Analytic` 默认关闭，调试开 `wevtutil sl Microsoft-Windows-NCSI/Analytic /e:true`（交互确认喂 `y`），事后 `/e:false`。
- **不要开 Windows 移动热点（ICS）共享 `gdut`**：AP 正常起（`StartTetheringAsync` Success、`192.168.137.1`、Wi-Fi Direct 适配器 `本地连接* 9/10`），但校园侧 13s 内首次踢线，之后每 ~35s 一踢（重拨即再踢），**没有客户端连上也照踢**；关热点 30s 内恢复稳定（2026-09-16 服务日志三次 `Probe … LinkDown` → 重拨循环）。疑似未授权 AP 检测。

### 给用户的断网窗口操作
- 必须自带回滚块（能独立执行；网络炸了用户侧无 AI 可达）。
