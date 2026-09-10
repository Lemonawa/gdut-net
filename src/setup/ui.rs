//! 安装器 GUI（中文）：向导 + 维护页。
//!
//! 视觉方向"校园一卡通 / 圈存机"（`.impeccable/surfaces/src-setup-ui-rs.md`）：
//! 左侧卡面预览，右侧小票步骤，底部软键。功能页面由 Task 8 实现，
//! 完整排版在 Task 13 对着截图评审。

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use anyhow::Result;
use eframe::egui::{self, ViewportBuilder};

use crate::service::InstallState;
use crate::setup_args::Mode;

use super::{install_dir, work, SetupArgs};

// ---- 校园卡配色（自绘色板；egui 默认主题只作控件底色）----

/// 卡蓝：卡面与主按钮。
const CARD_BLUE: egui::Color32 = egui::Color32::from_rgb(0x1B, 0x4F, 0x9C);
/// 票纸白：小票底色。
const PAPER_WHITE: egui::Color32 = egui::Color32::from_rgb(0xFB, 0xFA, 0xF5);
/// 墨黑：正文与虚线。
const INK_BLACK: egui::Color32 = egui::Color32::from_rgb(0x1F, 0x23, 0x28);
/// 读卡绿：完成标记。
const READER_GREEN: egui::Color32 = egui::Color32::from_rgb(0x2E, 0x9E, 0x5B);
/// 朱红：错误。
const VERMILION: egui::Color32 = egui::Color32::from_rgb(0xC0, 0x39, 0x2B);

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

/// 向导页面。
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

pub(crate) struct SetupApp {
    pub(crate) args: SetupArgs,
    pub(crate) state: InstallState,
    pub(crate) font_ok: bool,
    pub(crate) page: Page,
    pub(crate) error: Option<String>,
    /// 账号页输入（密码只在内存，绝不落日志）。
    pub(crate) student_id: String,
    pub(crate) password: String,
    pub(crate) keep_existing: bool,
    /// 已有可用密文时填入学号（`service::existing_account`）。
    pub(crate) has_existing: Option<String>,
    pub(crate) rx: Option<Receiver<work::Ev>>,
    /// 步骤行（名称，完成）。
    pub(crate) steps: Vec<(String, bool)>,
    pub(crate) result: Option<Result<(), String>>,
    pub(crate) status_line: Option<String>,
}

impl SetupApp {
    fn new(args: SetupArgs, state: InstallState, font_ok: bool) -> Self {
        // 模式定初始页（spec §5）：--repair 定位修复、--uninstall 定位卸载、
        // --start-service 直接走启动流程；无参按安装态进向导或维护页。
        let page = match args.mode().unwrap_or(Mode::Gui) {
            Mode::Repair | Mode::Uninstall => Page::Maintenance,
            Mode::StartService => Page::StartService,
            _ => match state {
                InstallState::Installed { .. } => Page::Maintenance,
                InstallState::NotInstalled => Page::Welcome,
            },
        };
        let has_existing = crate::service::existing_account(&super::config_path());
        Self {
            args,
            state,
            font_ok,
            page,
            error: None,
            student_id: has_existing.clone().unwrap_or_default(),
            password: String::new(),
            keep_existing: has_existing.is_some(),
            has_existing,
            rx: None,
            steps: Vec::new(),
            result: None,
            status_line: None,
        }
    }

    /// 拉起安装工作线程（Account「开始安装」/ StartService 失败重试后的再次安装）。
    fn start_install(&mut self) {
        self.error = None;
        self.steps.clear();
        self.result = None;
        self.status_line = None;
        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        self.page = Page::Progress;
        work::spawn_install(
            tx,
            self.args.clone(),
            self.student_id.trim().to_string(),
            if self.keep_existing {
                None
            } else {
                Some(self.password.clone())
            },
        );
    }

    /// 拉取工作线程事件；Progress 与 StartService 共用。
    /// 返回 Done 事件的结果（由调用方决定跳转哪个页面）。
    fn drain_events(&mut self) -> Option<Result<(), String>> {
        let mut done = None;
        if let Some(rx) = &self.rx {
            loop {
                match rx.try_recv() {
                    Ok(work::Ev::Step(label)) => self.steps.push((label, false)),
                    Ok(work::Ev::StepDone(label)) => {
                        // 标记同名步骤完成；带括号补充文本的按前缀匹配。
                        for (l, step_done) in self.steps.iter_mut().rev() {
                            if label.starts_with(l.as_str()) {
                                *step_done = true;
                                *l = label.clone();
                                break;
                            }
                        }
                        // 拨号结果补充行（"等待拨号结果（Connected）"）留给完成页展示。
                        if label.starts_with("等待拨号结果") {
                            self.status_line = Some(label);
                        }
                    }
                    Ok(work::Ev::Done(result)) => done = Some(result),
                    // 通道空 = 本轮拉完；断开 = 线程结束（Done 之前断开说明线程死了）。
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        if done.is_none() {
                            done = Some(Err("安装线程意外退出，请查看日志。".to_string()));
                        }
                        break;
                    }
                }
            }
        }
        done
    }
}

impl eframe::App for SetupApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // 工作线程跑着时保持重绘（mpsc 无唤醒，靠轮询）。
        if matches!(self.page, Page::Progress | Page::StartService) && self.rx.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(80));
        }
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
                Page::Welcome => self.welcome_page(ui),
                Page::Account => self.account_page(ui),
                Page::Progress => self.progress_page(ui),
                Page::Done => self.done_page(ui),
                Page::Maintenance => self.maintenance_page(ui),
                Page::UninstallConfirm => self.uninstall_confirm_page(ui),
                Page::StartService => self.start_service_page(ui),
            }
        });
    }
}

impl SetupApp {
    fn welcome_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_top(|ui| {
            self.card_preview(ui, "待发卡");
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.set_width(280.0);
                ui.label("gdut-net 会在后台自动完成校园网拨号，并在拔线时接管校园 WiFi。");
                ui.add_space(8.0);
                ui.label("安装过程不会改动你的代理、VPN 或其他网卡设置。");
            });
        });
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(6.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("开始安装").clicked() {
                self.page = Page::Account;
            }
        });
    }

    fn account_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_top(|ui| {
            self.card_preview(ui, "申请中");
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.set_width(280.0);
                ui.label(egui::RichText::new("学号").strong());
                ui.add(
                    egui::TextEdit::singleline(&mut self.student_id)
                        .desired_width(260.0)
                        .hint_text("例如 3124000000"),
                );
                ui.add_space(8.0);
                ui.label(egui::RichText::new("密码").strong());
                ui.add_enabled(
                    !self.keep_existing,
                    egui::TextEdit::singleline(&mut self.password)
                        .password(true)
                        .desired_width(260.0)
                        .hint_text("校园网密码"),
                );
                if self.has_existing.is_some() {
                    ui.add_space(4.0);
                    ui.checkbox(&mut self.keep_existing, "使用现有密码");
                }
            });
        });
        if let Some(err) = &self.error {
            ui.add_space(6.0);
            ui.colored_label(VERMILION, err);
        }
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(6.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("开始安装").clicked() {
                // 校验：学号非空；不使用现有密码时密码非空。
                if self.student_id.trim().is_empty() {
                    self.error = Some("请填写学号。".to_string());
                } else if !self.keep_existing && self.password.is_empty() {
                    self.error = Some("请填写密码，或勾选“使用现有密码”。".to_string());
                } else {
                    self.start_install();
                }
            }
            if ui.button("返回").clicked() {
                self.error = None;
                self.page = match self.state {
                    InstallState::Installed { .. } => Page::Maintenance,
                    InstallState::NotInstalled => Page::Welcome,
                };
            }
        });
    }

    fn progress_page(&mut self, ui: &mut egui::Ui) {
        if let Some(result) = self.drain_events() {
            self.result = Some(result);
            self.page = Page::Done;
        }
        ui.horizontal_top(|ui| {
            self.card_preview(ui, "发卡中");
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.set_width(280.0);
                self.receipt(ui);
                if self.result.is_none() {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("正在处理，请勿关闭窗口…");
                    });
                }
            });
        });
    }

    fn done_page(&mut self, ui: &mut egui::Ui) {
        match self.result.clone() {
            Some(Ok(())) => {
                ui.horizontal_top(|ui| {
                    self.card_preview(ui, "已发卡");
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        ui.set_width(280.0);
                        self.receipt(ui);
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new("服务已启动")
                                .color(READER_GREEN)
                                .strong(),
                        );
                        if let Some(line) = &self.status_line {
                            ui.label(line);
                        }
                    });
                });
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("打开 GDUT Net").clicked() {
                        self.open_tray();
                    }
                    if ui.button("关闭").clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            }
            Some(Err(e)) => {
                ui.horizontal_top(|ui| {
                    self.card_preview(ui, "发卡失败");
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        ui.set_width(280.0);
                        ui.colored_label(VERMILION, egui::RichText::new("安装失败").strong());
                        ui.add_space(4.0);
                        // 错误文本可能是多行（anyhow chain），逐行画红。
                        for line in e.lines() {
                            ui.colored_label(VERMILION, line);
                        }
                        ui.add_space(6.0);
                        ui.label("已自动回滚到安装前的状态，可打开日志查看原因，或返回重试。");
                    });
                });
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("打开日志").clicked() {
                        self.open_logs();
                    }
                    if ui.button("重试").clicked() {
                        // 回账号页重来；保留已填学号。
                        self.error = None;
                        self.result = None;
                        self.page = Page::Account;
                    }
                    if ui.button("关闭").clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            }
            None => {
                // 理论到不了（Progress 收到 Done 才跳转）；给个稳妥的后退。
                ui.label("没有可显示的安装结果。");
                if ui.button("返回").clicked() {
                    self.page = Page::Account;
                }
            }
        }
    }

    fn maintenance_page(&mut self, ui: &mut egui::Ui) {
        match &self.state {
            InstallState::Installed {
                service_exe,
                version,
            } => {
                ui.label("GDUT Net 已安装在这台电脑上。");
                ui.add_space(6.0);
                ui.label(format!(
                    "安装位置：{}",
                    if service_exe.as_os_str().is_empty() {
                        install_dir().display().to_string()
                    } else {
                        service_exe
                            .parent()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| service_exe.display().to_string())
                    }
                ));
                ui.label(format!(
                    "版本：{}",
                    version.as_deref().unwrap_or(env!("CARGO_PKG_VERSION"))
                ));
            }
            InstallState::NotInstalled => {
                ui.label("GDUT Net 尚未安装。修复安装会重新解包文件并注册服务。");
            }
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button("修复安装").clicked() {
                // 已有可用密文时默认勾选"使用现有密码"（预填学号）。
                self.has_existing = crate::service::existing_account(&super::config_path());
                self.student_id = self.has_existing.clone().unwrap_or_default();
                self.keep_existing = self.has_existing.is_some();
                self.password.clear();
                self.error = None;
                self.page = Page::Account;
            }
            if matches!(self.state, InstallState::Installed { .. }) && ui.button("卸载").clicked()
            {
                // Task 9 接上真实卸载（保留配置 / 彻底清除两档）。
                self.page = Page::UninstallConfirm;
            }
        });
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(6.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("关闭").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }

    fn uninstall_confirm_page(&mut self, ui: &mut egui::Ui) {
        // Task 9 实现真实卸载（保留配置 / 彻底清除两档）。
        ui.label("卸载向导将在下一个任务完成。");
        ui.add_space(12.0);
        if ui.button("返回").clicked() {
            self.page = Page::Maintenance;
        }
    }

    fn start_service_page(&mut self, ui: &mut egui::Ui) {
        // 首帧自动启动；Done 后停（rx 置 None 防止重复跑）。
        if self.rx.is_none() && self.result.is_none() {
            self.steps.clear();
            self.status_line = None;
            let (tx, rx) = std::sync::mpsc::channel();
            self.rx = Some(rx);
            work::spawn_start_service(tx);
        }
        if let Some(result) = self.drain_events() {
            self.result = Some(result);
            self.rx = None;
        }
        match self.result.clone() {
            None => {
                ui.horizontal_top(|ui| {
                    self.card_preview(ui, "续卡");
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        ui.set_width(280.0);
                        self.receipt(ui);
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("正在启动服务…");
                        });
                    });
                });
            }
            Some(Ok(())) => {
                ui.horizontal_top(|ui| {
                    self.card_preview(ui, "已发卡");
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        ui.set_width(280.0);
                        self.receipt(ui);
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new("服务已启动")
                                .color(READER_GREEN)
                                .strong(),
                        );
                        if let Some(line) = &self.status_line {
                            ui.label(line);
                        }
                    });
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("打开 GDUT Net").clicked() {
                        self.open_tray();
                    }
                    if ui.button("关闭").clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            }
            Some(Err(e)) => {
                ui.horizontal_top(|ui| {
                    self.card_preview(ui, "启动失败");
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        ui.set_width(280.0);
                        ui.colored_label(VERMILION, egui::RichText::new("服务启动失败").strong());
                        for line in e.lines() {
                            ui.colored_label(VERMILION, line);
                        }
                    });
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("打开日志").clicked() {
                        self.open_logs();
                    }
                    if ui.button("重试").clicked() {
                        self.result = None;
                        self.rx = None;
                    }
                    if ui.button("返回").clicked() {
                        self.page = Page::Maintenance;
                    }
                });
            }
        }
    }

    // ---- 小票与卡面（方向契约的骨架，完整排版 Task 13 评审）----

    /// 左侧卡面预览：卡蓝底、白字学号、状态章。
    fn card_preview(&self, ui: &mut egui::Ui, status: &str) {
        egui::Frame::new()
            .fill(CARD_BLUE)
            .corner_radius(8)
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(168.0);
                ui.set_height(96.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("GDUT Net 上网卡")
                            .color(egui::Color32::WHITE)
                            .strong(),
                    );
                    ui.add_space(10.0);
                    let id = if self.student_id.trim().is_empty() {
                        "—"
                    } else {
                        self.student_id.trim()
                    };
                    ui.label(
                        egui::RichText::new(id)
                            .color(egui::Color32::WHITE)
                            .size(18.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::BOTTOM), |ui| {
                        ui.label(egui::RichText::new(status).color(egui::Color32::WHITE));
                    });
                });
            });
    }

    /// 右侧小票：逐行步骤 + 结束虚线。
    fn receipt(&self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(PAPER_WHITE)
            .stroke(egui::Stroke::new(1.0, INK_BLACK.gamma_multiply(0.25)))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(250.0);
                ui.label(egui::RichText::new("安装小票").color(INK_BLACK).strong());
                ui.add_space(4.0);
                for (label, done) in &self.steps {
                    let line = if *done {
                        format!("✓ {label}")
                    } else {
                        format!("… {label}")
                    };
                    let color = if *done { READER_GREEN } else { INK_BLACK };
                    ui.colored_label(color, line);
                }
                if let Some(result) = &self.result {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("- ".repeat(20)).color(INK_BLACK.gamma_multiply(0.45)),
                    );
                    match result {
                        Ok(()) => ui.colored_label(READER_GREEN, "发卡完成"),
                        Err(_) => ui.colored_label(VERMILION, "发卡失败"),
                    };
                }
            });
    }

    // ---- 外部动作 ----

    /// 打开日常界面（安装目录的 gdut-net.exe tray）。
    fn open_tray(&self) {
        if let Err(e) = std::process::Command::new(install_dir().join("gdut-net.exe"))
            .arg("tray")
            .spawn()
        {
            log::warn!("Failed to open tray: {e}");
        }
    }

    /// 打开日志目录（资源管理器）。
    fn open_logs(&self) {
        let dir = PathBuf::from(super::DATA_DIR).join("logs");
        if let Err(e) = std::process::Command::new("explorer").arg(&dir).spawn() {
            log::warn!("Failed to open log dir {}: {e}", dir.display());
        }
    }
}
