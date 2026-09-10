---
version: 1
slug: "src-tray-gui-rs"
primary_target: "src/tray/gui.rs"
related_targets: []
---

# Surface — 日常状态窗口（src/tray/gui.rs）

Mode: Operate
Audience: 作者与安装 gdut-net 的 GDUT 学生；宿舍桌面、中文 Windows，断线后瞥一眼的状态场景。
Job: 一眼看懂"我的网现在怎么样"，一键处理；不需要读文档。
Action/task: 看状态（已连接 / 重拨中 / 认证失败 / 无线接管 / 服务未运行）、立即重拨、切模式、修改账号密码、打开日志目录。
Proof/content: 实时快照（状态、出口、IP、在线时长、上次掉线、心跳、事件环）、版本号；不展示密码。
Constraints: 单窗口 460×540、关窗=隐藏、500ms 刷新、egui glow、系统 CJK 字体、不发网络请求（命令走 IPC）。

Chosen direction: 校园一卡通 / 圈存机（卡面 + 票纸小票 + 读卡灯）。
Memorable moment: 状态章——状态变化时顶部卡面重盖一枚章；事件以"小票流水"一行行印出。

## Direction contract

THESIS: 你的联网资格是一张卡：窗口是卡面加圈存小票，状态像读卡灯一样只有四种，绝不冒充别的信息。
OWN-WORLD: 卡蓝 #1D4E9E、票纸白 #F4F1E8、墨黑 #1A1A1A、读卡绿 #2F9E63、朱红 #C2402F；圆形卡面区、虚线小票行、等宽数字、印章色块；无渐变、无阴影堆砌。
STORY: 打开即认可信状态与出口 → 一键重拨 / 切模式 → 需要改密码时进安装器 → 关闭，窗口隐藏。
FIRST VIEWPORT: 顶部卡面（卡蓝地、学号如卡号、状态章），其下票纸白小票：大字状态、虚线分隔的字段行、事件流水；底部圈存机软键排。
FORM: 校园一卡通（自选排序 #1；掷签 key f8e9de4f），code-led 构建（无图像生成）。
FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
