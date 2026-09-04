# Tailscale P2P under gdut-net（校园 PPP 出口）诊断记录 — 2026-09-04

> 结论先行：**校园侧（gdut-net + Verge/Merge.yaml）无需任何改动**。直连失败的根因是
> 「校园 PPP CGNAT（对称、重写端口）× 家路由器（按远端过滤）」双向死锁；
> 修复动作在家里：路由器开 UPnP 或手动转发 UDP 41641 → 192.168.5.11（Mac）。

## 症状

gdut-net 模式下（Verge TUN 开/关均复现）：

- `tailscale ping nimbus-2000`（家 Mac）永远 `via DERP(den)` ≈ 440ms，`direct connection not established`。
- 同环境下 Parsec 可 9ms 直连；tailscale 对 SG VPS（workload-speedypage-sg）可 197ms 直连。
- 对照：两天前用 DrCOM（宿舍 NAT 出口）时 tailscale 可直连；Mac 侧无任何变更。

## 决定性证据（SG VPS 做第三方探针）

```
VPS → Mac:  pong via 113.75.185.47:41641 in 247ms          # 直连 ✓
VPS → TX:   pong via 120.236.177.116:24726 in 194ms        # 直连 ✓（还抖出 v6 面 240e:...:41641）
```

1. **Mac 的 `113.75.185.47:41641` 是真实活面**：家路由器对 41641 端口保持；VPS 用自己真实的
   41641 源端口打洞 → 家路由器放行（匹配 Mac 自己 ping 过的远端）→ Mac 回 pong → 直连成立。
   → Mac 侧完全健康。曾怀疑的 Surge Enhanced Mode（utun7）**不背锅**：macOS tailscaled 绑定
   en0 物理口（IP_BOUND_IF），UDP 不走 utun；tcpdump 里 STUN 出包源即 192.168.5.11.41641。
2. **校园 CGNAT 重写端口**：`120.236.177.116:24726` 是校园 tailscaled 主 socket（本地 41641）
   朝 VPS 方向的真实出口面。外加多 ISP 出口池（120.236.177.116=移动 / 58.248.162.127=电信，
   按目的地分池；同 socket 对不同目的地面不同）= 教科书式对称 NAT。
3. **死锁机理**：家路由器入站过滤按「远端 (IP,端口)」匹配（Mac 只放行自己 ping 过的组合）。
   VPS 的源端口 41641 恰好匹配（Mac ping 的是 VPS 通告的 41641）→ 进得去；
   校园的源端口被 CGNAT 改成随机值（24726…）→ Mac 过滤表无此条 → 校园的洞包全灭。
   反向（Mac → 校园通告面 120.236.177.116:41641）被校园对称 CGNAT 丢弃（无匹配映射）。
   双向都进不去 → 只能 DERP。
4. **DrCOM 时代为何通**：当时校园出口是宿舍 NAT（简单型、端口保持、通告面==真实源），
   Mac 单方面即可打进。换 gdut-net 后唯一活路是「校园先手 + Mac 回 pong」，
   恰好被「CGNAT 重写端口 × 家路由按远端过滤」这对组合断掉。Mac 没动过不矛盾——
   它一直是好端，坏的是组合。
5. DERP 兜底选中 den（丹佛）是次生问题：电信家宽晚高峰到 hkg 路由烂（216ms），
   Mac 的 nearest 变 sfo/den，校园 nearest 是 sin——中继路径绕美。

## 已排除的干扰项（后人勿踩）

- 家 113.75.184.0/20、Parsec 3 个 STUN IP 的 route-exclude（9/4 三板斧）对 tailscale 同样生效且必要
  （tailed 的外层包必须走 PPP 出，已在排除表内）。
- `PROCESS-NAME,tailscaled,DIRECT`（9/1 补丁）工作正常；netcheck `UDP: true`、真面可见。
- ICMP 不可达探针法**不可用**：连已证可达的 SG 对照都静默（CGNAT 不回传 ICMP 差错）。
- PowerShell 源绑定 UDP 测试会被防火墙拦成假阴性；WSL 的 STUN 面数据受 WSL NAT 二次改写污染
  （但方向性结论与 tailscaled 权威面一致）。
- `tailscale debug daemon-logs` 在 Windows 服务模式下无输出（logtap 无日志可挂）；
  看魔法套接字状态用第三方 peer 的 `tailscale ping` 反推更有效。

## 修复（家侧，二选一）

**A. 路由器开 UPnP**（192.168.5.1 管理页 → NAT/UPnP 设置开启）→ Mac 上
`tailscale netcheck` 的 `PortMapping:` 应变为非空 → UPnP 映射通常接受任意来源，
校园被改写的洞包即可直达。**回滚 = 关掉 UPnP，无残留。**

**B. 手动端口转发**（更稳）：UDP 41641 → 192.168.5.11:41641。**回滚 = 删该条转发。**

**验收**（校园侧）：`tailscale ping --c 10 nimbus-2000` 出现
`via 113.75.185.47:41641 in <30ms` 即成（跨省绕行，预期 10–25ms）。
修好后 Mac↔VPS、TX↔VPS、TX↔Mac 三对全直连，DERP 不再登场。

**若不动家路由的替代方案**：在 SG VPS 上跑自建 DERP（derper）并把 tailnet DERPMap 指过去，
兜底从 440ms 降到 ~200ms（治标，不直连）。

## 日常排查速查（校园侧）

- `tailscale netcheck`：`IPv4:` 的 IP 判断出口池（120.236=移动 / 58.248=电信 / 其他=新池）；
  `MappingVariesByDestIP: true` = 对称 NAT；`PortMapping:` 空 = 无 UPnP。
- gdut-net 每次重拨后建议复测一次（换 BRAS 会话可能换池）。
- 判断某 peer 直连状态：`tailscale ping --c 4 <peer>`；`via IP:port` = 直连，`via DERP(xx)` = 中继。
