//! 日常状态窗口（常驻）：首次点开创建；关窗 = 隐藏；左键托盘唤出。
//! egui/eframe glow（ADR-0006）；快照每帧拉取，操作经 mpsc 回泵线程发 IPC。
//!
//! 视觉方向"校园一卡通 / 圈存机"（`.impeccable/surfaces/src-tray-gui-rs.md`）：
//! 顶部卡面（卡蓝地、学号如卡号、状态章），中部票纸白小票（大字状态、虚线字段
//! 行、事件流水），底部圈存机软键排。单窗口 460×540；不发网络请求，命令走 IPC。

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use eframe::egui::ViewportBuilder;
use eframe::{NativeOptions, Renderer};

use crate::ipc::protocol::{HeartbeatStatus, NetMode, SessionStatus, StateSnapshot, WPhase};

use super::SharedSnapshot;

const CFG_PATH: &str = r"C:\ProgramData\gdut-net\config.toml";
const LOG_DIR: &str = r"C:\ProgramData\gdut-net\logs";

// ---- 校园卡色板（方向契约的精确取值；与安装器同一世界）----

/// 卡蓝：卡面与主按钮。
const CARD_BLUE: egui::Color32 = egui::Color32::from_rgb(0x1D, 0x4E, 0x9E);
/// 卡蓝 hover：向票纸白提亮。
const CARD_BLUE_HOVER: egui::Color32 = egui::Color32::from_rgb(0x3D, 0x66, 0xA9);
/// 卡蓝 pressed：向墨黑压深。
const CARD_BLUE_PRESSED: egui::Color32 = egui::Color32::from_rgb(0x1C, 0x3E, 0x76);
/// 票纸白：小票与软键键面。
const PAPER_WHITE: egui::Color32 = egui::Color32::from_rgb(0xF4, 0xF1, 0xE8);
/// 墨黑：正文与虚线基准。
const INK_BLACK: egui::Color32 = egui::Color32::from_rgb(0x1A, 0x1A, 0x1A);
/// 读卡绿：在线。
const READER_GREEN: egui::Color32 = egui::Color32::from_rgb(0x2F, 0x9E, 0x63);
/// 朱红：认证失败 / 服务未运行。
const VERMILION: egui::Color32 = egui::Color32::from_rgb(0xC2, 0x40, 0x2F);
/// 纸上次级墨色：墨黑向票纸白 30%（仍是墨的色调，不是灰）。
const INK_SECONDARY: egui::Color32 = egui::Color32::from_rgb(0x5B, 0x5B, 0x58);
/// 小票虚线：墨黑向票纸白 52%。
const INK_RULE: egui::Color32 = egui::Color32::from_rgb(0x8B, 0x8A, 0x85);
/// 卡面次级白：票纸白向卡蓝 25%（同色相压暗，不是灰）。
const CARD_DIM: egui::Color32 = egui::Color32::from_rgb(0xBE, 0xC8, 0xD6);
/// 软键 hover 键面：票纸白向卡蓝 12%。
const SOFT_HOVER: egui::Color32 = egui::Color32::from_rgb(0xDA, 0xDD, 0xDF);
/// 软键 pressed 键面：票纸白向卡蓝 22%。
const SOFT_PRESSED: egui::Color32 = egui::Color32::from_rgb(0xC5, 0xCD, 0xD8);

/// 底部软键排高度：小票区（可滑动）让出的固定空间，随卡片一起被扣除。
const SOFT_ROW_H: f32 = 52.0;

// ---- Task 10 的窗口生命周期（未改：single-flight / 关窗=隐藏）----

/// GUI 状态机：Absent → Starting → Live；窗口线程退出后一律 Dead 终态。
///
/// winit 0.30 每进程只允许一个事件循环（EVENT_LOOP_CREATED 置位后不复位），
/// 所以 `run_native` 一旦返回（正常结束或初始化失败），本进程都不可能再建
/// 第二个窗口——重建分支不存在，Starting 也绝不能留在状态里。
pub(crate) enum GuiState {
    /// 尚未创建（或线程 spawn 失败，可重试）。
    Absent,
    /// 正在创建：single-flight 占位，窗口出现后自会可见。
    Starting,
    /// 窗口线程活着（可能处于隐藏）。
    Live(egui::Context),
    /// 终态：事件循环已结束，无法再显示（附原因）。
    Dead(String),
}

/// GUI 状态句柄。
pub(crate) type GuiShared = Arc<Mutex<GuiState>>;

/// 显示或创建窗口；已存在则显示 + 聚焦。
///
/// single-flight：持锁判定 + 置位，只有一个调用能走到 `spawn`；`Starting`
/// 期间（含双击连发的第二次点击）直接返回——正在创建的窗口会自行显示。
pub(crate) fn show_or_focus(
    shared: GuiShared,
    snapshot: SharedSnapshot,
    redial_tx: Sender<()>,
    setmode_tx: Sender<NetMode>,
) {
    let mut state = match shared.lock() {
        Ok(g) => g,
        Err(_) => return, // 锁中毒：无从安全恢复，静默放弃本次显示
    };
    match &*state {
        // 已存在：显示 + 聚焦。克隆 ctx 后放开锁再做跨线程唤醒。
        GuiState::Live(ctx) => {
            let ctx = ctx.clone();
            drop(state);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.request_repaint();
            return;
        }
        // 正在创建：窗口首次显示即在前台，无需额外动作。
        GuiState::Starting => return,
        // 终态：事件循环已结束，本进程内无法再建（winit 0.30 单事件循环），
        // 重试也不会成功，只记英文 warn（泵线程不做 MessageBox）。
        GuiState::Dead(reason) => {
            log::warn!("GUI window unavailable: {reason}");
            return;
        }
        // 首次：置 Starting 占位（single-flight）。
        GuiState::Absent => *state = GuiState::Starting,
    }
    drop(state);

    let shared2 = Arc::clone(&shared);
    let spawned = std::thread::Builder::new()
        .name("gdut-net-gui".into())
        .spawn(move || {
            // panic 也是退出路径：catch 住后同样落 Dead，绝不把 Starting 留下。
            let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_window(Arc::clone(&shared2), snapshot, redial_tx, setmode_tx)
            })) {
                Ok(r) => r,
                Err(_) => Err(anyhow::anyhow!("GUI thread panicked")),
            };
            let reason = match &result {
                Ok(()) => "window closed".to_string(),
                Err(e) => format!("{e:#}"),
            };
            if let Err(e) = &result {
                log::error!("GUI window exited: {e:#}");
            }
            if let Ok(mut g) = shared2.lock() {
                // run_native 返回 = 事件循环结束：Ok/Err 都进终态，绝不留 Starting。
                *g = GuiState::Dead(reason);
            }
        });
    if let Err(e) = spawned {
        log::error!("Failed to start GUI window thread: {e}");
        if let Ok(mut g) = shared.lock() {
            *g = GuiState::Absent; // 线程根本没起来：允许下次点击重试
        }
    }
}

fn run_window(
    shared: GuiShared,
    snapshot: SharedSnapshot,
    redial_tx: Sender<()>,
    setmode_tx: Sender<NetMode>,
) -> anyhow::Result<()> {
    let mut options = NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("GDUT Net 校园网")
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
            install_style(&cc.egui_ctx);
            // 中文字体必须显式注入；失败时整页退回英文提示（见 Gui::ui），
            // 绝不让中文渲染成豆腐块。
            let font_ok = match crate::fonts::install_cjk_fonts(&cc.egui_ctx) {
                Ok(_) => true,
                Err(e) => {
                    log::error!("Failed to load CJK fonts: {e:#}");
                    false
                }
            };
            if let Ok(mut g) = shared.lock() {
                *g = GuiState::Live(cc.egui_ctx.clone());
            }
            Ok(Box::new(Gui {
                snapshot,
                redial_tx,
                setmode_tx,
                student_id: load_student_id(),
                cfg_mtime: config_mtime(),
                font_ok,
                stamp_t: 1.0,
                last_word: None,
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe failed: {e}"))
}

/// egui 主题：亮底 + 校园卡色板（只做全局底色/交互反馈；具体材质在组件里画）。
fn install_style(ctx: &egui::Context) {
    // 两套主题（light/dark）都写成同一套值；再显式选浅色，避免跟随系统深色。
    ctx.all_styles_mut(|style| {
        let mut v = egui::Visuals::light();
        v.panel_fill = PAPER_WHITE;
        v.window_fill = PAPER_WHITE;
        v.extreme_bg_color = PAPER_WHITE;
        v.override_text_color = Some(INK_BLACK);
        v.weak_text_color = Some(INK_SECONDARY);
        // 选中/高亮统一走卡蓝；文字选中反白。
        v.selection.bg_fill = CARD_BLUE;
        v.selection.stroke = egui::Stroke::new(1.0, PAPER_WHITE);
        v.hyperlink_color = CARD_BLUE;
        // 控件直角色块（票纸/卡面材质是平的，不堆圆角）。
        let radius = egui::CornerRadius::same(2);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = radius;
        }
        // 普通按钮（软键）默认键面：票纸白 + 细墨边；按钮文字在局部作用域内换色。
        v.widgets.inactive.weak_bg_fill = PAPER_WHITE;
        v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, INK_RULE);
        v.widgets.hovered.weak_bg_fill = SOFT_HOVER;
        v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, CARD_BLUE);
        v.widgets.active.weak_bg_fill = SOFT_PRESSED;
        v.widgets.active.bg_stroke = egui::Stroke::new(1.0, CARD_BLUE);
        // ScrollArea 的悬浮滑杆取 fg_stroke：默认墨色，悬停卡蓝。
        v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, INK_SECONDARY);
        v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, CARD_BLUE);
        v.widgets.active.fg_stroke = egui::Stroke::new(1.5, CARD_BLUE);

        style.visuals = v;
        style.spacing.item_spacing = egui::vec2(8.0, 4.0);
        style.spacing.button_padding = egui::vec2(12.0, 5.0);
    });
    ctx.set_theme(egui::Theme::Light);
}

/// 学号：空值视为无卡。
fn load_student_id() -> Option<String> {
    crate::config::Config::load(std::path::Path::new(CFG_PATH))
        .ok()
        .map(|c| c.account.student_id)
        .filter(|s| !s.trim().is_empty())
}

/// 配置 mtime：R4——每帧一次 stat，变化才重读学号。
fn config_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(CFG_PATH).and_then(|m| m.modified()).ok()
}

struct Gui {
    snapshot: SharedSnapshot,
    redial_tx: Sender<()>,
    setmode_tx: Sender<NetMode>,
    /// 卡面学号（R4：配置 mtime 变化时重读）。
    student_id: Option<String>,
    cfg_mtime: Option<std::time::SystemTime>,
    /// CJK 字体是否可用；false 时整页英文提示，避免豆腐块。
    font_ok: bool,
    /// 状态章重盖动效进度（0→1）；状态字变化时归零。
    stamp_t: f32,
    /// 上一帧的状态字，用于检测"重盖"。
    last_word: Option<String>,
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

        // 修改账号密码后配置变了：mtime 变化才重读（ruling R4）。
        let mtime = config_mtime();
        if mtime != self.cfg_mtime {
            self.student_id = load_student_id();
            self.cfg_mtime = mtime;
        }

        let snap = self.snapshot.lock().ok().and_then(|g| g.clone());
        let view = status_view(snap.as_ref());

        // 状态章重盖：状态字变化时重新压一枚章（短促淡入 + 压印）。
        if self.last_word.as_deref() != Some(view.word) {
            self.last_word = Some(view.word.to_string());
            self.stamp_t = 0.0;
        }
        self.stamp_t = (self.stamp_t + ctx.input(|i| i.stable_dt)).min(1.0);
        if self.stamp_t < 1.0 {
            ctx.request_repaint();
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(PAPER_WHITE)
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ui, |ui| {
                if !self.font_ok {
                    no_font_notice(ui);
                    return;
                }
                // 顶/中/底三段自己管间距：默认 item_spacing 会在软键排后多垫 4px，
                // 把蓝条挤出窗口底边。
                ui.spacing_mut().item_spacing.y = 0.0;
                card_face(ui, &view, self.student_id.as_deref(), self.stamp_t);
                // 小票占满到软键排之上；内容超出可滑动（像小票继续吐纸）。
                let body_h = (ui.available_height() - SOFT_ROW_H).max(0.0);
                ui.allocate_ui(egui::vec2(ui.available_width(), body_h), |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| match snap.as_ref() {
                            Some(s) => {
                                receipt(ui, s, &view, &self.redial_tx, &self.setmode_tx, body_h)
                            }
                            None => service_down(ui, &view),
                        });
                });
                // 软键排占满剩余高度（蓝条一直铺到窗口底边）。
                ui.allocate_ui(egui::vec2(ui.available_width(), SOFT_ROW_H), soft_keys);
            });
    }
}

// ---- 状态呈现（章 / 灯 / 大字，一处定义）----

/// 一屏的状态呈现：章上的字与墨色、读卡灯、票面大字颜色、出口。
struct StatusView {
    word: &'static str,
    stamp_fill: egui::Color32,
    stamp_ink: egui::Color32,
    word_color: egui::Color32,
    /// None = 空环（空闲/未运行）。
    light: Option<egui::Color32>,
    egress: &'static str,
}

/// 状态 → 呈现。优先级：有线已连接 > 无线已接管 > 有线进行中/失败/空闲。
fn status_view(s: Option<&StateSnapshot>) -> StatusView {
    let Some(s) = s else {
        return StatusView {
            word: "服务未运行",
            stamp_fill: VERMILION,
            stamp_ink: PAPER_WHITE,
            word_color: VERMILION,
            light: None,
            egress: "—",
        };
    };
    if s.status == SessionStatus::Connected {
        return StatusView {
            word: "已连接",
            stamp_fill: READER_GREEN,
            // 绿底上用墨色章字：白字对读卡绿只有 3.4:1，墨字 6.5:1。
            stamp_ink: INK_BLACK,
            word_color: READER_GREEN,
            light: Some(READER_GREEN),
            egress: "有线",
        };
    }
    if s.wireless.phase == WPhase::Online {
        return StatusView {
            word: "无线接管",
            stamp_fill: READER_GREEN,
            stamp_ink: INK_BLACK,
            word_color: READER_GREEN,
            light: Some(READER_GREEN),
            egress: "无线",
        };
    }
    let (word, fill, ink, color, light) = match s.status {
        SessionStatus::Dialing => (
            "拨号中",
            INK_BLACK,
            PAPER_WHITE,
            INK_BLACK,
            Some(PAPER_WHITE),
        ),
        SessionStatus::Backoff => (
            "重拨中",
            INK_BLACK,
            PAPER_WHITE,
            INK_BLACK,
            Some(PAPER_WHITE),
        ),
        SessionStatus::AuthFail => (
            "认证失败",
            VERMILION,
            PAPER_WHITE,
            VERMILION,
            Some(VERMILION),
        ),
        // 空闲：空白章（纸面 + 墨字），像一张未启用的卡。
        SessionStatus::Idle => ("空闲", PAPER_WHITE, INK_BLACK, INK_BLACK, None),
        SessionStatus::Connected => unreachable!("connected handled above"),
    };
    StatusView {
        word,
        stamp_fill: fill,
        stamp_ink: ink,
        word_color: color,
        light,
        egress: "—",
    }
}

// ---- 顶部卡面 ----

/// 卡面：卡蓝地、读卡灯、学号如卡号、状态章。
fn card_face(ui: &mut egui::Ui, view: &StatusView, student_id: Option<&str>, stamp_t: f32) {
    egui::Frame::new()
        .fill(CARD_BLUE)
        .inner_margin(egui::Margin::symmetric(18, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // 行高由状态章决定；灯与题字固定行高内垂直居中（egui 的横排
            // 对"先放的小件"不会事后居中，所以这里显式给高度）。
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 36.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    reader_light(ui, view.light);
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new("GDUT Net · 校园网一卡通")
                            .color(CARD_DIM)
                            .size(13.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        status_stamp(ui, view, stamp_t);
                    });
                },
            );
            ui.add_space(8.0);
            ui.label(egui::RichText::new("学号").color(CARD_DIM).size(12.0));
            ui.add_space(1.0);
            let id = student_id.unwrap_or("—");
            let id_text = if id == "—" {
                id.to_string()
            } else {
                grouped_id(id)
            };
            ui.add(
                egui::Label::new(
                    egui::RichText::new(id_text)
                        .color(PAPER_WHITE)
                        .monospace()
                        .size(24.0),
                )
                .truncate(),
            );
        });
}

/// 读卡灯：绿=在线、白=进行中、红=失败、空环=空闲（只有这四种）。
fn reader_light(ui: &mut egui::Ui, light: Option<egui::Color32>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    let c = rect.center();
    let painter = ui.painter();
    match light {
        Some(color) => {
            painter.circle_filled(c, 5.0, color);
            painter.circle_stroke(c, 6.0, egui::Stroke::new(1.0, color.gamma_multiply(0.45)));
        }
        None => {
            painter.circle_stroke(c, 5.5, egui::Stroke::new(1.2, CARD_DIM));
        }
    }
}

/// 状态章：色块 + 内衬细线 + 章字，轻微歪斜像手盖的；状态变化时淡入压印。
fn status_stamp(ui: &mut egui::Ui, view: &StatusView, t: f32) {
    let alpha = 0.35 + 0.65 * t;
    let grow = 1.0 + 0.06 * (1.0 - t);
    let galley = ui.painter().layout_no_wrap(
        view.word.to_string(),
        egui::FontId::proportional(15.0),
        view.stamp_ink,
    );
    let base = galley.size() + egui::vec2(22.0, 12.0);
    let size = base * grow + egui::vec2(4.0, 6.0); // 压章/歪斜的余量
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let center = rect.center();
    let angle = -0.05; // 手盖章的歪斜（逆时针约 3°）
    let rot = egui::emath::Rot2::from_angle(angle);

    let fill = view.stamp_fill.gamma_multiply(alpha);
    let line = view.stamp_ink.gamma_multiply(0.75 * alpha);
    let poly = |half: egui::Vec2| -> Vec<egui::Pos2> {
        [
            egui::vec2(-half.x, -half.y),
            egui::vec2(half.x, -half.y),
            egui::vec2(half.x, half.y),
            egui::vec2(-half.x, half.y),
        ]
        .into_iter()
        .map(|v| center + rot * v)
        .collect()
    };

    let painter = ui.painter();
    painter.add(egui::Shape::convex_polygon(
        poly(base * 0.5 * grow),
        fill,
        egui::Stroke::NONE,
    ));
    painter.add(egui::Shape::closed_line(
        poly((base - egui::vec2(7.0, 7.0)) * 0.5 * grow),
        egui::Stroke::new(1.0, line),
    ));
    // 歪斜绕章心：TextShape 的 angle 绕 pos 转，先把 pos 反推到"转后中心=章心"。
    let pivot = egui::Align2::CENTER_CENTER
        .pos_in_rect(&galley.rect)
        .to_vec2();
    painter.add(egui::Shape::Text(
        egui::epaint::TextShape::new(center - rot * pivot, galley, view.stamp_ink)
            .with_angle(angle)
            .with_opacity_factor(alpha),
    ));
}

// ---- 中部小票 ----

/// 一张小票：大字状态 + 出口、虚线字段行、立即重拨、模式选择、事件流水。
///
/// `body_h` 是小票区的固定高度（App 先扣掉软键排）：事件滚动区按“固定内容
/// 用掉多少、剩多少”精确分配，外层 ScrollArea 只作溢出兜底。
fn receipt(
    ui: &mut egui::Ui,
    s: &StateSnapshot,
    view: &StatusView,
    redial_tx: &Sender<()>,
    setmode_tx: &Sender<NetMode>,
    body_h: f32,
) {
    ui.spacing_mut().item_spacing.y = 2.0;
    let margin = egui::Margin::symmetric(18, 0);
    let frame = egui::Frame::new().inner_margin(margin);
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add_space(8.0);
        status_row(ui, view);
        ui.add_space(6.0);
        dashed_rule(ui);
        ui.add_space(6.0);
        field_rows(ui, s);
        ui.add_space(8.0);
        dashed_rule(ui);
        ui.add_space(8.0);
        if primary_button(ui, "立即重拨").clicked() {
            let _ = redial_tx.send(());
        }
        ui.add_space(6.0);
        mode_radios(ui, s.mode, setmode_tx);
        ui.add_space(8.0);
        dashed_rule(ui);
        ui.add_space(6.0);
        // 事件流水：先占满标题行，剩下的高度全给滚动区。
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("最近事件")
                    .color(INK_BLACK)
                    .size(13.0)
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new("最新在上")
                        .color(INK_SECONDARY)
                        .size(11.0),
                );
            });
        });
        ui.add_space(2.0);
        // 事件流水按剩余高度给滚动区（不是整张小票滚动）；最少 40 保留可读性。
        let used = ui.min_rect().height();
        let stream_h = (body_h - used - 2.0).max(40.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(stream_h)
            .show(ui, |ui| {
                if s.events.is_empty() {
                    ui.label(
                        egui::RichText::new("（暂无事件）")
                            .color(INK_SECONDARY)
                            .size(12.0),
                    );
                }
                for line in s.events.iter().rev() {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(line)
                                .monospace()
                                .color(INK_SECONDARY)
                                .size(12.0),
                        )
                        .truncate(),
                    );
                }
            });
    });
}

/// 大字状态 + 出口（出口未知时如实写 —）。
fn status_row(ui: &mut egui::Ui, view: &StatusView) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(view.word)
                .color(view.word_color)
                .size(28.0)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("出口 · {}", view.egress))
                    .color(INK_SECONDARY)
                    .size(13.0),
            );
        });
    });
}

/// 虚线小票行：一行为一段（label 次级墨色，值墨黑；数字走等宽）。
fn field_rows(ui: &mut egui::Ui, s: &StateSnapshot) {
    let heartbeat = match &s.heartbeat {
        HeartbeatStatus::Off => "关闭".to_string(),
        HeartbeatStatus::Running => "运行中".to_string(),
        HeartbeatStatus::Error(e) => format!("错误（{e}）"),
    };
    let wireless = wireless_zh(s);
    let rows: [(&str, String, bool); 5] = [
        ("IP", s.ip.clone().unwrap_or_else(|| "—".into()), true),
        ("在线时长", s.uptime_text(), true),
        (
            "上次掉线",
            s.last_drop_reason.clone().unwrap_or_else(|| "—".into()),
            false,
        ),
        ("心跳", heartbeat, false),
        ("无线", wireless, false),
    ];
    egui::Grid::new("receipt_fields")
        .num_columns(2)
        .spacing([14.0, 2.0])
        .show(ui, |ui| {
            for (label, value, mono) in rows {
                ui.label(egui::RichText::new(label).color(INK_SECONDARY).size(13.0));
                let text = if mono {
                    egui::RichText::new(value)
                        .monospace()
                        .color(INK_BLACK)
                        .size(13.0)
                } else {
                    egui::RichText::new(value).color(INK_BLACK).size(13.0)
                };
                ui.add(egui::Label::new(text).truncate());
                ui.end_row();
            }
        });
}

/// 无线字段的中文短语（含 IP / 错误）。
fn wireless_zh(s: &StateSnapshot) -> String {
    let phase = match s.wireless.phase {
        WPhase::Off => "关闭",
        WPhase::Joining => "连接中",
        WPhase::Authing => "认证中",
        WPhase::Online => "已接管",
        WPhase::Error => "错误",
    };
    match (&s.wireless.ip, &s.wireless.last_error) {
        (Some(ip), _) => format!("{phase} {ip}"),
        (None, Some(e)) => format!("{phase}（{e}）"),
        _ => phase.to_string(),
    }
}

/// 服务未运行页：大字状态 + 说明 + 启动服务（主按钮）。
fn service_down(ui: &mut egui::Ui, view: &StatusView) {
    ui.spacing_mut().item_spacing.y = 2.0;
    let frame = egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 0));
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add_space(8.0);
        status_row(ui, view);
        ui.add_space(6.0);
        dashed_rule(ui);
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("后台服务没有运行，网络不会自动拨号。")
                .color(INK_BLACK)
                .size(14.0),
        );
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new("启动服务会请求一次管理员授权（UAC）。")
                .color(INK_SECONDARY)
                .size(12.5),
        );
        ui.add_space(12.0);
        if primary_button(ui, "启动服务").clicked() {
            launch_setup(&["--start-service"]);
        }
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("如果刚卸载或重装过，也可以从开始菜单重新运行安装程序。")
                .color(INK_SECONDARY)
                .size(12.5),
        );
    });
}

// ---- 底部软键排 ----

/// 圈存机软键排：卡蓝机体 + 票纸白键面；右侧版本号。
fn soft_keys(ui: &mut egui::Ui) {
    egui::Frame::new()
        .fill(CARD_BLUE)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            // 铺满分配到的整条（蓝条到窗口底）。
            ui.set_min_size(ui.available_size());
            ui.horizontal(|ui| {
                if soft_key(ui, "修改账号密码").clicked() {
                    launch_setup(&["--repair"]);
                }
                if soft_key(ui, "打开日志目录").clicked() {
                    open_logs();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                            .color(CARD_DIM)
                            .size(12.0),
                    );
                });
            });
        });
}

/// 单枚软键：票纸白键面、卡蓝字；hover 上蓝、pressed 压深。
fn soft_key(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.scope(|ui| {
        ui.visuals_mut().override_text_color = Some(CARD_BLUE);
        ui.add(egui::Button::new(
            egui::RichText::new(label).size(13.0).strong(),
        ))
    })
    .inner
}

/// 主按钮：卡蓝块、票纸白字（hover/pressed 由全局弱底色给）。
fn primary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let size = egui::vec2(ui.available_width(), 30.0);
    ui.scope(|ui| {
        let v = ui.visuals_mut();
        v.widgets.inactive.weak_bg_fill = CARD_BLUE;
        v.widgets.hovered.weak_bg_fill = CARD_BLUE_HOVER;
        v.widgets.active.weak_bg_fill = CARD_BLUE_PRESSED;
        v.widgets.inactive.bg_stroke = egui::Stroke::NONE;
        v.widgets.hovered.bg_stroke = egui::Stroke::NONE;
        v.widgets.active.bg_stroke = egui::Stroke::NONE;
        ui.add_sized(
            size,
            egui::Button::new(
                egui::RichText::new(label)
                    .color(PAPER_WHITE)
                    .size(15.0)
                    .strong(),
            ),
        )
    })
    .inner
}

/// 模式单选（卡蓝圆点）。
fn mode_radios(ui: &mut egui::Ui, mode: NetMode, setmode_tx: &Sender<NetMode>) {
    ui.scope(|ui| {
        let v = ui.visuals_mut();
        v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, CARD_BLUE);
        v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, CARD_BLUE);
        v.widgets.active.fg_stroke = egui::Stroke::new(1.0, CARD_BLUE);
        v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, INK_SECONDARY);
        v.widgets.hovered.bg_stroke = egui::Stroke::new(1.5, CARD_BLUE);
        v.widgets.active.bg_stroke = egui::Stroke::new(1.5, CARD_BLUE);
        ui.horizontal(|ui| {
            if ui
                .radio(
                    mode == NetMode::WiredExclusive,
                    egui::RichText::new("有线优先（自动无线接管）").size(13.0),
                )
                .clicked()
            {
                let _ = setmode_tx.send(NetMode::WiredExclusive);
            }
            if ui
                .radio(
                    mode == NetMode::WiredPlusStandby,
                    egui::RichText::new("有线 + 无线备用").size(13.0),
                )
                .clicked()
            {
                let _ = setmode_tx.send(NetMode::WiredPlusStandby);
            }
        });
    });
}

// ---- 小票零件 ----

/// 一条虚线（小票分节）。
fn dashed_rule(ui: &mut egui::Ui) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    let y = rect.center().y;
    let shapes = egui::Shape::dashed_line(
        &[egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
        egui::Stroke::new(1.0, INK_RULE),
        5.0,
        4.0,
    );
    ui.painter().extend(shapes);
}

/// 字体缺失页：默认字体无 CJK，只能用英文说明（保证不是豆腐块）。
fn no_font_notice(ui: &mut egui::Ui) {
    ui.add_space(24.0);
    ui.colored_label(
        VERMILION,
        "No CJK font found (checked %SystemRoot%\\Fonts). Chinese text cannot be displayed.",
    );
}

/// 卡号式分组：每 4 个字符一组（"3124000000" → "3124 0000 00"）。
fn grouped_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    chars
        .chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

// ---- 外部动作 ----

/// 改密码 / 启动服务：拉起安装目录的 setup（它自提权，弹一次 UAC）。
fn launch_setup(args: &[&str]) {
    let exe = crate::setup::install_dir().join("gdut-net-setup.exe");
    match std::process::Command::new(&exe).args(args).spawn() {
        Ok(_) => log::info!("Launched setup {args:?}"),
        Err(e) => log::error!("Failed to launch setup {}: {e}", exe.display()),
    }
}

/// 打开日志目录（资源管理器）。
fn open_logs() {
    let dir = std::path::PathBuf::from(LOG_DIR);
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::process::Command::new("explorer.exe").arg(&dir).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::protocol::WirelessSnapshot;

    fn snap(status: SessionStatus, phase: WPhase) -> StateSnapshot {
        StateSnapshot {
            status,
            since_unix: None,
            ip: None,
            last_drop_reason: None,
            redial_attempts: 0,
            heartbeat: HeartbeatStatus::Off,
            mode: NetMode::WiredExclusive,
            wireless: WirelessSnapshot {
                phase,
                ip: None,
                last_error: None,
            },
            events: Default::default(),
        }
    }

    #[test]
    fn status_words_cover_all_states() {
        assert_eq!(status_view(None).word, "服务未运行");
        assert_eq!(status_view(None).egress, "—");
        assert_eq!(
            status_view(Some(&snap(SessionStatus::Connected, WPhase::Off))).word,
            "已连接"
        );
        assert_eq!(
            status_view(Some(&snap(SessionStatus::Dialing, WPhase::Off))).word,
            "拨号中"
        );
        assert_eq!(
            status_view(Some(&snap(SessionStatus::Backoff, WPhase::Off))).word,
            "重拨中"
        );
        assert_eq!(
            status_view(Some(&snap(SessionStatus::AuthFail, WPhase::Off))).word,
            "认证失败"
        );
        assert_eq!(
            status_view(Some(&snap(SessionStatus::Idle, WPhase::Off))).word,
            "空闲"
        );
    }

    #[test]
    fn wireless_takeover_wins_until_wired_connects() {
        let taken = status_view(Some(&snap(SessionStatus::Backoff, WPhase::Online)));
        assert_eq!(taken.word, "无线接管");
        assert_eq!(taken.egress, "无线");
        let wired = status_view(Some(&snap(SessionStatus::Connected, WPhase::Online)));
        assert_eq!(wired.word, "已连接");
        assert_eq!(wired.egress, "有线");
    }

    #[test]
    fn busy_states_read_as_busy_not_absent() {
        for st in [SessionStatus::Dialing, SessionStatus::Backoff] {
            assert_eq!(status_view(Some(&snap(st, WPhase::Off))).egress, "—");
        }
        // 读卡灯只有四种：在线绿、进行中白、失败红、空闲空环。
        assert_eq!(
            status_view(Some(&snap(SessionStatus::Dialing, WPhase::Off))).light,
            Some(PAPER_WHITE)
        );
        assert_eq!(
            status_view(Some(&snap(SessionStatus::AuthFail, WPhase::Off))).light,
            Some(VERMILION)
        );
        assert!(status_view(Some(&snap(SessionStatus::Idle, WPhase::Off)))
            .light
            .is_none());
    }

    #[test]
    fn grouped_id_prints_like_card_number() {
        assert_eq!(grouped_id("3124000000"), "3124 0000 00");
        assert_eq!(grouped_id(""), "");
        assert_eq!(grouped_id("1234567890123456"), "1234 5678 9012 3456");
    }
}
