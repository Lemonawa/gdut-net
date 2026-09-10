---
version: 1
slug: "src-setup-ui-rs"
primary_target: "src/setup/ui.rs"
related_targets: []
---

# Surface — 安装器（src/setup/ui.rs）

Mode: Operate
Audience: 没有命令行经验的 GDUT 学生（含作者）；从发布页双击 setup，一次走完。
Job: 输账号密码 → 装好 → 知道成没成；失败知道下一步。
Action/task: 欢迎（一句话说明 + 安全承诺）→ 账号（学号 + 密码 / 使用现有密码）→ 进度（小票逐行打印）→ 完成（服务/拨号/托盘三项摘要）；维护页：修复 / 卸载。
Proof/content: 安装步骤事件、完成状态、日志入口与重试；不展示密码。
Constraints: 520×460、自提权一次 UAC、中文、silent 模式供脚本、失败自动回滚且可重试；未打包时读身边 payload/ 目录开发态运行。

Chosen direction: 校园一卡通 / 发卡（圈存机办卡：卡面预览 + 小票打印）。
Memorable moment: 小票打印——每个步骤完成即印出一行；完成时撕纸双虚线 + "发卡完成"。

## Direction contract

THESIS: 安装就是发一张联网卡：办卡柜台式流程，每一步都印在小票上，失败不藏、可重来。
OWN-WORLD: 同日常窗口（卡蓝 / 票纸白 / 墨黑 / 读卡绿 / 朱红）；左侧卡面预览随学号填入，右侧小票逐行打印，底部软键排。
STORY: 明白这是办一张"上网卡" → 给账号密码 → 看小票打印 → 拿卡走人；出问题知道去哪看、点哪重试。
FIRST VIEWPORT: 圈存机框体：左卡面（学号 / 状态章），右小票（步骤行 + 虚线 + 撕纸线），底部软键（上一步 / 下一步）。
FORM: 校园一卡通（自选排序 #1；掷签 key f8e9de4f），code-led 构建（无图像生成）。
FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
