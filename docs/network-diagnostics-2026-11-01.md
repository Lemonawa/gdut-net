# 2026-11-01：首次 WLAN 计费周期与 DNS

## 首次 WLAN 登录

用户先手动在 `10.0.3.2` 网页确认开启周期，网络才恢复。因此，11:29:50、11:30:00、11:30:20 的“正在进入新计费周期，请等待5分钟后再登录”只能说明当时服务器仍拒绝普通登录，不能说明客户端已经完成周期确认。

已查阅 `lin-snow/GDUT-Login`、`GDUTMeow/GDUT-Wlan-Login`、`ymxc152/gdut-auto-login`、`chonqin/linux-gdut-campus-network-login` 的登录实现，均没有发现该确认流程。找到的对应实现是 [FUjr/gdut_network 的 try_login.sh](https://github.com/FUjr/gdut_network/blob/c0dc9519100a3afd736b4ff8e0c3e12f7ff769b9/luci/luci-app-gdutwifi/root/usr/share/gdutwifi/try_login.sh)。它提供 `custom/operator` 的确认请求；未照搬其脚本代码。

随后直接读取校园当前 [a58.js](http://10.0.3.2/a58.js) 与 [a43.js](http://10.0.3.2/a43.js)，确认网页行为：

1. 同源 `:801/eportal/portal/custom/operator`，参数 `account`、`r=0`，查询周期门禁。
2. `code="1",result="fail"` 时网页弹确认框；点击继续提交同一端点 `r=1`。
3. 等待两次 2s，再调用普通登录。
4. `r=0` 返回 `ok` 时直接普通登录。

本实现更严格：查询 HTTP/格式/业务码异常不发送确认或密码；确认必须 HTTP 200 且 `code=1,result=ok` 才继续。账号参数 URL 编码，固定同源，不使用回复里的地址。三个请求都绑定调用方 WLAN 源 IP，沿用既有 UA；单请求 8s、整个流程 32s，停止/让位取消 future 即停止后续动作。服务与 `wireless test` 共用同一实现。日志不保存账号、密码、query、任意服务器文本，GUI 保留既有错误显示。

2026-11-01 真机只读查询得到 HTTP 200、`code="1",result="ok"`。本周期已由用户手动开启，没有为了测试再次发 `r=1`。已用本地 HTTP 模拟覆盖正常周期、首次确认顺序与 4s 等待、拒绝/坏包/HTTP 失败不继续、取消后不登录。真实跨周期分支待下一周期验收。

## DNS 准确性

测试使用本机物理 PPP 地址与接口绑定确认直连路径；代理路径显式经本机 mixed port 7890。DoH/DoT 均保留证书校验。以下是本次样本结果，不代表对所有未签名域名或未来状态的绝对保证。

- uDNS 广州 `42.194.232.31` 的 850/851 端口 TCP 可达，但 Windows SslStream 和 Linux OpenSSL 均拒绝证书。证书有效期至 2027-01-22；OpenSSL 报无法找到上级签发证书。这是当前这两个加密端点的证书链问题，不是已证实全网 uDNS 下线。
- AliDNS、DNSPod 对 `dns.sb/A` 返回 `127.0.0.1`；AliDNS 对 AAAA 返回 `::1`，对 `www.google.com/AAAA` 返回 `2001::1`。加密传输不保证解析器上游答案正确。把 AliDNS 同时放入 primary 和 fallback 不能独立纠偏，重复地址也不增加冗余。
- Cloudflare、Google 经代理对 `dns.sb` 的 A/AAAA 答案集合一致，均给出 AD=1，分别是 `185.222.222.222/45.11.45.11` 与 `2a09::/2a11::`。`cloudflare.com` 的 A/AAAA 也给出 AD=1。
- 两家均对签名损坏的 `dnssec-failed.org` 返回 SERVFAIL，对 `.invalid` 测试名返回 NXDOMAIN、AD=1。AD 是认证 DoH 上游的验证声明，本次客户端没有另行实现 DNSSEC 验证器。
- `www.google.com` 没有 AD，两个独立上游返回相同 A/AAAA 集合；仍需结合目标 HTTPS 身份验证，不能单凭地址看起来正常断言所有域名准确。
- Google 和 Cloudflare 本次物理直连超时或重置，不能将其当作直连公共 DNS 替换。须指定现有代理组，并用独立 `proxy-server-nameserver` 解析节点，避免 DNS/代理依赖环。
- 公共 DNS 对 `mirrors4/6.gdut.edu.cn` 返回 NXDOMAIN，而校园解析器能给出记录，因此必须保留校园策略。

候选方案：默认查询经代理的 Cloudflare/Google；明确的国内域名通过 `geosite:cn` 使用国内解析器；保留校园域名策略。腾讯域名在校内明确使用 `udp://10.1.3.38:53#disable-ipv6=true`，不再依赖旧注释里推测的 fallback 机制。注意不带 `=true` 的片段参数本次曾导致失败，不能仅凭配置语法检查通过就上线。

候选在安装的 Mihomo v1.19.32 中以 DNS-only 独立实例测试：TUN 关闭、只监听 127.0.0.1:11053，不改生产配置；测试结束自动关闭该实例。**策略顺序必须是具体校园/腾讯域名在前，`geosite:cn` 在后**：反过来会先匹配国内集合，校园镜像退回公共 NXDOMAIN、腾讯 AAAA 也不能抑制；本次已复现并修正。

最终候选通过公共 A/AAAA、腾讯 A 正常且 AAAA 空答案、校园 mirrors4/mirrors6、DNSSEC 坏签名及 NXDOMAIN 检查。将解析到的地址固定用于对应域名的 HTTPS 连接，Google、DNS.SB、百度、腾讯均通过证书验证。Google/百度 HTTP 200，DNS.SB 403、腾讯 501；后两者只证明目标 TLS 身份正确，不能据此声称网页业务访问成功。

产物为 `dist/Merge.dns-verified.yaml`（完整配置，保留回家/回校脚本的标记块）、`dist/dns-validation-2026-11-01.json`（隔离 Mihomo 应答）及 `dist/dns-upstreams-2026-11-01.json`（两个独立上游与 AliDNS 对照）。默认公共查询依赖 `Final` 的可用代理。初次交付未改生产；用户随后授权部署，实际结果见文末。

参考：[Mihomo DNS 配置](https://wiki.metacubex.one/config/dns/)、[Google DNS 安全说明](https://developers.google.com/speed/public-dns/docs/security)、[Cloudflare DoH](https://developers.cloudflare.com/1.1.1.1/encryption/dns-over-https/make-api-requests/)。

## FlClash 作为替代客户端

结论：可以作为候选，建议从 v0.9.2 做隔离试用；现有证据不足以直接替换本机主用客户端。这是版本/源码与本机只读状态评估，没有启动或安装 FlClash，也没有宣称完成真机 TUN 切换验收。

核对到的最新稳定版是 [v0.9.2（2026-10-24）](https://github.com/chen08209/FlClash/releases/tag/v0.9.2)，内核 mihomo v1.19.33。相较当前 Verge 的 v1.19.32，内核版本略新，并非完全不同的网络实现。发布说明包含网络变化时清理 DNS 缓存、DNS 手动清理、Windows 系统代理处理改进与可选覆写项。

可取之处：

- [v0.9.2 task.dart](https://github.com/chen08209/FlClash/blob/v0.9.2/lib/common/task.dart) 明确启用 TUN 自动检测接口，`interfaceNameMode.clear` 清空固定接口绑定，适合本机 PPP/WLAN 按 metric 自动选出口。必须保持 clear，不设固定 `gdut`。
- 支持 DNS/TUN 配置、脚本覆写、MTU 与排除路由；可以表达本机 redir-host、IPv6、1480 MTU、RFC1918/Parsec/家庭公网绕行要求。[v0.9.1](https://github.com/chen08209/FlClash/releases/tag/v0.9.1) 起 TUN 按字段覆盖；迁移时须检查生成配置，不能仅检查订阅原文。
- [v0.9.2 proxy.dart](https://github.com/chen08209/FlClash/blob/v0.9.2/lib/common/proxy.dart) 将代理操作串行化，停止操作只撤销本次运行自己设置过的代理。旧的关机代理残留问题由作者在 [#2083](https://github.com/chen08209/FlClash/issues/2083) 标为 v0.8.97 修复。以上与本仓库 9 月的旧版真机经验并不等同，不能以旧经验断言新版永远不可用。

尚待验证的阻碍：

- [#2733](https://github.com/chen08209/FlClash/issues/2733) 记录 v0.9.1 在频繁切换 Wi-Fi 后 TUN 不再创建、mixed port 不监听。2026-11-01 查询仍 open，作者仅要求进一步日志；v0.9.2 发布说明没有明确标注该问题已解决。这是单个环境的故障报告，不是本机已复现，但场景与无线接管相关，迁移验收必须覆盖。
- 本机 `clash-campus-dns.ps1` 只编辑 Verge 的 `profiles/Merge.yaml`。FlClash 不会自动读取该文件；其订阅覆写、回家/回校校园 DNS 开关必须另做适配，否则回家后仍可能卡在不可达校园 DNS。
- 当前只读检查没有发现 FlClash/FlClashCore 进程或 FlClash 命名的 SCM 服务，注册表 `ProxyEnable=0`。因此本次没有证据表明旧 Helper 仍在干扰代理，也没有证明新版 Helper 在本机不会复发。

试用标准：先备份并准备独立回退，只运行一个 TUN；系统代理先关闭并核对注册表。检查最终 DNS/路由/接口配置，再完成有线 → WLAN → 有线、睡眠恢复、重启/退出后代理清理、回家/回校 DNS 切换与 Parsec/Tailscale 测试。FlClash 负责代理流量；学校的周期确认仍由 gdut-net 完成，uDNS 证书链仍需服务端修复或换上游。

## 交付状态

自动周期确认已接入服务和无线体检，完整 `cargo test`、Linux Clippy、Windows `--all-targets` Clippy、格式检查通过，交叉 release 构建和单文件打包成功，产物 `dist/gdut-net-setup.exe`。初次交付未部署；用户随后授权执行，结果如下。真实跨周期自动确认仍待下次周期验收，FlClash 未安装或切换。

## 授权部署结果（北京时间）

- 2026-11-01 15:10:37，经既有 `gdut-switch` 任务开始安装；桌面回滚目标先替换为当时正在运行的准确版本，完整备份保留在桌面 `gdut-net/deploy-20261101-151013/`。
- 15:10:43 新服务拨号成功，15:11:59 通过 75s 稳定检查与 HTTP 204 检查。已核对账号密文、无线配置未变，安装后的主程序 SHA256 为 `3fcc2a35e9feb59765c295e6fee9beaea97cf2bfb1c8d9b45857bef779c2d309`，与构建产物相同；普通用户会话托盘已恢复。
- DNS 首次尝试在 HTTPS 检查调用 Windows curl 时失败，于 15:12:36 自动恢复原 Merge.yaml 并重启 Verge。DNS 策略本身的前置检查已过，不能把验证器失败说成解析器失败。
- HTTPS 检查改用已单独验证的 .NET TCP CONNECT + SslStream，固定解析所得 Google IP，保留 Windows 证书校验并检查 HTTP 200；连接、TLS 和响应头均有超时。第二次部署有独立的 210s 回退进程守护。
- 15:14:39、15:15:00 两轮生产检查通过，中间观察 20s：公共 A/AAAA、腾讯仅 IPv4、校园镜像、DNSSEC 坏签名、NXDOMAIN、固定 IP 的 HTTPS、系统代理关闭。15:15:00 正式成功；回退守护和部署进程均已退出。
- 清理 Windows DNS 缓存后，系统 `getaddrinfo` 对 `dns.sb` 返回 `185.222.222.222/45.11.45.11/2a09::/2a11::`；对 `www.qq.com` 只返回 IPv4。活动 Merge.yaml 与已验证配置的 SHA256 一致。服务后续 ICMP/HTTP 探针持续 Alive。
- 离线回退入口为备份目录里的 `rollback-all.bat`：先恢复原 DNS，再把旧安装包放回桌面工具包并触发现有 `gdut-switch`，不依赖 AI 或外网。`dns-attempt2/deploy-dns.ps1 -Mode Rollback` 可只恢复 DNS。保留部署备份与回退件，不删除。
