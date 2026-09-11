---
name: GDUT Net
description: 校园网拨号守护的桌面工具——安装与日常界面共用一套"校园一卡通 / 圈存机"视觉世界。
colors:
  card-blue: "#1D4E9E"
  card-blue-hover: "#3D66A9"
  card-blue-pressed: "#1C3E76"
  card-dim: "#BEC8D6"
  paper-white: "#F4F1E8"
  ink-black: "#1A1A1A"
  ink-secondary: "#5B5B58"
  ink-rule: "#8B8A85"
  reader-green: "#2F9E63"
  vermilion: "#C2402F"
  soft-hover: "#DADDDF"
  soft-pressed: "#C5CDD8"
typography:
  display:
    fontFamily: "system CJK (Deng.ttf → simhei.ttf → msyh.ttc → simsun.ttc)"
    fontSize: "28px"
    fontWeight: 600
    lineHeight: 1.2
  card-number:
    fontFamily: "system CJK monospace"
    fontSize: "24px"
    fontWeight: 600
    letterSpacing: "0.08em"
  body:
    fontFamily: "system CJK (Deng.ttf → simhei.ttf → msyh.ttc → simsun.ttc)"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: 1.5
  label:
    fontFamily: "system CJK"
    fontSize: "12px"
    fontWeight: 400
rounded:
  card: "8px"
spacing:
  xs: "4px"
  sm: "6px"
  md: "12px"
  lg: "16px"
components:
  card-face:
    backgroundColor: "{colors.card-blue}"
    textColor: "{colors.paper-white}"
    rounded: "{rounded.card}"
    padding: "12px"
    width: "168px"
    height: "96px"
  soft-key:
    backgroundColor: "{colors.paper-white}"
    textColor: "{colors.ink-black}"
    padding: "6px 12px"
  soft-key-hover:
    backgroundColor: "{colors.soft-hover}"
  soft-key-pressed:
    backgroundColor: "{colors.soft-pressed}"
  primary-action:
    backgroundColor: "{colors.card-blue}"
    textColor: "{colors.paper-white}"
    padding: "6px 16px"
  status-stamp:
    backgroundColor: "{colors.vermilion}"
    textColor: "{colors.paper-white}"
    padding: "3px 11px"
---

# Design System: GDUT Net

## Overview

**Creative North Star: "校园一卡通 / 圈存机"**

GDUT Net 的两个界面（安装器 = 发卡柜台，日常窗口 = 一卡通卡片与圈存小票）共用同一个世界：网络资格是一张卡，安装是发卡，状态像读卡灯一样只有四种，每一步都印在小票上。界面是纸与塑料的平面印刷物——卡蓝的卡面、票纸白的收据、等宽数字、虚线分隔、印章色块；不模仿屏幕外的材质，也不假装别的产品。

密度上，信息按"卡面 → 小票 → 软键排"的阅读顺序排布：一眼认可信状态与出口，其余字段成行打印，动作落在底部软键。两个窗口尺寸固定（日常 460×540；安装 520×460），没有响应式重排——这是桌面工具，窗口本身就是柜台与卡片的尺寸。

**Key Characteristics:**
- 平面印刷，无渐变、无阴影堆砌；层次只靠色块与虚线。
- 状态像读卡灯：已连接 / 重拨中 / 认证失败 / 无线接管（含服务未运行页），只有这几种颜色语义。
- 数字一律等宽：学号、IP、时长、版本。
- 动效克制：状态变化时卡面重盖一枚章（短促重绘），其余无过渡动画。

## Colors

一套平面印刷色：卡蓝承重，票纸白做底，墨黑为字，读卡绿与朱红只作状态。

### Primary
- **卡蓝 Card Blue** (#1D4E9E)：卡面底色；主操作（立即重拨 / 开始安装 / 修复安装）的填充；安装器页面底也以票纸白为主、卡蓝只做卡面与主键。悬停 #3D66A9、按下 #1C3E76。
- **票纸白 Paper White** (#F4F1E8)：两个界面的页面底色与软键底色；小票的底。

### Secondary
- **读卡绿 Reader Green** (#2F9E63)：成功与在线——"已连接"读数、步骤完成勾、服务正常。
- **朱红 Vermilion** (#C2402F)：失败与停机——"服务未运行""认证失败"、印章底色、安装失败行。

### Neutral
- **墨黑 Ink Black** (#1A1A1A)：正文与数字。
- **次级墨 Secondary Ink** (#5B5B58)：字段名与说明文字。
- **虚线墨 Rule Ink** (#8B8A85)：小票虚线分隔（实为短横重复串）。
- **卡面暗字 Card Dim** (#BEC8D6)：卡面上的"学号"等小标与未点亮的读卡灯。
- **软键灰 Soft Hover / Pressed** (#DADDDF / #C5CDD8)：软键悬停与按下。

### Named Rules
**The Reader-Light Rule.** 绿与朱红是状态灯，不是装饰色：除状态读数、印章、勾/叉记号外，任何大面积区域不得使用绿或朱红。
**The Paper Rule.** 页面底永远是票纸白 #F4F1E8；卡蓝只出现在卡面与主操作上，不铺满页面。

## Typography

**Display Font:** 系统 CJK 字体（Deng.ttf → simhei.ttf → msyh.ttc → simsun.ttc，运行时探测，缺失时以英文报错，绝不渲染豆腐块）
**Body Font:** 同上（与等宽同源，均由系统 CJK 家族映射）
**Label/Mono Font:** 系统 CJK 的等宽族——所有数字字段（学号、IP、在线时长、版本）

**Character:** 系统字体是产品真相的一部分：中文 Windows 的观感、零字体分发、永不离线的可读性。排印靠字号阶梯与颜色区分层级，不靠字重花样。

### Hierarchy
- **Display** (28px)：日常窗口的小票大字状态（"已连接""服务未运行"）。
- **Card number** (24px, 等宽, 字距 0.08em)：卡面上的学号，四位一组。
- **Title** (15–18px)：卡面"GDUT Net 上网卡"、段落小标题、安装器页标题。
- **Body** (13px)：字段行、事件流水、说明文字。
- **Label** (11–12px)：字段名、卡面小标、辅助说明。

### Named Rules
**The Monospace-Number Rule.** 一切数字用等宽：学号、IP、时长、版本；比例字只用于中文与标点。

## Layout

- 日常窗口 460×540：顶部卡面（卡蓝，学号 + 读卡灯 + 状态章）→ 票纸白小票（大字状态、虚线分节、字段网格、模式二选一、最近事件滚动区）→ 底部软键排（修改账号密码 / 打开日志目录 / 版本）。
- 安装窗口 520×460：左卡面预览（168×96，学号随输入、状态章随流程：待发卡 / 申请中 / 已装卡）→ 右侧 250–280 宽的小票（步骤行逐行打印 + 结束虚线）→ 底部软键排（上一步 / 下一步；维护页为修复安装 / 卸载 / 关闭）。
- 间距节奏 4 / 6 / 12 / 16；分节用 1px 细线或 20 连短横的虚线，不用空白堆叠。

## Elevation & Depth

平面。没有阴影——层次由色块层叠（票纸白页面上的卡蓝卡面、软键的悬停/按下填充变化）与虚线分节表达；任何形式的投影、浮雕、金属/纸质模拟都被世界拒绝。

## Shapes

- 卡面：8px 圆角矩形（唯一成规模的圆角）。
- 其余形制均为直角：页面、软键、小票、印章为色块，状态章是一枚轻微倾斜的实心矩形（内嵌一圈细线）。
- 读卡灯是唯一的圆形元素；小票分隔是虚线；无边框装饰、无侧条。

## Components

### Card Face（卡面）——签名组件
- **Shape:** 8px 圆角，卡蓝填充；日常窗口满宽，安装器为 168×96 预览。
- **Content:** "GDUT Net 上网卡 / 校园网一卡通"；学号等宽、四位一组；读卡灯（圆形，状态色）；状态章斜置压角。
- **Behavior:** 状态变化时重盖印章（短促重绘），不播放长动画。

### Receipt（小票）
- **Style:** 票纸白底，1px 墨线描边，行内容为"字段名（次级墨）+ 值（墨黑/等宽）"；分节用虚线；结束处打印"发卡完成 / 发卡失败"。
- **Error:** 失败行用朱红；重试入口就在同一张票上。

### Soft Keys（软键排）
- **Shape:** 直角，票纸白底、墨字；悬停 #DADDDF、按下 #C5CDD8。
- **Primary:** 主操作以卡蓝填充、白字（立即重拨 / 开始安装 / 修复安装 / 启动服务）。

### Radio Pair（模式二选一）
- **Style:** 两行文字选项，"有线优先（自动无线接管）"与"有线 + 无线备用"；选中态以卡蓝记号，不引入新色。

### Reader Light（读卡灯）
- **States:** 已连接 = 绿；重拨中 = 暗（呼吸由事件驱动）；认证失败 = 朱红；无线接管 = 绿（语义为无线出口）；服务未运行 = 页级朱红大字，灯灭。

### Event Stream（最近事件）
- **Style:** 等宽小字一行一条，最新在上；小票流水隐喻；滚动区域不加边框。

## Do's and Don'ts

### Do:
- **Do** 页面底用票纸白 #F4F1E8，卡蓝 #1D4E9E 只给卡面与主操作。
- **Do** 数字一律等宽（学号、IP、时长、版本）。
- **Do** 用四种读卡灯语义表达状态；页级停机用朱红大字 + 启动服务软键。
- **Do** 保持窗口尺寸固定（460×540 / 520×460）与软键排位置固定。

### Don't:
- **Don't** 使用渐变、阴影、浮雕或任何材质模拟（世界是平面印刷）。
- **Don't** 把读卡绿 / 朱红用成装饰色；它们是状态灯。
- **Don't** 在日常窗口展示密码、明文 portal URL 或任何截图。
- **Don't** 引入另一套色板或字体；两个界面必须同色同字（安装器与日常窗口的色值同一处维护）。
- **Don't** 添加图标字体或图形图标：语言是文字 + 色块 + 印章。
