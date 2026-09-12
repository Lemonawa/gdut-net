//! 安装/修复/启动服务的工作流（后台线程 + 步骤事件 + 失败回滚）。

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::service::{
    self, capture_prev_service, install_with_rollback, rollback_install, Credential, InstallRequest,
};
use crate::setup::{config_path, install_dir, SetupArgs};

/// 失败回滚实情由 `service` 定义；UI 继续以 `work::RollbackOutcome` 引用。
pub use crate::service::RollbackOutcome;

/// 工作线程 → UI 的事件（步骤开始 / 步骤完成 / 整体结果）。
/// 步骤事件都带 `key`（稳定英文标识：UI 匹配、silent 输出）与 `label`（中文 UI 文案）。
pub enum Ev {
    Step {
        key: &'static str,
        label: String,
    },
    StepDone {
        key: &'static str,
        label: String,
    },
    /// 带结局的一步（卸载报告翻译用）：UI 按 Done/Skipped/Failed 选 ✓/跳过/失败 文案。
    StepFinished {
        key: &'static str,
        label: String,
        outcome: StepOutcome,
    },
    /// 工作流结束：`result` 是安装结果，`rollback` 是回滚实情。
    /// UI / silent 必须按 `rollback` 陈述，不得默认"已回滚"。
    Done {
        result: Result<(), String>,
        rollback: RollbackOutcome,
    },
}

// ---- 步骤键：UI 按 key 匹配步骤行；silent 经 `step_label_en(key)` 输出英文 ----

pub const STEP_STOP_SERVICE: &str = "stop_service";
pub const STEP_UNPACK: &str = "unpack";
pub const STEP_INSTALL_CORE: &str = "install_core";
pub const STEP_SHELL_INTEGRATION: &str = "shell_integration";
pub const STEP_START_SERVICE: &str = "start_service";
pub const STEP_WAIT_DIAL: &str = "wait_dial";
pub const STEP_UNINSTALL_STOP_SERVICE: &str = "uninstall_stop_service";
/// 报告四行键由 `service` 定义（`UninstallReport::rows`），重导出供 UI/silent 使用。
pub use crate::service::{
    STEP_UNINSTALL_AUTOSTART, STEP_UNINSTALL_ENTROPY, STEP_UNINSTALL_EVENT_SOURCE,
    STEP_UNINSTALL_SERVICE,
};
pub const STEP_UNINSTALL_PURGE: &str = "uninstall_purge";
pub const STEP_UNINSTALL_REMOVE_DIR: &str = "uninstall_remove_dir";

/// 步骤键 → 英文一行（silent 控制台输出；GBK 控制台防乱码）。
/// 未知键原样返回（不假装认识）；中文只留在 label 里，供 GUI 使用。
pub fn step_label_en(key: &str) -> &str {
    match key {
        STEP_STOP_SERVICE => "Stop old service",
        STEP_UNPACK => "Unpack files",
        STEP_INSTALL_CORE => "Write config and register service",
        STEP_SHELL_INTEGRATION => "Create Start Menu shortcuts",
        STEP_START_SERVICE => "Start service",
        STEP_WAIT_DIAL => "Wait for dial result",
        STEP_UNINSTALL_STOP_SERVICE => "Stop service",
        STEP_UNINSTALL_SERVICE => "Remove service",
        STEP_UNINSTALL_EVENT_SOURCE => "Remove event source",
        STEP_UNINSTALL_ENTROPY => "Remove encryption key",
        STEP_UNINSTALL_AUTOSTART => "Remove tray autostart",
        STEP_UNINSTALL_PURGE => "Delete config and logs",
        STEP_UNINSTALL_REMOVE_DIR => "Remove install directory",
        other => other,
    }
}

/// `service::Step` 的 UI 侧镜像（Ev 不直接持有 service 内部类型）。
pub enum StepOutcome {
    Done,
    Skipped,
    Failed(String),
}

fn emit(tx: &Sender<Ev>, ev: Ev) {
    let _ = tx.send(ev);
}

/// 发一条步骤开始事件（key 为稳定英文标识，label 为中文 UI 文案）。
fn step(tx: &Sender<Ev>, key: &'static str, label: &str) {
    emit(
        tx,
        Ev::Step {
            key,
            label: label.to_string(),
        },
    );
}

/// 发一条步骤完成事件。
fn step_done(tx: &Sender<Ev>, key: &'static str, label: &str) {
    emit(
        tx,
        Ev::StepDone {
            key,
            label: label.to_string(),
        },
    );
}

/// 发一条带结局的步骤事件（卸载报告翻译）。
fn step_finished(tx: &Sender<Ev>, key: &'static str, label: &str, outcome: StepOutcome) {
    emit(
        tx,
        Ev::StepFinished {
            key,
            label: label.to_string(),
            outcome,
        },
    );
}

/// 从 exe 尾读 payload；未打包时回退到 exe 旁 payload/ 目录（开发态）。
fn load_payload() -> Result<Vec<crate::payload::Entry>> {
    let exe = std::env::current_exe()?;
    let bytes = std::fs::read(&exe)?;
    match crate::payload::unpack(&bytes)? {
        Some(entries) => Ok(entries),
        None => {
            let dir = exe
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("payload");
            crate::packaging::collect_dir(&dir)
                .with_context(|| format!("Not packed and no dev payload dir at {}", dir.display()))
        }
    }
}

/// 杀掉旧托盘进程（镜像名 gdut-net.exe，不会命中 setup 自身）。
pub(crate) fn kill_tray() {
    use std::os::windows::process::CommandExt as _;
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/IM", "gdut-net.exe"])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .status();
}

/// 安装/修复：停旧服务 → 解包 → install_core → shell 集成 → 起服务。
/// 任一步失败：恢复旧服务路径（或删除新建服务）。
pub fn spawn_install(
    tx: Sender<Ev>,
    args: SetupArgs,
    student_id: String,
    password: Option<String>,
) {
    std::thread::Builder::new()
        .name("gdut-net-setup-install".into())
        .spawn(move || run_install(tx, args, student_id, password))
        .expect("Failed to spawn install thread");
}

fn run_install(tx: Sender<Ev>, args: SetupArgs, student_id: String, password: Option<String>) {
    // args 供调用方标记模式；凭据由 password 的 Option 显式表达（None = KeepExisting）。
    let _ = args;

    // 先记下安装前的服务状态（在解包覆盖之前）——回滚只能依据它。
    let prev = capture_prev_service();
    // install 核心失败时回滚已在 install_with_rollback 内完成；记下实情，
    // 收尾不再重复回滚。解包/快捷方式/起服务等后续失败才由收尾回滚。
    let mut core_rollback: Option<RollbackOutcome> = None;

    let result: Result<()> = (|| {
        step(&tx, STEP_STOP_SERVICE, "停止旧服务");
        service::stop_service(Duration::from_secs(16))?;
        kill_tray();
        step_done(&tx, STEP_STOP_SERVICE, "停止旧服务");

        step(&tx, STEP_UNPACK, "解包文件");
        let dir = install_dir();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create {}", dir.display()))?;
        for entry in load_payload()? {
            let dest = dir.join(&entry.name);
            std::fs::write(&dest, &entry.data)
                .with_context(|| format!("Failed to write {}", dest.display()))?;
        }
        let self_exe = std::env::current_exe()?;
        let setup_dest = crate::paths::setup_exe();
        // 从安装目录内运行（修复场景）：正在运行的文件不能覆盖，跳过自我拷贝。
        let same_file = match (
            std::fs::canonicalize(&self_exe),
            std::fs::canonicalize(&setup_dest),
        ) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        if !same_file {
            std::fs::copy(&self_exe, &setup_dest)
                .context("Failed to copy setup exe into install dir")?;
        }
        step_done(&tx, STEP_UNPACK, "解包文件");

        step(&tx, STEP_INSTALL_CORE, "写入配置并注册服务");
        let credential = match password {
            Some(p) => Credential::Plain(p),
            None => Credential::KeepExisting,
        };
        if let Err(f) = install_with_rollback(
            InstallRequest {
                cfg_path: config_path(),
                student_id: Some(student_id),
                credential,
                service_exe: crate::paths::install_exe(),
                tray_exe: crate::paths::install_exe(),
            },
            &prev,
        ) {
            core_rollback = Some(f.rollback);
            return Err(f.error);
        }
        step_done(&tx, STEP_INSTALL_CORE, "写入配置并注册服务");

        step(&tx, STEP_SHELL_INTEGRATION, "创建开始菜单快捷方式");
        crate::shell::install_shell_integration(&dir, env!("CARGO_PKG_VERSION"))?;
        step_done(&tx, STEP_SHELL_INTEGRATION, "创建开始菜单快捷方式");

        step(&tx, STEP_START_SERVICE, "启动服务");
        service::start_service()?;
        step_done(&tx, STEP_START_SERVICE, "启动服务");
        Ok(())
    })();

    match result {
        Ok(()) => emit(
            &tx,
            Ev::Done {
                result: Ok(()),
                rollback: RollbackOutcome::NotNeeded,
            },
        ),
        Err(e) => {
            let rollback = match core_rollback {
                // 核心失败的日志与回滚都在 install_with_rollback 内完成。
                Some(rb) => rb,
                None => {
                    // 后续步骤失败：此刻才回滚（日志文案与旧实现一致）。
                    log::error!("Install failed (rolling back): {e:#}");
                    rollback_install(&prev)
                }
            };
            emit(
                &tx,
                Ev::Done {
                    result: Err(format!("{e:#}")),
                    rollback,
                },
            );
        }
    }
}

/// 卸载：停服务/托盘 → uninstall_core（报告逐行翻译）→ 安排删除安装目录。
/// 卸载没有回滚概念：任何失败只记录并上报（幂等，可原样重试）。
pub fn spawn_uninstall(tx: Sender<Ev>, purge: bool, remove_dir: bool) {
    std::thread::Builder::new()
        .name("gdut-net-setup-uninstall".into())
        .spawn(move || run_uninstall(tx, purge, remove_dir))
        .expect("Failed to spawn uninstall thread");
}

fn run_uninstall(tx: Sender<Ev>, purge: bool, remove_dir: bool) {
    let result: Result<()> = (|| {
        step(&tx, STEP_UNINSTALL_STOP_SERVICE, "停止服务");
        service::stop_service(Duration::from_secs(16))?;
        kill_tray();
        step_done(&tx, STEP_UNINSTALL_STOP_SERVICE, "停止服务");

        // uninstall_core 返回分步报告；按规范行序（service::rows）逐行翻译成 UI 行。
        let report = service::uninstall_core(&config_path(), purge)?;
        for (key, step) in report.rows() {
            step_finished(&tx, key, label_zh(key), outcome_of(step));
        }
        if let Some(purge_step) = report.purge {
            step_finished(
                &tx,
                STEP_UNINSTALL_PURGE,
                "删除配置与日志",
                outcome_of(purge_step.step),
            );
        }

        if remove_dir {
            step(&tx, STEP_UNINSTALL_REMOVE_DIR, "移除安装目录");
            crate::shell::schedule_install_dir_removal(&install_dir())?;
            step_done(&tx, STEP_UNINSTALL_REMOVE_DIR, "移除安装目录");
        }
        Ok(())
    })();

    match result {
        Ok(()) => emit(
            &tx,
            Ev::Done {
                result: Ok(()),
                rollback: RollbackOutcome::NotNeeded,
            },
        ),
        Err(e) => {
            log::error!("Uninstall failed: {e:#}");
            emit(
                &tx,
                Ev::Done {
                    result: Err(format!("{e:#}")),
                    rollback: RollbackOutcome::NotNeeded,
                },
            );
        }
    }
}

/// 报告四行键 → 中文 UI 文案（silent 用 `step_label_en`，不经过这里）。
fn label_zh(key: &str) -> &str {
    match key {
        service::STEP_UNINSTALL_SERVICE => "移除服务",
        service::STEP_UNINSTALL_EVENT_SOURCE => "移除事件源",
        service::STEP_UNINSTALL_ENTROPY => "移除加密密钥",
        service::STEP_UNINSTALL_AUTOSTART => "移除托盘自启",
        other => other,
    }
}

/// `service::Step` → UI 结局（Failed 保留原因文本）。
fn outcome_of(step: service::Step) -> StepOutcome {
    match step {
        service::Step::Done => StepOutcome::Done,
        service::Step::Skipped => StepOutcome::Skipped,
        service::Step::Failed(e) => StepOutcome::Failed(e),
    }
}

/// 读一次服务快照（启动服务页显示拨号结果用）。
pub fn query_status_once() -> Result<crate::ipc::protocol::StateSnapshot> {
    crate::ipc::session::status_snapshot()
}

/// 启动服务并等待连接（≤25s），结果经 Ev 回报。
pub fn spawn_start_service(tx: Sender<Ev>) {
    std::thread::Builder::new()
        .name("gdut-net-setup-start".into())
        .spawn(move || {
            step(&tx, STEP_START_SERVICE, "启动 gdut-net 服务");
            let result: Result<()> = (|| {
                service::start_service()?;
                step_done(&tx, STEP_START_SERVICE, "启动 gdut-net 服务");
                step(&tx, STEP_WAIT_DIAL, "等待拨号结果");
                let deadline = Instant::now() + Duration::from_secs(25);
                loop {
                    if let Ok(s) = query_status_once() {
                        use crate::ipc::protocol::SessionStatus::*;
                        if matches!(s.status, Connected | Backoff | AuthFail) {
                            step_done(
                                &tx,
                                STEP_WAIT_DIAL,
                                &format!("等待拨号结果（{}）", crate::status::session_en(s.status)),
                            );
                            return Ok(());
                        }
                    }
                    if Instant::now() >= deadline {
                        bail!("25s 内未读到服务状态");
                    }
                    std::thread::sleep(Duration::from_millis(800));
                }
            })();
            match result {
                Ok(()) => emit(
                    &tx,
                    Ev::Done {
                        result: Ok(()),
                        rollback: RollbackOutcome::NotNeeded,
                    },
                ),
                Err(e) => emit(
                    &tx,
                    Ev::Done {
                        result: Err(format!("{e:#}")),
                        rollback: RollbackOutcome::NotNeeded,
                    },
                ),
            }
        })
        .expect("Failed to spawn start-service thread");
}
