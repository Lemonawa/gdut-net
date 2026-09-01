# 托盘用 tray-icon + egui，状态面板按需弹出

托盘栈选型（2026-08 依赖核实）：iced/slint 均无官方托盘支持；Tauri v2 为纯托盘引入 WebView2 运行时（100MB+）不成比例。选定 tray-icon 0.24（tauri-apps 官方）+ egui/eframe 0.36（glow 后端）——社区标准组合，事件通道可直接接入 eframe 循环。常驻部分仅托盘图标 + 原生菜单（几 MB）；"状态面板"点击时才创建 egui 窗口（会话状态、IP、在线时长、掉线原因、手动重拨），关窗即释放，空闲内存 <30MB。eframe 0.36 MSRV 1.95，CI 用最新 stable 工具链。

系统通知用 tauri-winrt-notification 直调（notify-rust 在 Windows 只是它的旧版空壳）。
