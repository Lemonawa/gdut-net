# ADR-0008：不伪造 NCSI 网络徽标

日期：2026-09-16
状态：已接受（本机实测取证）

## 背景

校园有线是双出口拓扑：物理口（DHCP 172.17.x.x）只有校园内网，互联网出站必须走 PPPoE 会话 `gdut`（10.30.x.x）。Windows 的 NCSI 按接口独立探测，以太网因此长期显示"无法访问 Internet"（`LocalNetwork`）——用户会误以为网卡或程序故障，希望"让它显示成已联网"。

## 实验（2026-09-16，本机）

- 把 `HKLM\SYSTEM\CurrentControlSet\Services\NlaSvc\Parameters\Internet` 的 `ActiveWebProbeHost` 指向本机，并在本机 80 端口应答 `Microsoft Connect Test`；开启 `Microsoft-Windows-NCSI/Analytic` 取证。
- 结果：以太网的 HTTP 探测确实成功四次（`ActiveHttpProbeSucceeded`，探测器 `User-Agent: Microsoft NCSI`），但每次都在 **1–7 秒内被 `NoRoute` 降回 `LocalNetwork`**。原因：NCSI 的"到 Internet 的下一跳"是全系统选举——PPP 会话有效 metric 26，物理口 4250，PPP 一在线就夺走它；探测此后仍成功（20s 一次，连续 5 次）也无法恢复。
- 保持探测成功的唯一手段是伪造路由（把 NCSI 参考地址的下一跳塞给物理口）：会让这些地址的真实流量被导入校园网黑洞、依赖未知的目标 IP 集合、微软换探测地址即失效。

## 决策

不伪造 NCSI 状态、不重定向探测地址。以太网显示"无法访问 Internet"保持现状，改以文档答疑（`说明.txt` 常见疑问、`docs/desktop-kit.md`、`CONTEXT.md`）。

## 后果

- 用户问"以太网为什么没有 Internet"时以文档作答；是否在线以 GDUT Net 状态/托盘为准（绿=有线通，蓝=无线在用）。
- 后续"改注册表骗徽标"的提议直接引用本 ADR 与实验记录驳回。
- 同类反制：用 ICS 移动热点共享 `gdut` 会被校园侧反复踢线（见 `CONTEXT.md` 实测陷阱），也不要尝试。
