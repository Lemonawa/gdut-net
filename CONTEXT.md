# gdut-net

广东工业大学（大学城校区）有线网第三方认证客户端，替代 Dr.COM 官方客户端。核心是 PPPoE 拨号守护，可选的 Dr.COM 心跳兼容模式，且与 TUN 类虚拟网卡完全共存。

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
无线网网页认证所用账号体系，与本客户端无关（明确非目标）。

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

- 心跳相关的一切发包绑定物理适配器，绑定失败（端口 61440 被官方客户端占用）视为兼容模式不可用，报错而非静默。
- "掉线"以流量探测为准，不单看 RAS 状态。
- 双出口下 TUN/代理出站必须显式绑 `gdut`（Mihomo `interface-name: gdut`；`auto-detect-interface` 会跟 metric 0 的物理口走，被墙），TUN MTU≤1400（PPPoE 1480 减开销）。
- WSL 为 mirror 模式，跟随主机路由表；TUN 开 fake-ip 时直连失败是预期，只能走 TUN/代理。
- 查系统代理只信注册表 `HKCU\...\Internet Settings\ProxyEnable`，不信 GUI 开关（前后端脱节）；该值重启不清零；FlClashHelperService（SYSTEM 常驻，FlClash 关了也可能活着）会把它写回 1；Verge 守卫在 OFF 时已停可排除；`clash-verge-service` 不是 SCM 服务（sc 1060），只跑内核不管代理。
- Verge 运行时配置注入点是 `profiles/Merge.yaml`（全局拓展配置），别手改生成的 `clash-verge.yaml`；回滚=删段后重选订阅。
