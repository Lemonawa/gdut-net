# ADR-0006：托盘面板回归 egui（glow + default_fonts），黑屏根因是字体特性被关

日期：2026-09-09
状态：已接受（替代 commit 5916a53 的 MessageBox 权宜与"核显黑屏"结论）

## 背景

ADR-0004 选了 tray-icon + egui/eframe。实施中面板两度"黑屏"（c80a9bc glow 版、112b87f wgpu 版），最终 5916a53 退回 Win32 MessageBox，代码注释归因"wgpu 在部分核显上仍黑屏"。

2026-09-09 取证（子代理读 vendored crate 源码 + git 史）：

- 两次尝试都是 `default-features = false, features=["glow"(+"wgpu")]`——`default_fonts` 从未开启。
- epaint 0.36.1 `fonts.rs:496-504`：无该特性时 `FontDefinitions::default()` = 空字体表；`:644-661` 空字体族短路且**无任何警告日志**，`glyph_width` 恒 0。
- 面板 UI 全是文字 → 零文字光栅化 → 只剩深色 `panel_fill #1B1B1B` = "黑屏"。两个渲染器同症是共因铁证，与 GPU 无关。
- egui 上游 issue #1723 / #2276 同款翻车记录。

## 决策

1. 面板回归 eframe 0.36，`Renderer::Glow` + 显式 `features = ["glow", "default_fonts"]`，`winit` `with_any_thread(true)`（面板跑在独立线程）。
2. glow 的 WGL→EGL→GLES→软渲染回退链即核显兼容路径；不引 wgpu（零 wgpu crate，exe ~5.8MB vs 现 ~1.5MB，接受）。
3. 若上机仍黑屏（spike 负对照已建：关 default_fonts 应复现原黑屏），fallback 是零新依赖的 Win32 modeless 原生控件面板（B1 方案），不再折腾 GPU。
4. 托盘菜单升级为原生富菜单（muda 已有 API）：CheckMenuItem 模式二选一、分隔线、禁用态、按状态换 `set_icon`/tooltip。

## 后果

- 修正 5916a53 注释的错误归因；`panel.rs` 的 MessageBox 实现删除。
- 面板打开期间不再冻结托盘菜单（MessageBox 曾在泵线程弹）。
- MSRV 1.95（eframe 0.36.1），仓库 stable 1.98 满足。
- 验收：spike 上机过（文字可见、关开 5 次无残留）才合正式代码。
