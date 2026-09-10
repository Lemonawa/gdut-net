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
    // GUI 安装器无 stderr（windows 子系统）：写 ProgramData 文件日志。
    crate::logging::init_tray_logging(r"C:\ProgramData\gdut-net\logs", "setup");
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
    UninstallDone,
    StartService,
}

/// 小票行状态（安装/卸载共用）。
pub(crate) enum StepStatus {
    /// 已开始、未完成。
    Running,
    Done,
    Skipped,
    Failed(String),
}

/// 小票单行：`key` 用于事件匹配（稳定英文标识），`label` 用于显示（中文）。
pub(crate) struct StepRow {
    key: &'static str,
    label: String,
    status: StepStatus,
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
    /// 步骤行（key 匹配，label 显示）。
    pub(crate) steps: Vec<StepRow>,
    pub(crate) result: Option<Result<(), String>>,
    /// 失败时的回滚实情（完成页据此陈述，不得默认"已回滚"）。
    pub(crate) rollback: Option<work::RollbackOutcome>,
    pub(crate) status_line: Option<String>,
    /// 当前流程是否为卸载（决定 Progress → UninstallDone 与小票文案）。
    pub(crate) uninstalling: bool,
    /// 卸载确认页的 "同时删除配置与日志" 勾选，默认 OFF。
    pub(crate) purge: bool,
}

impl SetupApp {
    fn new(args: SetupArgs, state: InstallState, font_ok: bool) -> Self {
        // 模式定初始页（spec §5）：--repair 定位修复、--uninstall 定位卸载、
        // --start-service 直接走启动流程；无参按安装态进向导或维护页。
        let page = match args.mode().unwrap_or(Mode::Gui) {
            Mode::Repair => Page::Maintenance,
            // 开始菜单"卸载 GDUT Net"/应用和功能入口：已安装直接进卸载确认。
            Mode::Uninstall => match state {
                InstallState::Installed { .. } => Page::UninstallConfirm,
                InstallState::NotInstalled => Page::Maintenance,
            },
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
            rollback: None,
            status_line: None,
            uninstalling: false,
            purge: false,
        }
    }

    /// 拉起安装工作线程（Account「开始安装」/ StartService 失败重试后的再次安装）。
    fn start_install(&mut self) {
        self.error = None;
        self.steps.clear();
        self.result = None;
        self.rollback = None;
        self.status_line = None;
        self.uninstalling = false;
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

    /// 拉起卸载工作线程（UninstallConfirm「卸载」）。
    fn start_uninstall(&mut self) {
        self.error = None;
        self.steps.clear();
        self.result = None;
        self.rollback = None;
        self.status_line = None;
        self.uninstalling = true;
        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        self.page = Page::Progress;
        // remove_dir=true：真正删安装目录的是 shell 的延迟删除助手。
        work::spawn_uninstall(tx, self.purge, true);
    }

    /// 拉取工作线程事件；Progress 与 StartService 共用。
    /// 返回 Done 事件的结果（由调用方决定跳转哪个页面）。
    fn drain_events(&mut self) -> Option<Result<(), String>> {
        // 取出接收端，处理期间可自由改 self；处理完原样放回（rx 未被消耗）。
        let rx = self.rx.take()?;
        let mut done = None;
        loop {
            match rx.try_recv() {
                Ok(work::Ev::Step { key, label }) => self.steps.push(StepRow {
                    key,
                    label,
                    status: StepStatus::Running,
                }),
                Ok(work::Ev::StepDone { key, label }) => {
                    // 拨号结果补充行（"等待拨号结果（Connected）"）留给完成页展示。
                    if key == work::STEP_WAIT_DIAL {
                        self.status_line = Some(label.clone());
                    }
                    self.finish_step(key, label, StepStatus::Done);
                }
                Ok(work::Ev::StepFinished {
                    key,
                    label,
                    outcome,
                }) => {
                    // 卸载报告行：uninstall_core 返回后由 worker 逐行翻译（核心不打印）。
                    let status = match outcome {
                        work::StepOutcome::Done => StepStatus::Done,
                        work::StepOutcome::Skipped => StepStatus::Skipped,
                        work::StepOutcome::Failed(e) => StepStatus::Failed(e),
                    };
                    self.finish_step(key, label, status);
                }
                Ok(work::Ev::Done { result, rollback }) => {
                    self.rollback = Some(rollback);
                    done = Some(result);
                }
                // 通道空 = 本轮拉完；断开 = 线程结束（Done 之前断开说明线程死了）。
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if done.is_none() {
                        let what = if self.uninstalling {
                            "卸载"
                        } else {
                            "安装"
                        };
                        done = Some(Err(format!("{what}线程意外退出，请查看日志。")));
                    }
                    break;
                }
            }
        }
        self.rx = Some(rx);
        done
    }

    /// 按 key 定位步骤行并落结局；找不到则追加（防御未知 key）。
    fn finish_step(&mut self, key: &'static str, label: String, status: StepStatus) {
        if let Some(row) = self.steps.iter_mut().rev().find(|r| r.key == key) {
            row.label = label;
            row.status = status;
        } else {
            self.steps.push(StepRow { key, label, status });
        }
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
                Page::UninstallDone => self.uninstall_done_page(ui),
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
            self.page = if self.uninstalling {
                Page::UninstallDone
            } else {
                Page::Done
            };
        }
        let (card, busy) = if self.uninstalling {
            ("销卡中", "正在卸载，请勿关闭窗口…")
        } else {
            ("发卡中", "正在处理，请勿关闭窗口…")
        };
        ui.horizontal_top(|ui| {
            self.card_preview(ui, card);
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.set_width(280.0);
                self.receipt(ui);
                if self.result.is_none() {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(busy);
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
                        // 回滚实情：只陈述真的发生过的事，不替回滚打包票。
                        match self.rollback.clone() {
                            Some(work::RollbackOutcome::Restored) => {
                                ui.label("已自动回滚到安装前的状态。");
                            }
                            Some(work::RollbackOutcome::RestoredUnknown) => {
                                ui.colored_label(
                                    VERMILION,
                                    "服务原本已存在，但无法读取其路径，未做任何改动。请查看日志或手动修复。",
                                );
                            }
                            Some(work::RollbackOutcome::Failed(rb)) => {
                                ui.colored_label(
                                    VERMILION,
                                    format!("自动回滚失败：{rb}，请运行安装程序重试或查看日志。"),
                                );
                            }
                            Some(work::RollbackOutcome::NotNeeded) | None => {
                                ui.label("可打开日志查看原因，或返回重试。");
                            }
                        }
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
                let location = match service_exe {
                    Some(exe) => exe
                        .parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| exe.display().to_string()),
                    None => "（无法读取服务路径）".to_string(),
                };
                ui.label(format!("安装位置：{location}"));
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
                // 每次进入确认页都从"不删除配置与日志"开始（防误勾选）。
                self.purge = false;
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
        ui.horizontal_top(|ui| {
            self.card_preview(ui, "待销卡");
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.set_width(280.0);
                ui.label("将停止后台服务，并移除开始菜单快捷方式与“应用和功能”条目。");
                ui.add_space(10.0);
                ui.checkbox(
                    &mut self.purge,
                    "同时删除配置与日志（含学号密码、拨号记录）",
                );
                ui.add_space(6.0);
                if self.purge {
                    ui.colored_label(
                        VERMILION,
                        "警告：删除后需重新输入学号和密码才能再次安装，历史日志无法恢复。",
                    );
                } else {
                    ui.label("配置与日志将保留：重新安装时可直接使用现有密码。");
                }
            });
        });
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(6.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let uninstall = egui::Button::new(
                egui::RichText::new("卸载")
                    .color(egui::Color32::WHITE)
                    .strong(),
            )
            .fill(VERMILION);
            if ui.add(uninstall).clicked() {
                self.start_uninstall();
            }
            if ui.button("取消").clicked() {
                self.page = Page::Maintenance;
            }
        });
    }

    fn uninstall_done_page(&mut self, ui: &mut egui::Ui) {
        match self.result.clone() {
            Some(Ok(())) => {
                ui.horizontal_top(|ui| {
                    self.card_preview(ui, "已销卡");
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        ui.set_width(280.0);
                        self.receipt(ui);
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("卸载完成").color(READER_GREEN).strong());
                        if self.purge {
                            ui.label("配置与日志已删除。");
                        } else {
                            ui.label("配置与日志已保留。");
                        }
                        // 目录删除交给延迟助手，此刻不能声称"目录已不存在"。
                        ui.label(
                            "安装目录的删除已安排后台执行；若目录仍在，重启电脑后手动删除即可。",
                        );
                    });
                });
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("关闭").clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            }
            Some(Err(e)) => {
                ui.horizontal_top(|ui| {
                    self.card_preview(ui, "销卡失败");
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        ui.set_width(280.0);
                        ui.colored_label(VERMILION, egui::RichText::new("卸载未完成").strong());
                        ui.add_space(4.0);
                        for line in e.lines() {
                            ui.colored_label(VERMILION, line);
                        }
                        ui.add_space(6.0);
                        ui.label("卸载是幂等的：可再次尝试；若问题依旧，请查看日志。");
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
                        self.result = None;
                        self.rx = None;
                        self.page = Page::UninstallConfirm;
                    }
                    if ui.button("关闭").clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            }
            None => {
                // 理论到不了（Progress 收到 Done 才跳转）；给个稳妥的后退。
                ui.label("没有可显示的卸载结果。");
                if ui.button("返回").clicked() {
                    self.page = Page::Maintenance;
                }
            }
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
                let title = if self.uninstalling {
                    "销卡小票"
                } else {
                    "安装小票"
                };
                ui.label(egui::RichText::new(title).color(INK_BLACK).strong());
                ui.add_space(4.0);
                for row in &self.steps {
                    let (line, color) = match &row.status {
                        StepStatus::Running => (format!("… {}", row.label), INK_BLACK),
                        StepStatus::Done => (format!("✓ {}", row.label), READER_GREEN),
                        StepStatus::Skipped => {
                            (format!("跳过 {}", row.label), INK_BLACK.gamma_multiply(0.6))
                        }
                        StepStatus::Failed(e) => (format!("失败 {}：{e}", row.label), VERMILION),
                    };
                    ui.colored_label(color, line);
                }
                if let Some(result) = &self.result {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("- ".repeat(20)).color(INK_BLACK.gamma_multiply(0.45)),
                    );
                    match (result, self.uninstalling) {
                        (Ok(()), false) => ui.colored_label(READER_GREEN, "发卡完成"),
                        (Ok(()), true) => ui.colored_label(READER_GREEN, "卸载完成"),
                        (Err(_), false) => ui.colored_label(VERMILION, "发卡失败"),
                        (Err(_), true) => ui.colored_label(VERMILION, "卸载失败"),
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
