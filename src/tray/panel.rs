//! 按需 egui 状态面板（eframe glow，独立线程）。
//!
//! 内存约束：窗口未开时零 egui 资源；打开即起线程跑 `eframe::run_native`，
//! 窗口关闭 `run_native` 返回、线程退出、app 连同 eframe/GL 资源一起销毁。

use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;
use eframe::egui;

use super::SharedSnapshot;

/// 面板刷新周期（在线时长秒级跳动所需）。
const REFRESH: Duration = Duration::from_millis(500);

/// 面板单例控制：已有面板窗口时 `show` 只做前置，不再开新线程。
/// 值为面板的 egui::Context（其 send_viewport_cmd 跨线程可用）。
static PANEL_CTX: Mutex<Option<egui::Context>> = Mutex::new(None);

/// 打开状态面板；已开则前置既有窗口。
///
/// `redial_tx`：面板"立即重拨"按钮 → 托盘泵线程（由泵线程统一操作
/// PipeClient）。
pub fn show(snapshot: SharedSnapshot, redial_tx: Sender<()>) {
    let guard = PANEL_CTX.lock().expect("PANEL_CTX 中毒");
    if let Some(ctx) = guard.as_ref() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        return;
    }

    // 线程退出（无论 run_native 成败）时清掉注册。
    struct CtxGuard;
    impl Drop for CtxGuard {
        fn drop(&mut self) {
            *PANEL_CTX.lock().expect("PANEL_CTX 中毒") = None;
        }
    }

    let spawned = std::thread::Builder::new()
        .name("gdut-net-panel".into())
        .spawn(move || {
            let _guard = CtxGuard;
            if let Err(e) = run(snapshot, redial_tx) {
                log::warn!("状态面板异常退出: {e:#}");
            }
        });
    if let Err(e) = spawned {
        // 线程没起来，CtxGuard 不会运行；这里手动清注册。
        *PANEL_CTX.lock().expect("PANEL_CTX 中毒") = None;
        log::warn!("启动面板线程失败: {e}");
    }
}

fn run(snapshot: SharedSnapshot, redial_tx: Sender<()>) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("gdut-net 状态")
            .with_inner_size(egui::vec2(380.0, 250.0))
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "gdut-net-panel",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(Panel {
                snapshot,
                redial_tx,
                redial_clicked: false,
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe 运行失败: {e}"))
}

struct Panel {
    snapshot: SharedSnapshot,
    /// "立即重拨" → 托盘泵线程。
    redial_tx: Sender<()>,
    /// 点击置位，ui 闭包外逐帧消费（闭包内只改自身状态，不做 IO）。
    redial_clicked: bool,
}

impl eframe::App for Panel {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // 定时重绘：在线时长需要秒级跳动，不依赖输入事件。
        ui.ctx().request_repaint_after(REFRESH);
        // 首帧注册 Context，供 show 前置已开窗口。
        *PANEL_CTX.lock().expect("PANEL_CTX 中毒") = Some(ui.ctx().clone());

        egui::CentralPanel::default()
            .frame(egui::Frame::default().inner_margin(egui::Margin::same(14)))
            .show(ui, |ui| {
                ui.heading("gdut-net 状态");
                ui.add_space(10.0);

                let guard = self.snapshot.lock().expect("快照缓存中毒");
                match guard.as_ref() {
                    None => {
                        ui.colored_label(
                            egui::Color32::ORANGE,
                            "未连接到 gdut-net 服务（服务未运行？）",
                        );
                    }
                    Some(s) => {
                        let rows: [(&str, String); 6] = [
                            ("状态", s.status_text()),
                            ("在线时长", s.uptime_text()),
                            ("IP", s.ip.clone().unwrap_or_else(|| "—".into())),
                            (
                                "掉线原因",
                                s.last_drop_reason.clone().unwrap_or_else(|| "—".into()),
                            ),
                            ("重拨次数", s.redial_attempts.to_string()),
                            ("心跳", s.heartbeat_text()),
                        ];
                        egui::Grid::new("status-grid")
                            .num_columns(2)
                            .spacing([16.0, 6.0])
                            .show(ui, |ui| {
                                for (label, value) in rows {
                                    ui.strong(label);
                                    ui.label(value);
                                    ui.end_row();
                                }
                            });
                    }
                }
                drop(guard);

                ui.add_space(14.0);
                if ui.button("立即重拨").clicked() {
                    self.redial_clicked = true;
                }
            });

        if self.redial_clicked {
            self.redial_clicked = false;
            let _ = self.redial_tx.send(());
        }
    }
}
