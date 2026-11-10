# 2026-11-10：关闭 TUN 后系统 DNS 失败

用户报告关闭 TUN 后 B 站报 `ERR_NAME_NOT_RESOLVED`。服务探针持续 Alive，注册表 `ProxyEnable=0`，PPP 默认路由有效。

## 复现与修复

通过 Mihomo 本地控制管道短暂关闭 TUN，finally 恢复，并有独立超时恢复进程。关闭时 PPP 没有 IPv4 DNS；物理以太网仍带四台校园 DNS：`222.200.115.251`、`10.1.3.38`、`222.200.115.252`、`10.1.3.34`。

- 系统 `getaddrinfo(www.bilibili.com)` 报找不到主机；Windows curl 报错误 6，百度也失败。
- 清空 Windows DNS 缓存后仍失败，`Resolve-DnsName -DnsOnly` 超时。
- 同一窗口内，`nslookup` 对四台校园 DNS 逐台直查均能返回 B 站地址。因此“其他 up 网卡配置了 DNS”不足以证明 Windows 系统解析可用。
- 用户授权一次 Windows 管理员提权。给活动 PPP 配置 `10.1.3.38`、`222.200.115.251` 后，关闭 TUN 连续三轮系统解析、B 站和百度 HTTPS 200 均通过；随后恢复 TUN 开启。修改有独立的 75s 回退进程，验证通过后取消回退。

源码 `ras::dial_once` 原本在每次成功后调用 `adapter::detach_ppp_dns`，为了恢复 AAAA 清空 PPP DNS。本次移除自动调用，保留 RAS 下发的 DNS。不能为了历史上的 AAAA 问题牺牲 TUN 关闭时的基本解析。保留 PPP DNS 后的原生 AAAA 行为需要单独观察，不能以 TUN 开启时的结果推断。

## 部署

- 完整 `cargo test`、Linux Clippy、Windows `--all-targets` Clippy、`cargo fmt --check`、Windows release 交叉构建与单文件打包通过。首次沙箱测试无法绑定 localhost，放行该权限后全套通过。
- 14:38:10 触发现有 `gdut-switch` 事务，14:38:17 新服务拨号成功；PPP 自动带回 `222.200.115.251`、`10.1.3.38`，没有再被清空。
- 部署后再次关闭 TUN：系统解析 B 站正常，HTTPS 200 约 0.16s；清缓存后重测仍 200，百度也 200。测试结束恢复 TUN 开启。
- 14:39:34 既有部署事务通过 75s 稳定检查，HTTP 204，计划任务结果 0。服务后续探针 Alive。
- 已安装主程序 SHA256：`7df60d8d3a8a8daa06712fd33d80d44a8f83400cba153419c6bf91f6b2f82197`。构建产物与安装路径一致。
- 更新前准确版本与桌面原文件备份在 `C:\Users\Lemonawa\Desktop\gdut-net\dns-fix-20261110-143809\`；部署前已把桌面回滚主程序更新为当时正在运行的准确版本。新安装器同步到桌面部署入口。
