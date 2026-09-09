# ADR-0005：有线/无线双模切换（exclusive / standby）

日期：2026-09-09
状态：已接受

## 背景

设备位限制 1 有线 + 2 无线（MAC + UA 判定）。需求：插线用有线、拔线自动切无线（SSID `gdut` 开放网络 + Dr.COM eportal 网页认证，与有线同一套学号密码）、插回线自动让位。两种运行语义：

- **exclusive**（默认）：平时 WLAN 完全断开（无线位让给手机），有线失联后去抖接管，有线恢复稳定后让位。省设备位，代价是 3~10s 切换空窗。
- **standby**：WLAN 常连常认证，有线断由 OS 路由瞬间接替。零空窗，代价是常驻占一个无线设备位。

## 决策

1. 新增 `wireless` 模块：纯逻辑状态机（Linux 可测）+ WlanAPI/路由 Win32 胶水，克隆 heartbeat actor 形态挂进 runtime。watchdog（有线）零改动——拔线后它照常退避重拨，插回线自动恢复，wireless manager 只看它的快照做接管/让位决策。
2. eportal 认证用绑 WLAN 源 IP 的手搓 HTTP GET（复用 probe.rs 模式），常量全走配置（`portal_url`、`wlan_ac_ip`），凭据复用 DPAPI blob。含密码的 URL 永不落日志。
3. **必须自管两条 /32 主机路由**（portal 主机 + HTTP 探测目标，via WLAN 网关）：Mihomo TUN 覆盖路由会使绑源 socket `ENETUNREACH`（2026-09-08 实测）。增删与 WLAN 会话生命周期绑定，三条出口（让位/切模式/服务停止）全覆盖回滚，启动清残留。
4. standby 模式把 WLAN 接口 metric 压到 10（低于物理口自动 metric ~25），保证有线断后新连接真的走 WLAN 而非被墙的物理 DHCP 口；离开 standby 还原。配置可关（`standby_metric = 0`）。
5. 模式经 IPC `SetMode` 运行时切换并持久化 config.toml；拔线判定双保险：以太网 link 状态（秒级）为主、watchdog 失联持续去抖（防"线在会话死"）为辅。
6. 不依赖 logout（上游已知不可用）：让位 = 断 WLAN 关联。

## 后果

- exclusive 有 3~10s 空窗（关联 + DHCP + portal），不可避免；standby 无。
- wireless 探针沿用两级判定（ICMP 网关 on-link + HTTP 经 /32 路由），被踢自动重登（幂等）。
- Session 0 里 WlanConnect 若不可用，fallback 到服务内 `netsh wlan connect`（后端可切换）。
- 用户侧 TUN 配置（`interface-name: gdut`）在无线期失效，属用户配置，本程序只文档提示不代管。
