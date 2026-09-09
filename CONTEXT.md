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
用户会话内的常驻 UI 进程，展示会话状态并在守护异常时弹系统通知；与服务 IPC，不参与拨号。
_Avoid_: 界面（泛称）

**双出口 (Dual Egress)**:
校园网同时存在 DHCP 物理口（172.17.x.x，默认路由 metric 0）与 PPP 会话口（`gdut`，10.30.x.x，metric 1）；两者隔离，互联网出站必须走 PPP，家中单出口无此问题。

## Rules

- 无线接管的一切发包（portal 登录、ICMP/HTTP 探针）显式绑 WLAN 适配器源 IP；WLAN 会话存活期间服务自管两条 /32 主机路由（portal 主机 + HTTP 探测目标，via WLAN 网关），否则 Mihomo TUN 覆盖路由下绑源 socket `ENETUNREACH`。增删与让位/切模式/服务停止三条出口绑定，启动清残留。
- 含密码的 portal URL 永不落日志/事件尾巴（打码只留 host+path）。
- 心跳相关的一切发包绑定物理适配器，绑定失败（端口 61440 被官方客户端占用）视为兼容模式不可用，报错而非静默。
- "掉线"以流量探测为准，不单看 RAS 状态。
- 双出口下 TUN/代理出站必须显式绑 `gdut`（Mihomo `interface-name: gdut`；`auto-detect-interface` 会跟 metric 0 的物理口走，被墙），TUN MTU≤1400（PPPoE 1480 减开销）。
- WSL 为 mirror 模式，跟随主机路由表；TUN 开 fake-ip 时直连失败是预期，只能走 TUN/代理。
- 查系统代理只信注册表 `HKCU\...\Internet Settings\ProxyEnable`，不信 GUI 开关（前后端脱节）；该值重启不清零；FlClashHelperService（SYSTEM 常驻，FlClash 关了也可能活着）会把它写回 1；Verge 守卫在 OFF 时已停可排除；`clash-verge-service` 不是 SCM 服务（sc 1060），只跑内核不管代理。
- Verge 运行时配置注入点是 `profiles/Merge.yaml`（全局拓展配置），别手改生成的 `clash-verge.yaml`；回滚=删段后重选订阅。
