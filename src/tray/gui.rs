//! 日常状态窗口（常驻）：首次点开创建；关窗 = 隐藏；左键托盘唤出。
//! egui/eframe glow（ADR-0006）；快照每帧拉取，操作经 mpsc 回泵线程发 IPC。
//!
//! Task 10 保留旧面板内容（英文）；Task 11 换成锁定视觉方向的中文界面。

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use eframe::egui::{CentralPanel, Grid, ScrollArea, ViewportBuilder};
use eframe::{NativeOptions, Renderer};

use crate::ipc::protocol::{NetMode, StateSnapshot};

use super::SharedSnapshot;

/// GUI 的 egui 上下文句柄：Some = 窗口线程活着（可能处于隐藏）。
pub(crate) type GuiShared = Arc<Mutex<Option<egui::Context>>>;

/// 显示或创建窗口；已存在则显示 + 聚焦。
pub(crate) fn show_or_focus(
    shared: GuiShared,
    snapshot: SharedSnapshot,
    redial_tx: Sender<()>,
    setmode_tx: Sender<NetMode>,
) {
    if let Some(ctx) = shared.lock().ok().and_then(|g| g.clone()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
        return;
    }
    let shared2 = Arc::clone(&shared);
    let _ = std::thread::Builder::new()
        .name("gdut-net-gui".into())
        .spawn(move || {
            if let Err(e) = run_window(Arc::clone(&shared2), snapshot, redial_tx, setmode_tx) {
                log::error!("GUI window exited: {e:#}");
            }
            if let Ok(mut g) = shared2.lock() {
                *g = None; // 线程退出后允许下次点击重建
            }
        });
}

fn run_window(
    shared: GuiShared,
    snapshot: SharedSnapshot,
    redial_tx: Sender<()>,
    setmode_tx: Sender<NetMode>,
) -> anyhow::Result<()> {
    let mut options = NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("GDUT Net")
            .with_inner_size(egui::vec2(460.0, 540.0))
            .with_resizable(false),
        renderer: Renderer::Glow,
        ..Default::default()
    };
    options.event_loop_builder = Some(Box::new(|builder| {
        use winit::platform::windows::EventLoopBuilderExtWindows as _;
        builder.with_any_thread(true);
    }));
    eframe::run_native(
        "gdut-net-gui",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::light());
            if let Ok(mut g) = shared.lock() {
                *g = Some(cc.egui_ctx.clone());
            }
            Ok(Box::new(Gui {
                snapshot,
                redial_tx,
                setmode_tx,
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe failed: {e}"))
}

struct Gui {
    snapshot: SharedSnapshot,
    redial_tx: Sender<()>,
    setmode_tx: Sender<NetMode>,
}

impl eframe::App for Gui {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // 关窗 = 隐藏：取消关闭、窗口留活；托盘左键（或二次启动）再唤出。
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        ctx.request_repaint_after(Duration::from_millis(500));
        CentralPanel::default().show(ui, |ui| {
            // 每帧拉一次快照（锁内只做 clone，不放 egui 绘制进锁）。
            let (wired, wireless, mode, events) = {
                let guard = self.snapshot.lock().expect("snapshot poisoned");
                let s: Option<&StateSnapshot> = guard.as_ref();
                (
                    s.map(|s| {
                        (
                            s.status_text(),
                            s.uptime_text(),
                            s.ip.clone(),
                            s.last_drop_reason.clone(),
                            s.heartbeat_text(),
                        )
                    }),
                    s.map(|s| (s.wireless_text(), s.wireless.ip.clone())),
                    s.map(|s| s.mode),
                    s.map(|s| s.events.iter().cloned().collect::<Vec<_>>())
                        .unwrap_or_default(),
                )
            };
            ui.heading("gdut-net");
            ui.add_space(8.0);
            Grid::new("status")
                .num_columns(2)
                .spacing([24.0, 6.0])
                .show(ui, |ui| {
                    ui.strong("Wired");
                    ui.label(
                        wired
                            .as_ref()
                            .map_or_else(|| "No service".to_string(), |w| w.0.clone()),
                    );
                    ui.end_row();
                    ui.strong("Uptime");
                    ui.label(
                        wired
                            .as_ref()
                            .map_or_else(|| "—".to_string(), |w| w.1.clone()),
                    );
                    ui.end_row();
                    ui.strong("Wired IP");
                    ui.label(
                        wired
                            .as_ref()
                            .and_then(|w| w.2.clone())
                            .unwrap_or_else(|| "—".to_string()),
                    );
                    ui.end_row();
                    ui.strong("Heartbeat");
                    ui.label(
                        wired
                            .as_ref()
                            .map_or_else(|| "—".to_string(), |w| w.4.clone()),
                    );
                    ui.end_row();
                    ui.strong("Drop reason");
                    ui.label(
                        wired
                            .as_ref()
                            .and_then(|w| w.3.clone())
                            .unwrap_or_else(|| "—".to_string()),
                    );
                    ui.end_row();
                    ui.strong("Wireless");
                    ui.label(
                        wireless
                            .as_ref()
                            .map(|w| w.0.clone())
                            .unwrap_or_else(|| "—".to_string()),
                    );
                    ui.end_row();
                    ui.strong("Wireless IP");
                    ui.label(
                        wireless
                            .as_ref()
                            .and_then(|w| w.1.clone())
                            .unwrap_or_else(|| "—".to_string()),
                    );
                    ui.end_row();
                });
            ui.add_space(10.0);
            ui.label(egui::RichText::new("Network mode").strong());
            let current = mode.unwrap_or_default();
            if ui
                .radio(
                    current == NetMode::WiredExclusive,
                    "Wired only (auto wireless takeover)",
                )
                .clicked()
            {
                let _ = self.setmode_tx.send(NetMode::WiredExclusive);
            }
            if ui
                .radio(
                    current == NetMode::WiredPlusStandby,
                    "Wired + wireless standby",
                )
                .clicked()
            {
                let _ = self.setmode_tx.send(NetMode::WiredPlusStandby);
            }
            ui.add_space(10.0);
            if ui.button("Redial now").clicked() {
                let _ = self.redial_tx.send(());
            }
            ui.add_space(10.0);
            ui.label(egui::RichText::new("Recent events").strong());
            ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                for line in events.iter().rev() {
                    ui.monospace(line);
                }
            });
        });
    }
}
