# gdut-net

广东工业大学（大学城校区）**有线网第三方认证客户端**：单个 Rust 二进制，以 Windows 服务方式守护 PPPoE 拨号，掉线自动重拨；可选 Dr.COM 心跳兼容模式；与 Clash TUN / Tailscale（wintun）等虚拟网卡完全共存（一切拨号与探测流量显式绑定物理网卡）。

替代官方 Dr.COM 客户端，不含 LSP/npf 注入。

## 安装（管理员 PowerShell）

```powershell
# 下载或构建得到 gdut-net.exe 后：
.\gdut-net.exe install          # 交互输入校园网密码（或 --password-stdin 脚本化）
net start gdut-net
```

`install` 会依次：

1. 校验管理员权限（非管理员直接报错）
2. 提示输入校园网密码（DPAPI 加密存入配置，不明文落盘）；既有配置存在则复用学号
3. 创建 PPPoE 拨号条目 `gdut`（`C:\ProgramData\gdut-net\gdut.pbk`）
4. 注册 Windows 服务 `gdut-net`（自启动；失败自动重启 5s/30s/60s，24h 重置）
5. 注册事件日志源（供 `log.event_log = true` 时 warn/error 同步进事件查看器）

启动后服务自动拨号；掉线按指数退避自动重拨（1s 起步、上限 5 分钟；认证失败 691 固定 10 分钟），连续失败 ≥10 分钟弹系统 Toast。

## 卸载

```powershell
.\gdut-net.exe uninstall            # 停止并删除服务、事件源、entropy 键
.\gdut-net.exe uninstall --purge    # 额外删除 C:\ProgramData\gdut-net（配置+日志）
```

每步幂等：重复执行安全；`--purge` 有目录名防护（仅删名为 `gdut-net` 的目录）。

## 配置

`C:\ProgramData\gdut-net\config.toml`（TOML，install 后生成）：

```toml
# 大学城认证服务器 10.0.3.2；龙洞/东风路为 10.0.3.6
# 心跳默认关闭；开启前必须抓包验证（见 ADR-0002）

[account]
student_id = "你的学号"
password_blob = "…DPAPI 密文（install 自动写入）"

[dial]
entry_name = "gdut"                      # 拨号条目名
pbk_path = "C:\\ProgramData\\gdut-net\\gdut.pbk"
interface = ""                           # 物理网卡 FriendlyName；空 = 自动选择
probe_interval_secs = 30                 # 探针间隔（下限 5）
http_probe_url = "http://9.9.9.9"        # HTTP 复核探测地址

[heartbeat]
enabled = false                          # 兼容模式开关（默认关）
module = "gdut"                          # 仅支持 "gdut"
server = "10.0.3.2"                      # 大学城；龙洞/东风路 10.0.3.6
port = 61440
interval_secs = 20

[log]
dir = "C:\\ProgramData\\gdut-net\\logs"
max_size_mb = 5                          # 单文件滚动阈值
rotate_keep = 5                          # 保留的历史份数
event_log = false                        # warn/error 镜像到 Windows 事件日志
```

字段说明：

| 字段 | 含义 |
|---|---|
| `account.student_id` | 学号（认证用户名） |
| `account.password_blob` | DPAPI 加密密码（machine 级；install 负责，勿手改） |
| `dial.entry_name` / `dial.pbk_path` | RAS 电话簿条目与路径 |
| `dial.interface` | 强制指定物理网卡；多网卡或自动识别错时填写 |
| `dial.probe_interval_secs` | 掉线探针周期 |
| `dial.http_probe_url` | 网关 ICMP 不通时的 HTTP 复核（判定是否被服务器踢） |
| `heartbeat.*` | Dr.COM 心跳兼容模式（默认关，见下） |
| `log.*` | 文件日志目录、按大小滚动、保留份数、事件日志镜像 |

### 心跳兼容模式（默认关闭）

服务器若开启心跳校验，直拨会话会被周期性踢掉，此时需开启 `heartbeat.enabled = true`（发送 Dr.COM keepalive 报文，UDP 61440，20s 一轮）。

**风险提示**：心跳常量（服务器 IP、flag、seed 校验方式）年代久远，且 GDUT 变体与 drcom-generic P 版不同——**开启前必须在校园网内抓包验证**（Wireshark 过滤 `udp.port == 61440`，对照官方客户端行为），详见 [ADR-0002](docs/adr/0002-heartbeat-gdut-variant.md)。

本机 61440 端口被官方客户端占用时兼容模式不可用，服务日志会报错（60s 后重试，非静默）。

## 真机验收清单

前提：Windows 11 x64 物理机，校园网有线接入。

### 部署与基础功能

```powershell
# 管理员 PowerShell，gdut-net.exe 所在目录
.\gdut-net.exe install            # 输入密码（学号为空时也会提示输入学号）
net start gdut-net
Get-Content C:\ProgramData\gdut-net\logs\*.log -Wait   # 应出现"拨号成功，会话建立"
net stop gdut-net                 # 优雅停止，日志"收到停止信号"
net start gdut-net                # 再次启动自动重拨成功
```

### 验收标准 1-5 对照

| # | 标准 | 验证方法 |
|---|---|---|
| 1 | 全新 Win11：安装 → 自动拨号 → 断网自动重连 | 上面部署步骤；然后拔网线 10s 再插回（或 `rasdial gdut /disconnect`），日志应出现"判定掉线"→ 指数退避重拨 →"重拨成功，会话恢复"，全程无人工干预 |
| 2 | TUN/Tailscale 共存，重启后重拨成功 | 安装 Clash TUN（TUN 模式）与 Tailscale 并确认 `route print` 出现虚拟路由；`net stop gdut-net && net start gdut-net`，重拨应成功且不干扰 TUN 路由；重启整机后服务自启、会话自动恢复 |
| 3 | 内存 <50MB / 24h 无泄漏 | 任务管理器观察 `gdut-net.exe`（服务进程）工作集 <50MB；挂机 24h 后工作集无明显增长（±5MB 内） |
| 4 | 关心跳 72h 不掉线；开心跳抓包验证 | `heartbeat.enabled=false` 挂机 72h，日志无"判定掉线"（或仅极少数且自动恢复）；需要验证兼容模式时 `enabled=true`，Wireshark 在**物理网卡**抓 `udp.port==61440`，应看到 20s 周期 keepalive 且端口/报文与官方客户端一致 |
| 5 | 卸载干净 | `.\gdut-net.exe uninstall --purge` 后：`sc.exe query gdut-net` → 1060（不存在）；`reg query "HKLM\SYSTEM\CurrentControlSet\Services\EventLog\Application\gdut-net"` → 拒绝访问/不存在；`reg query "HKLM\SOFTWARE\gdut-net"` → 不存在；`C:\ProgramData\gdut-net` 已删除 |

### 服务与事件日志检查

```powershell
sc.exe qfailure gdut-net         # 3 段恢复：5000/30000/60000 ms，重置期 86400 秒
sc.exe qc gdut-net               # AUTO_START，binPath 含 --config 与 run
reg query "HKLM\SYSTEM\CurrentControlSet\Services\EventLog\Application\gdut-net"
                                 # EventMessageFile=%SystemRoot%\System32\netmsg.dll（REG_EXPAND_SZ）
                                 # TypesSupported=0x7
```

`log.event_log = true` 时：事件查看器 → Windows 日志 → 应用程序，来源 `gdut-net`，warn/error 应同步出现（如"认证失败(691)"）。

### 托盘与状态（用户会话）

```powershell
.\gdut-net.exe tray      # 托盘青色图标；菜单显示状态/立即重拨/详情面板/退出
.\gdut-net.exe status    # 终端打印：状态、在线时长、IP、掉线原因、重拨次数、心跳
```

## 开发

```bash
cargo test                                  # 纯逻辑单测（Linux 可跑）
cargo clippy -- -D warnings                 # 静态检查
cargo fmt --check                           # 格式检查
cargo check --target x86_64-pc-windows-msvc # Windows 平台代码编译验证（Linux 开发机）
cargo build --release                       # 发布构建（Windows 上产出 gdut-net.exe）
```

CI（`.github/workflows/ci.yml`）：`linux-test`（test + clippy + fmt）、`windows-build`（test + clippy + release 构建 + 打包 `gdut-net-x86_64.zip` artifact），push 到 main 与全部 PR 触发。

架构与领域词汇见 [CONTEXT.md](CONTEXT.md)；设计决策见 [docs/adr/](docs/adr/)（心跳变体取舍 ADR-0002、两级掉线探测 ADR-0003、服务/托盘分进程 ADR-0001、托盘图标技术选型 ADR-0004）。

## License

仅供学习交流，未含任何官方客户端代码；Dr.COM 协议细节来自社区逆向成果。
