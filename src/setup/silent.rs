//! --silent（英文输出，供迁移脚本/高级用户）：安装走 KeepExisting；卸载幂等。
//!
//! 输出约定（脚本可解析）：`== step` / `ok step`（步骤一律英文行，GBK 控制台防乱码）、
//! `FAILED: ...` / `ROLLBACK FAILED: ...` / `step failed: ...`（stderr）、
//! 末行 `Install complete: <dir>` / `Uninstall complete (purge=...)`（stdout）。
//! 绝不打印任何凭据。

use std::time::Duration;

use anyhow::{bail, Result};

use super::{config_path, install_dir, work, Mode, SetupArgs};

pub fn run(args: &SetupArgs, mode: Mode) -> Result<()> {
    match mode {
        Mode::SilentInstall => run_install(args),
        Mode::SilentUninstall => run_uninstall(args),
        _ => unreachable!("silent::run called with mode {mode:?}"),
    }
}

fn run_install(args: &SetupArgs) -> Result<()> {
    if !args.keep_password {
        bail!("Silent install requires --keep-password (no interactive input available)");
    }
    let (tx, rx) = std::sync::mpsc::channel();
    // 空学号被 install_core 忽略（仅非空覆盖）；配置里没有学号则报错——迁移语义。
    work::spawn_install(tx, args.clone(), String::new(), None);
    let mut ok = false;
    while let Ok(ev) = rx.recv() {
        match ev {
            // label 是中文 GUI 文案，绝不进控制台；这里只出 step_label_en(key)。
            work::Ev::Step { key, .. } => println!("== {}", work::step_label_en(key)),
            work::Ev::StepDone { key, .. } => println!("ok {}", work::step_label_en(key)),
            // R7：Ev::Done 携带回滚实情，silent 必须照实打印（英文）。
            work::Ev::Done {
                result: Err(e),
                rollback,
            } => {
                eprintln!("FAILED: {e}");
                match rollback {
                    work::RollbackOutcome::NotNeeded => {}
                    work::RollbackOutcome::Restored => {
                        eprintln!("Rolled back to the previous service.");
                    }
                    work::RollbackOutcome::RestoredUnknown => {
                        eprintln!("Service existed but its path was unreadable; left untouched.");
                    }
                    // 回滚失败详情可能含中文（核心/GUI 文案）：英文标签 + 原文，不翻译。
                    work::RollbackOutcome::Failed(r) => eprintln!("ROLLBACK FAILED: {r}"),
                }
                std::process::exit(1);
            }
            work::Ev::Done { result: Ok(()), .. } => {
                ok = true;
                break;
            }
            // 此变体只由卸载 worker 发出，安装工作流不使用。
            work::Ev::StepFinished { .. } => {}
        }
    }
    if !ok {
        // 线程 panic 等导致的提前断开：不能假装安装成功，脚本必须拿到非零码。
        bail!("Install worker exited unexpectedly (see logs)");
    }
    println!("Install complete: {}", install_dir().display());
    Ok(())
}

fn run_uninstall(args: &SetupArgs) -> Result<()> {
    // 停服务（未安装时 no-op）+ 杀托盘：托盘占用安装目录里的 exe，不杀则目录删不掉。
    crate::service::stop_service(Duration::from_secs(16))?;
    work::kill_tray();
    // purge 只在显式 --purge 时为 true；卸载核心幂等宽容，返回分步报告。
    let report = crate::service::uninstall_core(&config_path(), args.purge)?;
    // 被容忍的单步失败照实上报（英文 + 原文）；Done/Skipped 不打印。退出码语义不变。
    let steps = [
        (work::STEP_UNINSTALL_SERVICE, report.service),
        (work::STEP_UNINSTALL_EVENT_SOURCE, report.event_source),
        (work::STEP_UNINSTALL_ENTROPY, report.entropy),
        (work::STEP_UNINSTALL_AUTOSTART, report.autostart),
    ];
    for (key, step) in steps {
        if let crate::service::Step::Failed(e) = step {
            eprintln!("step failed: {}: {e}", work::step_label_en(key));
        }
    }
    if let Some(purge_step) = report.purge {
        if let crate::service::Step::Failed(e) = purge_step.step {
            eprintln!(
                "step failed: {}: {e}",
                work::step_label_en(work::STEP_UNINSTALL_PURGE)
            );
        }
    }
    crate::shell::schedule_install_dir_removal(&install_dir())?;
    println!("Uninstall complete (purge={})", args.purge);
    Ok(())
}
