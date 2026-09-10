//! 安装/修复/启动服务的工作流（后台线程 + 步骤事件 + 失败回滚）。

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::service::{self, Credential, InstallRequest};
use crate::setup::{config_path, install_dir, SetupArgs};

/// 工作线程 → UI 的事件（步骤开始 / 步骤完成 / 整体结果）。
pub enum Ev {
    Step(String),
    StepDone(String),
    /// 工作流结束：`result` 是安装结果，`rollback` 是回滚实情。
    /// UI / silent 必须按 `rollback` 陈述，不得默认"已回滚"。
    Done {
        result: Result<(), String>,
        rollback: RollbackOutcome,
    },
}

/// 失败后的回滚实情（UI/silent 据此说真话）。
#[derive(Clone)]
pub enum RollbackOutcome {
    /// 没有需要回滚的改动（安装成功，或失败发生在任何改动之前）。
    NotNeeded,
    /// 旧服务注册已恢复（或本次新建的服务已删除）；旧服务已尽力启动。
    Restored,
    /// 服务原本存在但路径读不出：未做任何改动（绝不删除）。
    RestoredUnknown,
    /// 回滚动作失败（含恢复注册后启动失败）。
    Failed(String),
}

fn emit(tx: &Sender<Ev>, ev: Ev) {
    let _ = tx.send(ev);
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
fn kill_tray() {
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

    let result: Result<()> = (|| {
        emit(&tx, Ev::Step("停止旧服务".into()));
        service::stop_service(Duration::from_secs(16))?;
        kill_tray();
        emit(&tx, Ev::StepDone("停止旧服务".into()));

        emit(&tx, Ev::Step("解包文件".into()));
        let dir = install_dir();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create {}", dir.display()))?;
        for entry in load_payload()? {
            let dest = dir.join(&entry.name);
            std::fs::write(&dest, &entry.data)
                .with_context(|| format!("Failed to write {}", dest.display()))?;
        }
        let self_exe = std::env::current_exe()?;
        let setup_dest = dir.join("gdut-net-setup.exe");
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
        emit(&tx, Ev::StepDone("解包文件".into()));

        emit(&tx, Ev::Step("写入配置并注册服务".into()));
        let credential = match password {
            Some(p) => Credential::Plain(p),
            None => Credential::KeepExisting,
        };
        service::install_core(InstallRequest {
            cfg_path: config_path(),
            student_id: Some(student_id),
            credential,
            service_exe: dir.join("gdut-net.exe"),
            tray_exe: dir.join("gdut-net.exe"),
        })?;
        emit(&tx, Ev::StepDone("写入配置并注册服务".into()));

        emit(&tx, Ev::Step("创建开始菜单快捷方式".into()));
        crate::shell::install_shell_integration(&dir, env!("CARGO_PKG_VERSION"))?;
        emit(&tx, Ev::StepDone("创建开始菜单快捷方式".into()));

        emit(&tx, Ev::Step("启动服务".into()));
        service::start_service()?;
        emit(&tx, Ev::StepDone("启动服务".into()));
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
            log::error!("Install failed (rolling back): {e:#}");
            let rollback = rollback_for(&prev);
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

/// 安装开始前的服务状态：回滚的唯一依据。
enum PrevService {
    /// 没有服务：失败时删除本次可能新建的服务（不存在视为成功）。
    None,
    /// 有服务且路径已知：失败时恢复注册并尽力启动。
    Known(PathBuf),
    /// 有服务但路径读不出：失败时不碰注册，也绝不删除。
    Unknown,
}

fn capture_prev_service() -> PrevService {
    match service::install_state() {
        service::InstallState::Installed {
            service_exe: Some(exe),
            ..
        } => PrevService::Known(exe),
        service::InstallState::Installed {
            service_exe: None, ..
        } => PrevService::Unknown,
        service::InstallState::NotInstalled => PrevService::None,
    }
}

/// 失败回滚：恢复旧服务注册（或删除新建服务），并回报回滚实情。
/// 服务原本存在但路径未知时不做任何动作——宁可不回滚，也不误删。
fn rollback_for(prev: &PrevService) -> RollbackOutcome {
    match prev {
        PrevService::Unknown => {
            log::warn!("Service existed but its path could not be read; not touching it");
            RollbackOutcome::RestoredUnknown
        }
        PrevService::None => match service::delete_service() {
            Ok(()) => RollbackOutcome::Restored,
            Err(e) => {
                log::error!("Rollback delete_service failed: {e:#}");
                RollbackOutcome::Failed(format!("{e:#}"))
            }
        },
        PrevService::Known(exe) => match service::restore_service_path(&config_path(), exe) {
            // 旧服务被本次安装停掉了：恢复注册后尽力把它拉起来。
            Ok(()) => match service::start_service() {
                Ok(()) => RollbackOutcome::Restored,
                Err(e) => {
                    log::error!("Rollback restored registration but service start failed: {e:#}");
                    RollbackOutcome::Failed(format!("服务注册已恢复，但启动失败：{e:#}"))
                }
            },
            Err(e) => {
                log::error!("Rollback restore_service_path failed: {e:#}");
                RollbackOutcome::Failed(format!("{e:#}"))
            }
        },
    }
}

/// 读一次服务快照（启动服务页显示拨号结果用）。
pub fn query_status_once() -> Result<crate::ipc::protocol::StateSnapshot> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let mut client = crate::ipc::client::PipeClient::connect()?;
        client.next_state().await
    })
}

/// 启动服务并等待连接（≤25s），结果经 Ev 回报。
pub fn spawn_start_service(tx: Sender<Ev>) {
    std::thread::Builder::new()
        .name("gdut-net-setup-start".into())
        .spawn(move || {
            emit(&tx, Ev::Step("启动 gdut-net 服务".into()));
            let result: Result<()> = (|| {
                service::start_service()?;
                emit(&tx, Ev::StepDone("启动 gdut-net 服务".into()));
                emit(&tx, Ev::Step("等待拨号结果".into()));
                let deadline = Instant::now() + Duration::from_secs(25);
                loop {
                    if let Ok(s) = query_status_once() {
                        use crate::ipc::protocol::SessionStatus::*;
                        if matches!(s.status, Connected | Backoff | AuthFail) {
                            emit(
                                &tx,
                                Ev::StepDone(format!("等待拨号结果（{}）", s.status_text())),
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
