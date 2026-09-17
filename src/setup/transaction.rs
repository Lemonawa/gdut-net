//! 安装事务模块：唯一拥有安装步骤顺序、事件与回滚实情。
//!
//! GUI 与 silent 只是 reporter adapter；它们不再各自复述安装顺序。

use std::time::Duration;

use anyhow::{Context, Result};

use crate::service::{
    self, capture_prev_service, install_with_rollback, rollback_install, Credential,
    InstallRequest as ServiceInstallRequest, RollbackOutcome,
};
use crate::setup::{config_path, install_dir, work, SetupArgs};

pub struct InstallRequest {
    pub student_id: String,
    pub password: Option<String>,
}

struct CoreInstallRequest {
    student_id: String,
    password: Option<String>,
}

pub enum InstallEvent {
    Step {
        key: &'static str,
        label: String,
    },
    StepDone {
        key: &'static str,
        label: String,
    },
    Done {
        result: Result<()>,
        rollback: RollbackOutcome,
    },
}

pub trait InstallReporter {
    fn report(&mut self, event: InstallEvent);
}

impl From<InstallRequest> for CoreInstallRequest {
    fn from(req: InstallRequest) -> Self {
        Self {
            student_id: req.student_id,
            password: req.password,
        }
    }
}

impl InstallReporter for std::sync::mpsc::Sender<work::Ev> {
    fn report(&mut self, event: InstallEvent) {
        let event = match event {
            InstallEvent::Step { key, label } => work::Ev::Step { key, label },
            InstallEvent::StepDone { key, label } => work::Ev::StepDone { key, label },
            InstallEvent::Done { result, rollback } => work::Ev::Done {
                result: result.map_err(|e| format!("{e:#}")),
                rollback,
            },
        };
        let _ = self.send(event);
    }
}

pub fn run_install(_args: &SetupArgs, req: InstallRequest, mut reporter: impl InstallReporter) {
    let CoreInstallRequest {
        student_id,
        password,
    } = CoreInstallRequest::from(req);
    let prev = capture_prev_service();
    let mut core_rollback = None;

    let result: Result<()> = (|| {
        step(&mut reporter, work::STEP_STOP_SERVICE, "停止旧服务");
        service::stop_service(Duration::from_secs(16))?;
        work::kill_tray();
        done(&mut reporter, work::STEP_STOP_SERVICE, "停止旧服务");

        step(&mut reporter, work::STEP_UNPACK, "解包文件");
        let dir = install_dir();
        unpack_files(&dir).with_context(|| format!("Failed to unpack into {}", dir.display()))?;
        copy_setup_into_install_dir()?;
        done(&mut reporter, work::STEP_UNPACK, "解包文件");

        step(&mut reporter, work::STEP_INSTALL_CORE, "写入配置并注册服务");
        let credential = match password {
            Some(p) => Credential::Plain(p),
            None => Credential::KeepExisting,
        };
        if let Err(f) = install_with_rollback(
            ServiceInstallRequest {
                cfg_path: config_path(),
                student_id: Some(student_id.clone()),
                credential,
                service_exe: crate::paths::install_exe(),
                tray_exe: crate::paths::install_exe(),
            },
            &prev,
        ) {
            core_rollback = Some(f.rollback);
            return Err(f.error);
        }
        done(&mut reporter, work::STEP_INSTALL_CORE, "写入配置并注册服务");

        step(
            &mut reporter,
            work::STEP_SHELL_INTEGRATION,
            "创建开始菜单快捷方式",
        );
        crate::shell::install_shell_integration(&dir, env!("CARGO_PKG_VERSION"))?;
        done(
            &mut reporter,
            work::STEP_SHELL_INTEGRATION,
            "创建开始菜单快捷方式",
        );

        step(&mut reporter, work::STEP_START_SERVICE, "启动服务");
        service::start_service()?;
        done(&mut reporter, work::STEP_START_SERVICE, "启动服务");
        Ok(())
    })();

    let rollback = match result {
        Ok(()) => RollbackOutcome::NotNeeded,
        Err(ref error) => match core_rollback {
            Some(rollback) => rollback,
            None => {
                log::error!("Install failed (rolling back): {error:#}");
                rollback_install(&prev, &config_path())
            }
        },
    };
    reporter.report(InstallEvent::Done { result, rollback });
}

fn step(reporter: &mut impl InstallReporter, key: &'static str, label: &str) {
    reporter.report(InstallEvent::Step {
        key,
        label: label.to_string(),
    });
}

fn done(reporter: &mut impl InstallReporter, key: &'static str, label: &str) {
    reporter.report(InstallEvent::StepDone {
        key,
        label: label.to_string(),
    });
}

fn unpack_files(dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    for entry in work::load_payload()? {
        let dest = dir.join(&entry.name);
        std::fs::write(&dest, &entry.data)
            .with_context(|| format!("Failed to write {}", dest.display()))?;
    }
    Ok(())
}

fn copy_setup_into_install_dir() -> Result<()> {
    let self_exe = std::env::current_exe()?;
    let setup_dest = crate::paths::setup_exe();
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
    Ok(())
}
