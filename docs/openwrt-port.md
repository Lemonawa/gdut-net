# 路由器方案调研：把 gdut-net 搬上 OpenWrt（2026-09-04）

> 结论先行：**分三步走，第一步零代码**。先只用 OpenWrt 原生 PPPoE 验证"拨号即通"（躲过识别后不需要任何 Dr.COM 报文）；被踢才做心跳 PoC。已有先例证明整条路走得通，但停更于 2018 年，服务器现状必须实测。

## 已有方案盘点

| 方案 | 性质 | 对本项目的可用性 |
|---|---|---|
| `xnhw01/gdut-drcom-for-openwrt` | GDUT 大学城 5.2.1p 变体，C 实现，OpenWrt ipk + procd，含 PPPoE 拨号选项 | **最强先例**：2017-10~2018-07 真实可用，证明"路由器 PPPoE + gdut 心跳变体"成立。停更 8 年，仅作行为对照基准（GPL，不引用代码，遵守 ADR-0002 洁净室约束） |
| `drcoms/drcom-generic`、`mchome/dogcom` | P/d/x 版通用实现（Python/C） | 不适用：P 版心跳依赖登录 salt，无登录态守护进程走不通（ADR-0002 已否决） |
| 各 HTTP portal 登录脚本（HUTB 等） | "哆点"网页认证 | 与 GDUT 有线 PPPoE 无关 |

## 第 0 步：选型门槛（买之前）

- 任何 [toh.openwrt.org](https://toh.openwrt.org) 官方支持、带 WAN 口的机型即可；百元级 MT7621/MT7981（闲鱼小米 R4A、红米 AX6000 等）或 x86 软路由都行。
- 硬门槛只有两条：
  1. **架构决定交叉编译 target**：mipsel / aarch64 / x86_64 各对应一个 Rust target。嫌麻烦就选 aarch64 或 x86_64。
  2. Flash/RAM 不苛求：Rust musl 静态单二进制预计 <1MB、内存几 MB，心跳守护极轻。8/64 的老机器也够，16/128 舒适。
- **不要为心跳功能预先买单**：第一步不需要跑任何 Dr.COM 报文；心跳是第三步的可选项。最便宜的能跑 OpenWrt 的就行。

## 第 1 步：纯 PPPoE 验证（零代码）

刷机按 toh 型号页走，保留 OEM 救砖通道（多数机型有官方 recovery，刷前确认）。

OpenWrt 侧配置：

1. LuCI → Network → Interfaces → `wan`，Protocol 改 **PPPoE**，填写与 Windows 拨号条目相同的用户名/密码（学号 + 统一认证密码），物理口选 WAN。
2. 删掉/停用 WAN 口上多余的东西（不需要 VLAN 特判、不需要 mwan3）。
3. 无需任何 Dr.COM 组件、无需 Python、无需心跳。

Windows 上那套"双出口隔离 + TUN 绑 `gdut`"的坑在路由器上**不存在**：路由器自己就是网关，全部流量天然走唯一 PPP 出口。

**成功标准**：
- `curl ip.sb`（或 LuCI 首页）返回 10.30.x.x 段地址；
- 内网多设备（NAT 后）都能正常上网，持续 ≥1 天。

**此刻"躲过识别"即成立**：学校侧只见 1 个 WAN MAC、1 条 PPP 会话（多设备判定按 MAC + HTTP UA，CONTEXT.md；NAT 后 MAC 只有一个）。

## 第 2 步：观察期——要不要心跳，让服务器回答

- 不跑任何心跳，观察会话存活：几小时？几天？一直？
- 对策分岔：
  - **一直不掉 → 结案**，世上再无 Dr.COM，路由器就是全部答案；
  - **周期性被踢 → 进第 3 步**，同时顺手在校园网内抓一次官方客户端流量，复核 ADR-0002 警告的常量（大学城服务器 `10.0.3.2`、`keep_alive1_flag` 抓包 `2a` 与 Dialer `6a` 矛盾）。

## 第 3 步：心跳 PoC（仅当被踢证实后才做）

代码侧盘点（2026-09-04 子代理核对）：

- `heartbeat::spec`：纯逻辑（md5/md4/sha1），`tests/heartbeat_spec.rs` 在 Linux CI 全绿，**零改动**。
- `heartbeat::session::run_blocking`：std `UdpSocket` + tokio watch/CancellationToken，无 `#[cfg(windows)]` 门，**今天就能在 Linux 编译**（doc comment 写的 "Windows-only" 是过时描述）。bind `(src_ip, 61440)` 失败即报错的规则自包含在内。
- 唯一移植点：`runtime.rs:246` 的 `adapter::physical_adapter()`（Windows GAA）换成读 OpenWrt 上 `pppoe-wan` 的 IPv4（`ifstatus wan` / rtnetlink / uci get 皆可）。
- 外壳：procd init 脚本 + `hotplug` 在 `ifup` 时拉起/在 `ifdown` 时停；参考 gdut-drcom-for-openwrt 的 procd 集成形态（仅对照行为，不抄代码）。
- 交叉编译：`cargo build --release --target <arch>-unknown-linux-musl`（OpenWrt 用 musl，我们零系统依赖，静态直塞）。

## 风险与对策

| 风险 | 评估 | 对策 |
|---|---|---|
| 服务器 2018→2026 变了（心跳协议改/加校验） | 未知，唯一解法实测 | 第 2 步观察期 + 校园网抓包复核常量 |
| 多设备检测升级为 TTL/深度检测 | 低：HTTPS 已加密，UA 检测只对明文 HTTP 有效；NAT 后 TTL 不统一可能露馅 | 若被识别：`iptables -t mangle` 统一出站 TTL；或全屋流量走路由器自身统一出口 |
| 学号绑定了旧网卡 MAC | 可能首次拨号 691 | 校园网自助系统解绑/换绑（拿 Windows 侧 691 处理经验套用） |
| 宿舍口 MAC 数量限制 | PPPoE 只需线路通，一般无碍 | 实测 |

## 与 Windows 版的关系

Windows 版继续用（在校、宿舍）。路由器版成功后可降级为"家里/家里蹲"无关项，两者共用 `heartbeat::spec` 这一份协议规格，不 fork 协议知识。
