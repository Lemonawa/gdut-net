//! 安装事务模块：唯一拥有安装步骤顺序、事件与回滚实情。
//!
//! GUI 与 silent 只是 reporter adapter；它们不再各自复述安装顺序。

use std::time::Duration;

use anyhow::{Context, Result};

use crate::service::{
    self, capture_prev_service, rollback_install, Credential,
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
    // Validate every payload hash and capture all old bytes/credentials while
    // the old service is still available. Bad packages must not take it down.
    let prepared = (|| -> Result<_> {
        anyhow::ensure!(
            !matches!(prev, service::PrevService::Unknown),
            "Cannot safely back up service state; retry when service status is available"
        );
        let mut entries = work::load_payload()?;
        anyhow::ensure!(
            entries.iter().any(|e| e.name == "gdut-net.exe"),
            "Payload is missing gdut-net.exe"
        );
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
            anyhow::ensure!(
                !entries
                    .iter()
                    .any(|e| e.name.eq_ignore_ascii_case("gdut-net-setup.exe")),
                "Payload must not overwrite the installer"
            );
            entries.push(crate::payload::Entry {
                name: "gdut-net-setup.exe".into(),
                data: std::fs::read(&self_exe).context("Failed to read installer")?,
            });
        }
        let dir = install_dir();
        let files =
            super::file_backup::FileBackup::capture(entries.iter().map(|e| dir.join(&e.name)))?;
        let core = service::InstallBackup::capture(&config_path())?;
        let shell = crate::shell::ShellInstallBackup::capture()?;
        Ok((entries, files, core, shell))
    })();
    let (entries, files, core, shell) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            reporter.report(InstallEvent::Done {
                result: Err(error),
                rollback: RollbackOutcome::NotNeeded,
            });
            return;
        }
    };
    let mut files_changed = false;
    let mut core_changed = false;
    let mut stopped = false;
    let mut shell_changed = false;
    let result: Result<()> = (|| {
        step(&mut reporter, work::STEP_STOP_SERVICE, "停止旧服务");
        stopped = true;
        service::stop_service(Duration::from_secs(16))?;
        work::kill_tray();
        done(&mut reporter, work::STEP_STOP_SERVICE, "停止旧服务");

        step(&mut reporter, work::STEP_UNPACK, "解包文件");
        let dir = install_dir();
        files_changed = true;
        unpack_files(&dir, &entries)
            .with_context(|| format!("Failed to unpack into {}", dir.display()))?;
        done(&mut reporter, work::STEP_UNPACK, "解包文件");

        step(&mut reporter, work::STEP_INSTALL_CORE, "写入配置并注册服务");
        let credential = match password {
            Some(p) => Credential::Plain(p),
            None => Credential::KeepExisting,
        };
        // The outer transaction owns rollback. Do not restart the old service
        // until its executable and config have both been restored.
        core_changed = true;
        service::install_core(ServiceInstallRequest {
            cfg_path: config_path(),
            student_id: Some(student_id.clone()),
            credential,
            service_exe: crate::paths::install_exe(),
            tray_exe: crate::paths::install_exe(),
        })?;
        done(&mut reporter, work::STEP_INSTALL_CORE, "写入配置并注册服务");

        step(
            &mut reporter,
            work::STEP_SHELL_INTEGRATION,
            "创建开始菜单快捷方式",
        );
        shell_changed = true;
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
        Err(ref error) if stopped => {
            log::error!("Install failed (rolling back): {error:#}");
            let mut failures = Vec::new();
            // StartService can fail after partially starting the process.
            if let Err(e) = service::stop_service(Duration::from_secs(16)) {
                failures.push(format!("Failed to stop service for rollback: {e:#}"));
            }
            if files_changed {
                if let Err(e) = files.restore() {
                    failures.push(format!("{e:#}"));
                }
            }
            if core_changed {
                if let Err(e) = core.restore() {
                    failures.push(format!("{e:#}"));
                }
            }
            let service_rollback = rollback_install(&prev, &config_path());
            if let RollbackOutcome::Failed(ref e) = service_rollback {
                failures.push(e.clone());
            }
            if shell_changed {
                if let Err(e) = shell.restore() {
                    failures.push(format!("{e:#}"));
                }
            }
            if failures.is_empty() {
                service_rollback
            } else {
                RollbackOutcome::Failed(failures.join("; "))
            }
        }
        Err(_) => RollbackOutcome::NotNeeded,
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

fn unpack_files(dir: &std::path::Path, entries: &[crate::payload::Entry]) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    for entry in entries {
        let dest = dir.join(&entry.name);
        super::file_backup::replace_file(&dest, &entry.data)
            .with_context(|| format!("Failed to write {}", dest.display()))?;
    }
    Ok(())
}
