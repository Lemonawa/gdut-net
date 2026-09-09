//! 详情面板：egui（eframe glow + default_fonts，ADR-0006）。
//! 每次打开起独立线程跑 run_native（with_any_thread），关窗线程退。
//! 旧 MessageBox 实现已删；黑屏根因是 default_fonts 被关，非核显。

use std::sync::mpsc::Sender;

use eframe::egui;
use eframe::egui::{CentralPanel, Grid, ScrollArea, ViewportBuilder};
use eframe::{NativeOptions, Renderer};

use crate::ipc::protocol::{NetMode, StateSnapshot};

use super::SharedSnapshot;

/// 打开状态面板：每次点击起独立 eframe 线程（winit any_thread），
/// 关窗后 run_native 返回、线程退出。快照经 Arc 每帧拉取，操作经
/// mpsc 送回泵线程统一发 IPC。
pub fn show(snapshot: SharedSnapshot, redial_tx: Sender<()>, setmode_tx: Sender<NetMode>) {
    let _ = std::thread::Builder::new()
        .name("gdut-net-panel".into())
        .spawn(move || {
            if let Err(e) = run_panel(snapshot, redial_tx, setmode_tx) {
                log::error!("Panel exited: {e:#}");
            }
        });
}

fn run_panel(
    snapshot: SharedSnapshot,
    redial_tx: Sender<()>,
    setmode_tx: Sender<NetMode>,
) -> anyhow::Result<()> {
    let mut options = NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("gdut-net")
            .with_inner_size(egui::vec2(420.0, 360.0))
            .with_resizable(false),
        renderer: Renderer::Glow,
        ..Default::default()
    };
    options.event_loop_builder = Some(Box::new(|builder| {
        use winit::platform::windows::EventLoopBuilderExtWindows as _;
        builder.with_any_thread(true);
    }));
    eframe::run_native(
        "gdut-net-panel",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::light());
            Ok(Box::new(Panel {
                snapshot,
                redial_tx,
                setmode_tx,
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe failed: {e}"))
}

struct Panel {
    snapshot: SharedSnapshot,
    redial_tx: Sender<()>,
    setmode_tx: Sender<NetMode>,
}

impl eframe::App for Panel {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));
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
