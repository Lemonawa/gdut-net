# 无线接管与双模切换（Wireless Takeover & Mode Switch）设计

日期：2026-09-09
状态：已获口头批准，待 spec 评审
关联 ADR：ADR-0001（服务/托盘分离）、ADR-0003（两级探测）、ADR-0005（无线双模）、ADR-0006（托盘重做）

## 1. 目标与非目标

**目标**：插线用有线，拔线自动切无线（SSID `gdut` + eportal 认证），插回线自动让位；支持两种模式（exclusive / standby）运行时可切；托盘 UI 重做。

**非目标**：
- 不做无线网页认证的"统一身份认证"生态对接（CONTEXT.md 已声明与本客户端无关的部分不扩）。
- 不做 logout 依赖（上游 logout 接口已知不可用）；拆除 = 断 WLAN 关联。
- 不管理用户的 Mihomo/Verge TUN 配置（无线期 `interface-name: gdut` 失效属用户配置，只文档提示）。
- 不做 802.1X / WPA-Enterprise；只用现有开放网络 + 全用户 profile。

## 2. 背景事实（已实测/已核实）

- SSID `gdut` 开放认证，全用户 profile `gdut` 已存在本机。关联后 DHCP 得 `10.43.0.0/16`，网关 `10.43.0.1`。
- Portal 协议（lin-snow/GDUT-Login 2024 实证）：`GET http://10.0.3.2:801/eportal/portal/login?callback=dr1004&login_method=1&user_account=<学号>&user_password=<密码>&wlan_user_ip=<本机IP>&wlan_user_ipv6=&wlan_user_mac=000000000000&wlan_ac_ip=172.16.254.2&wlan_ac_name=&jsVersion=4.1.3&terminal_type=2&lang=zh-cn&v=2041`。回包 JSONP：`dr1004({"result":"1",...})`，`result=="1"` 为成功。无 token、无 JS 加密、MAC 可填全零。
- Mihomo TUN 覆盖路由（约百条，metric 0）使绑 WLAN 源 IP 的 socket 出网 `ENETUNREACH`（强主机模型）。**不加 /32 主机路由则 portal 请求与 HTTP 探测都发不出去。**
- 设备位限制：1 有线 + 2 无线（按 MAC + UA 判定）。
- 现有 egui 黑屏根因是 `default-features = false` 关掉 `default_fonts`（详见 ADR-0006），非核显问题。

## 3. 配置

```toml
[wireless]
enabled = true                       # 总开关；false 时 wireless manager 不启动
mode = "exclusive"                   # exclusive | standby
ssid = "gdut"
profile = "gdut"                     # Windows WLAN profile 名（须已存在、全用户）
portal_url = "http://10.0.3.2:801/eportal/portal/login"
wlan_ac_ip = "172.16.254.2"          # 大学城；龙洞/东风路未验证，走配置改
probe_host = "223.5.5.5"             # 无线 HTTP 探测目标（须 IPv4 字面量）
takeover_after_secs = 8              # exclusive：有线失联多久后接管
release_after_secs = 10              # exclusive：有线恢复稳定多久后让位
standby_metric = 10                  # standby：压制 WLAN 接口 metric 的目标值；0 = 不压
```

规则：
- `Config::validate()` 校验 mode 枚举、portal_url 用扩展版 `parse_http_probe_target`（接受带 query 的 URL）、probe_host 为 IPv4 字面量。
- 凭据复用 `account.student_id` / `account.password_blob`（DPAPI），不新增存储。
- `install` 不创建 WLAN profile（已存在；缺失时 wireless manager 报 `NoProfile` 错误并提示手工连一次生成）。

## 4. 模块结构

```
src/wireless/mod.rs        纯逻辑（Linux 可测）：状态机、决策、参数
src/wireless/portal.rs     纯逻辑：login URL 构建（percent-encode）、JSONP 解析、结果分类
src/wireless/wlan.rs       #[cfg(windows)] WlanAPI 胶水：枚举/关联状态/连接/断开
src/wireless/routes.rs     #[cfg(windows)] /32 主机路由增删 + WLAN 接口 metric 压制/还原
```

`lib.rs` 注册 `wireless`。纯逻辑与 Win32 胶水分层同 `probe.rs` 先例。

## 5. 状态机（纯逻辑，喂事件序列测试）

```rust
enum WPhase { Off, WaitLink, Joining, Authing, Online, Error(String) }
enum WInput {
    Tick,                          // 2s 周期事件
    LinkDown, LinkUp,              // 以太网 ifOperStatus
    WiredSession(bool),            // watchdog 是否 Connected
    WlanState(...),                // 关联/IP 事件（由 wlan.rs 轮询归一化）
    Probe(ProbeVerdict),
    AuthReply(Result),
    SetMode(NetMode),
}
```

**exclusive**：
- 有线会话 Connected 且持续 `release_after_secs` → 若 WLAN 在线：撤销（断路由/还原 metric/WlanDisconnect）→ `Off`。
- 以太网 LinkDown 或有线会话非 Connected 持续 `takeover_after_secs` → 启动接管：`Joining`（WlanConnect profile）→ 等 IPv4（≤20s，2s 轮询）→ `Authing`（加路由 → portal login）→ 成功 `Online` / 失败退避重试（5s、15s、30s 封顶，认证失败字面错误如 `result!=1` 固定 30s 重试并记 `last_error`）。
- `Online` 期间每 `dial.probe_interval_secs` 探测（ICMP 网关 + HTTP probe_host，绑 WLAN 源 IP）；`Kicked` → 重新 `Authing`；`LinkDown`（WLAN 自己掉了）→ 回 `Joining`。

**standby**：
- 不管有线状态：始终维持 `Online`（同样的 Joining→Authing→Online 循环 + 探测保活）。
- 进入 standby 时若 `standby_metric > 0`：压 WLAN 接口 metric 到该值（记原值）；离开时还原。
- wired 断的"瞬间接替"由 OS 路由自然完成（WLAN metric 已压制）。

**模式切换**（IPC `SetMode`，立即生效 + 持久化 config.toml）：
- → exclusive：若当前有线健康，走让位路径；有线不健康则维持无线。
- → standby：启动常连循环。

撤销路径（三条出口全覆盖）：WLAN 让位、模式切换、服务停止（CancellationToken child token 清理）；启动时先清残留路由（按特征匹配：我们加的 /32 且 via WLAN 网关）。

## 6. 路由与 metric（自包含 + 自动回滚）

- WLAN 会话存活期间维护（`CreateIpForwardEntry2`/`DeleteIpForwardEntry2`，SYSTEM 权限）：
  - `<portal_host>/32 via <wlan_gw> metric 1`
  - `<probe_host>/32 via <wlan_gw> metric 1`
- metric 压制用 `SetIpInterfaceEntry`（`MibUnload`… 否——`SetIpInterfaceEntry` 改 `InterfaceMetric`），进出成对。
- 回滚失败只记日志（/32 路由与 metric 还原是无害残留，下次启动会再清）。
- 不碰物理口/PPP 的任何路由。

## 7. IPC 协议

```rust
Command { Redial, SetMode { mode: NetMode } }        // NetMode: wired_exclusive | wired_plus_standby
StateSnapshot {
  ...,                                               // 原字段不动
  mode: NetMode,                                     // #[serde(default)]
  wireless: WirelessSnapshot,                        // #[serde(default)]
}
struct WirelessSnapshot { phase: String, ip: Option<String>, last_error: Option<String> }
```
- 新字段全部 `#[serde(default)]`；serde 默认忽略未知字段 → 新旧交错升级安全。
- 托盘显示：`Wired: Connected · Wireless: Online 10.43.x.x (standby)`。
- `status` 子命令输出同名两行。

## 8. 托盘重做（详见 ADR-0006）

- 菜单：状态行（disabled）→ 分隔线 → 模式二选一（CheckMenuItem）→ 分隔线 → Redial now / Open panel → 分隔线 → Exit。
- 图标按状态换色（绿=有线通 / 蓝=无线在用 / 黄=退避重试 / 灰=全断），`set_icon` + tooltip 同步。
- 面板：egui glow + `default_fonts`，每次打开起独立线程 `run_native`；内容：双链路状态卡、模式 radio（发 SetMode）、活秒表（`request_repaint_after`）、最近事件尾巴（快照 `events: VecDeque<String>` 最近 20 条，服务端在状态变迁时 push）、Redial 按钮。全英文。

## 9. 事件尾巴（events ring）

服务端在以下时刻 push 一条（带 unix 时间戳 + 英文摘要）：wired 状态变迁、wireless 相位变迁、SetMode、portal 认证成败、路由增删失败、探针判踢。容量 20，随快照全量下发（20 条 × ~100B，成本可忽略）。

## 10. CLI

```
gdut-net wireless test     # 连 gdut → 等 IP → 发一次 portal login → 打印回包原文与解析结果；不动路由、不驻留（现场验证 wlan_ac_ip 等常量）
gdut-net wireless off      # 等价 SetMode(exclusive) + 立即让位（现场排障用）
gdut-net wireless standby  # 等价 SetMode(standby)
```
输出全英文；`test` 不落密码（URL 只打 host+path，query 打码）。

## 11. 测试

- Linux 纯逻辑：URL percent-encode（含特殊字符密码）、JSONP 解析（成功/失败/畸形/超长 callback）、状态机全转移（脚本化事件序列：拔线/插线/被踢/模式切换/认证失败重试上限）、去抖窗口边界、事件环。
- 集成：`tests/wireless_sm.rs` 用 fake WlanControl trait 驱动状态机（同 MockDialer 先例）。
- Win32 胶水：`cargo check/clippy --target x86_64-pc-windows-msvc`（Linux 侧）+ 上机验收脚本（wireless test → 拔线 → 观察 → 插线 → 观察）。
- 托盘：spike 二进制上机过（字体渲染 + 关开 5 次不残留）；正式代码交叉编译验证。

## 12. 风险与开放问题

| 风险 | 对策 |
|---|---|
| Session 0 服务 WlanConnect 全用户 profile | 预期能用；fallback：服务内 CreateProcess `netsh wlan connect`（实现为可切换后端，test 时人工验证哪种可行） |
| portal 会话寿命/是否需主动 keepalive | 探针检踢 + 自动重登已覆盖；观察期调参 |
| `wlan_ac_ip` 等常量漂移 | 全走配置；`wireless test` 一次实测 |
| Mihomo TUN 与 /32 路由共存 | /32 只影响两台主机（portal 与探测目标），TUN 规则丢失这两目标无感 |
| standby 下 NLA 弹"登录网络"提示 | 无害，忽略 |
| 拔线接管期间 Windows 自动连别的 WLAN | profile `gdut` 设置为不自动连不会（开放网络默认手动）；`Joining` 前显式连接目标 profile |

## 13. 验收标准

1. exclusive：拔线 ≤15s 无线可用；插线回 wired Connected 后 ≤20s WLAN 断开。
2. standby：拔线后新 TCP 连接 ≤2s 走 WLAN（curl 验证）；设备位占用=1 无线。
3. 模式切换即时生效且重启保持。
4. 托盘菜单/面板无乱码（全英文）、面板非黑屏、图标随状态变色。
5. 服务重启/停止后无残留 /32 路由、WLAN metric 还原。
