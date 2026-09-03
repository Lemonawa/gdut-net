# 心跳兼容模式以 gdut-drcom 变体为准，非 drcom-generic P 版

需求文档原建议"调研 drcom-generic 的 P 版心跳实现"，但调研（2026-08）发现 GDUT 心跳与认证是分离的：gdut-drcom 变体不需要 challenge/salt/login 登录态，用 8 字节探测包直接从服务器拿 seed/host_ip，四个 UDP 报文（ka1 探测、ka1 96B、ka2 type1、ka2 type3）打 `服务器:61440`（本地也 bind 61440），20 秒一轮。P 版全流程依赖登录 salt，在"无登录态守护进程"里反而走不通。Rust 实现从提炼的协议 spec 出发（常量表 + 报文偏移），不逐行翻译 GPL/AGPL 代码，规避衍生作品问题。

心跳默认关闭；服务器 IP（大学城 10.0.3.2）、keep_alive1_flag（抓包 2a 与 Dialer 6a 矛盾）等常量年代久远，启用兼容模式前必须在校园网内抓包验证。
