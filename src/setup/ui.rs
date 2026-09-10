//! 安装器 GUI（中文）：向导 + 维护页。页面内容在后续任务补全。

use anyhow::Result;
use eframe::egui::{self, ViewportBuilder};

use crate::service::InstallState;

use super::{install_dir, SetupArgs};

pub fn run(args: SetupArgs) -> Result<()> {
    let state = crate::service::install_state();
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("GDUT Net 安装程序")
            .with_inner_size(egui::vec2(520.0, 460.0))
            .with_resizable(false),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "gdut-net-setup",
        options,
        Box::new(move |cc| {
            let font_ok = crate::fonts::install_cjk_fonts(&cc.egui_ctx).is_ok();
            Ok(Box::new(SetupApp::new(args, state, font_ok)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe failed: {e}"))
}

// Progress/Done/UninstallConfirm/StartService 与 args/state/error 由 Task 8/9 的页面消费。
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    Welcome,
    Account,
    Progress,
    Done,
    Maintenance,
    UninstallConfirm,
    StartService,
}

#[allow(dead_code)]
pub(crate) struct SetupApp {
    pub(crate) args: SetupArgs,
    pub(crate) state: InstallState,
    pub(crate) font_ok: bool,
    pub(crate) page: Page,
    pub(crate) error: Option<String>,
}

impl SetupApp {
    fn new(args: SetupArgs, state: InstallState, font_ok: bool) -> Self {
        let page = match state {
            InstallState::Installed { .. } => Page::Maintenance,
            InstallState::NotInstalled => Page::Welcome,
        };
        Self {
            args,
            state,
            font_ok,
            page,
            error: None,
        }
    }
}

impl eframe::App for SetupApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            if !self.font_ok {
                // 字体缺失时连这条消息都无法用中文渲染（默认字体无 CJK），故用英文，保证不是豆腐块。
                ui.colored_label(
                    egui::Color32::RED,
                    "No CJK font found (checked %SystemRoot%\\Fonts). Chinese text cannot be displayed.",
                );
                return;
            }
            ui.heading("GDUT Net 安装程序");
            ui.add_space(8.0);
            match self.page {
                Page::Welcome => {
                    ui.label("gdut-net 会在后台自动完成校园网拨号，并在拔线时接管校园 WiFi。");
                    ui.add_space(4.0);
                    ui.label("安装过程不会改动你的代理、VPN 或其他网卡设置。");
                    ui.add_space(12.0);
                    if ui.button("开始安装").clicked() {
                        self.page = Page::Account;
                    }
                }
                Page::Account => {
                    ui.label("账号页将在下一个任务实现。");
                    if ui.button("返回").clicked() {
                        self.page = Page::Welcome;
                    }
                }
                Page::Maintenance => {
                    ui.label(format!("已安装位置：{}", install_dir().display()));
                    ui.add_space(8.0);
                    if ui.button("关闭").clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                _ => {
                    ui.label("页面开发中");
                }
            }
        });
    }
}
