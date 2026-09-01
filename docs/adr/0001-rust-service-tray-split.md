# Rust + 纯 Windows 服务 + CLI 子命令（托盘另挂用户会话进程）

内存与长驻稳定性是硬验收指标（<50MB、24h 无泄漏），选 Rust。主体以 Windows 服务形态运行（LocalSystem），安装/卸载/诊断走同一二进制的特权 CLI 子命令，延续单二进制哲学。托盘与小 UI 是用户会话内的独立进程，与服务 IPC 通信——服务不承担 UI 职责，UI 崩溃不影响拨号守护。

## Considered Options

- Go：迭代快，但 GC 抖动与 RAS API 结构体手工对齐增加长驻风险。
- 托盘即服务进程：Session 0 无法显示 UI，需跨会话 tricks，违背低内存目标。
