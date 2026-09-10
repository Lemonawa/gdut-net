//! 安装/修复/启动服务的工作流（后台线程 + 步骤事件 + 失败回滚）。

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::service::{self, Credential, InstallRequest};
use crate::setup::{config_path, install_dir, SetupArgs};

/// 工作线程 → UI 的事件（步骤开始 / 步骤完成 / 整体结果）。
pub enum Ev {
    Step(String),
    StepDone(String),
    Done(Result<(), String>),
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

    // 先记下旧服务路径（在解包覆盖之前）。路径为空 = 存在但读不到，回滚时按"没有"处理。
    let prev_exe = match service::install_state() {
        service::InstallState::Installed { service_exe, .. }
            if !service_exe.as_os_str().is_empty() =>
        {
            Some(service_exe)
        }
        _ => None,
    };

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
        Ok(()) => emit(&tx, Ev::Done(Ok(()))),
        Err(e) => {
            log::error!("Install failed (rolling back): {e:#}");
            if let Err(rb) = rollback_for(prev_exe.as_deref()) {
                log::error!("Rollback failed: {rb:#}");
            }
            emit(&tx, Ev::Done(Err(format!("{e:#}"))));
        }
    }
}

/// 失败回滚：把服务重新指回旧 exe（不重写配置）；原先没有服务则删除。
fn rollback_for(prev_exe: Option<&std::path::Path>) -> Result<()> {
    match prev_exe {
        Some(p) => service::restore_service_path(&config_path(), p),
        None => service::delete_service(),
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
                Ok(()) => emit(&tx, Ev::Done(Ok(()))),
                Err(e) => emit(&tx, Ev::Done(Err(format!("{e:#}")))),
            }
        })
        .expect("Failed to spawn start-service thread");
}
