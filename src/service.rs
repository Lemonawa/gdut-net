//! Windows 服务：install / uninstall / service_main。
//!
//! install：管理员校验 → 凭据（DPAPI）→ 拨号条目 → 创建服务（自启动 + 3 段失败恢复）→
//! Event Source 注册表。uninstall 全程幂等宽容。service_main 走 windows-service 分发器，
//! Stop 事件经 CancellationToken 通知运行时退出。
//!
//! 仅 Windows 编译，以 `cargo check --target x86_64-pc-windows-msvc` 验证。

#[cfg(windows)]
mod win {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    use anyhow::{anyhow, bail, Context, Result};
    use tokio_util::sync::CancellationToken;
    use windows::Win32::Foundation::{ERROR_SERVICE_DOES_NOT_EXIST, ERROR_SERVICE_EXISTS};
    use windows::Win32::UI::Shell::IsUserAnAdmin;
    use windows_service::service::{
        ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
        ServiceErrorControl, ServiceFailureActions, ServiceFailureResetPeriod, ServiceInfo,
        ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    };
    use windows_service::service_control_handler::{
        self, ServiceControlHandlerResult, ServiceStatusHandle,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    use windows_service::{define_windows_service, service_dispatcher};

    use super::SERVICE_NAME;
    use crate::config::Config;
    use crate::eventlog;

    const RESET_PERIOD_SECS: u64 = 86400;
    /// uninstall 轮询 Stopped 的总时限。
    const STOP_TIMEOUT: Duration = Duration::from_secs(10);

    define_windows_service!(ffi_service_main, service_run);

    /// install 入口（`gdut-net install`）。
    pub fn install(cfg_path: &Path, password_stdin: bool) -> Result<()> {
        require_admin()?;

        // 既有配置复用学号；重装时 password_blob 一律重新覆盖。
        let mut cfg = if cfg_path.exists() {
            Config::load(cfg_path)?
        } else {
            Config::default()
        };
        let password = if password_stdin {
            read_stdin_password()?
        } else {
            rpassword::prompt_password("Enter password: ")?
        };
        if password.is_empty() {
            bail!("Password must not be empty");
        }
        if cfg.account.student_id.trim().is_empty() {
            cfg.account.student_id = prompt_nonempty("Enter student ID: ")?;
        }
        // 存量配置迁移：旧版 http_probe_url=9.9.9.9 被校园网墙，自动升级为 223.5.5.5
        if cfg.dial.http_probe_url == "http://9.9.9.9" {
            cfg.dial.http_probe_url = "http://223.5.5.5".into();
            log::info!("Auto-migrated http_probe_url: 9.9.9.9 -> 223.5.5.5");
        }
        cfg.account.password_blob = crate::crypto::protect(&password)?;
        cfg.save(cfg_path)?;

        // pbk 目录先建好，RAS 条目与日志目录都依赖它。
        let pbk_path = PathBuf::from(&cfg.dial.pbk_path);
        if let Some(parent) = pbk_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        crate::ras::ensure_entry(&cfg.dial.pbk_path, &cfg.dial.entry_name)?;
        crate::ras::set_credentials(
            &cfg.dial.pbk_path,
            &cfg.dial.entry_name,
            &cfg.account.student_id,
            &password,
        )?;

        create_service(cfg_path).context("Failed to create/update service")?;
        if let Err(e) = set_recovery_actions() {
            eprintln!("Warning: failed to set service recovery actions (ignored, service still usable): {e:#}");
            log::warn!("Failed to set service recovery actions (ignored): {e:#}");
        }
        if let Err(e) = eventlog::register_source() {
            eprintln!("Warning: failed to register event source (ignored): {e:#}");
            log::warn!("Failed to register event source (ignored): {e:#}");
        }

        println!("Install complete:");
        println!("  Service: {SERVICE_NAME} (auto-start, restart on failure 5s/30s/60s)");
        println!("  Config: {}", cfg_path.display());
        println!(
            "  Dial entry: {} ({})",
            cfg.dial.entry_name, cfg.dial.pbk_path
        );
        println!("Start service: net start {SERVICE_NAME}");
        println!("Note: after reinstall with new password, run net stop {SERVICE_NAME} && net start {SERVICE_NAME} to apply");
        Ok(())
    }

    /// uninstall 入口（`gdut-net uninstall [--purge]`）；每步幂等宽容。
    pub fn uninstall(cfg_path: &Path, purge: bool) -> Result<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .context("Failed to connect to service manager (administrator required)")?;

        let service_access =
            ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE;
        match manager.open_service(SERVICE_NAME, service_access) {
            Ok(service) => {
                if service.query_status()?.current_state != ServiceState::Stopped {
                    // stop 失败不中断：wait_stopped 兜底判定真实状态。
                    if let Err(e) = service.stop() {
                        log::warn!("Service stop request failed (polling status): {e}");
                    }
                    wait_stopped(&service)?;
                }
                service.delete().context("Failed to delete service")?;
                println!("Service removed");
            }
            // 仅"Service not found"属幂等场景；拒绝访问等真实错误照常上抛。
            Err(windows_service::Error::Winapi(e))
                if e.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST.0 as i32) =>
            {
                println!("Service not found, skipping")
            }
            Err(e) => return Err(anyhow!("Failed to open service: {e}")),
        }

        match eventlog::unregister_source() {
            Ok(()) => println!("Event source removed"),
            Err(e) => eprintln!("Failed to remove event source (ignored): {e}"),
        }
        match crate::crypto::delete_entropy() {
            Ok(()) => println!("Entropy removed"),
            Err(e) => eprintln!("Failed to remove entropy (ignored): {e}"),
        }

        if purge {
            let dir = program_data_dir(cfg_path);
            ensure_purge_safe(&dir)?;
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => println!("Removed {}", dir.display()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    println!("{} not found, skipping", dir.display())
                }
                Err(e) => return Err(anyhow!("Failed to remove {}: {e}", dir.display())),
            }
        }
        Ok(())
    }

    /// purge 防护：配置路径父目录可能被 --config 指到任意位置（极端：盘根），
    /// remove_dir_all 之前必须确认目录身份——目录名为 gdut-net（大小写不敏感），
    /// 否则拒绝删除。
    fn ensure_purge_safe(dir: &Path) -> Result<()> {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if name == "gdut-net" {
            return Ok(());
        }
        bail!(
            "Refusing to delete suspicious directory {} (not named gdut-net), please remove manually",
            dir.display()
        )
    }

    /// 服务运行入口（`gdut-net run`）：先分发器，成功即阻塞至服务停止。
    pub fn service_main() -> Result<()> {
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
            .context("Service dispatcher failed (must run as service)")
    }

    /// 服务真实主体：注册控制处理器 → Running → 跑运行时 → Stopped。
    /// 所有失败路径先经 stderr 兜底输出（logger 可能未装），再报 Stopped
    /// 后退出——绝不静默消失让 SCM 盲目重启。
    fn service_run(_arguments: Vec<OsString>) {
        let code = match run_service() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("gdut-net: service crashed: {e:#}");
                log::error!("Service crashed: {e:#}");
                // 非零码退出：让 SCM 的失败恢复动作（Restart 5s/30s/60s）接管。
                1
            }
        };
        // 无论成败都显式退出：曾观察到 SCM 已 Stopped 但进程残留（阻塞
        // 服务重建 1073），exit 兜底保证进程生命周期与 SCM 状态一致。
        std::process::exit(code);
    }

    fn run_service() -> Result<()> {
        let token = CancellationToken::new();
        let stop_token = token.clone();
        let status_handle =
            service_control_handler::register(SERVICE_NAME, move |event| match event {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    stop_token.cancel();
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            })?;

        status_handle.set_service_status(service_status(
            ServiceState::StartPending,
            0,
            Duration::from_secs(5),
        ))?;

        let cfg_path = std::env::args()
            .position(|a| a == "--config")
            .and_then(|i| std::env::args().nth(i + 1))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData\gdut-net\config.toml"));
        // logger 此刻尚未安装，log::error! 会静默 no-op：失败必须先
        // eprintln! 兜底，再报 Stopped 退出，给 SCM 与用户留诊断。
        let cfg = match Config::load(&cfg_path) {
            Ok(cfg) => cfg,
            Err(e) => {
                let ctx = anyhow::anyhow!(e)
                    .context(format!("Failed to load config: {}", cfg_path.display()));
                eprintln!("gdut-net: {ctx:#}");
                report_stopped(&status_handle)?;
                return Err(ctx);
            }
        };

        // 日志尽早初始化（配置加载后立即可用）；LoggerHandle 必须持有到
        // 进程退出（Drop 会 flush 并关闭 FileLogWriter），服务进程无优雅
        // drop 时机需求（Stopped 上报后即退出，OS 回收），forget 保活。
        let log_guard = match crate::logging::init_service_logging(&cfg.log) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("gdut-net: failed to init file logging: {e:#}");
                report_stopped(&status_handle)?;
                return Err(e).context("Failed to init file logging");
            }
        };
        std::mem::forget(log_guard);
        log::info!(
            "gdut-net service started (config: {}, log dir: {})",
            cfg_path.display(),
            cfg.log.dir
        );

        status_handle.set_service_status(service_status(
            ServiceState::Running,
            0,
            Duration::from_secs(0),
        ))?;

        let result = crate::runtime::start_all(cfg, token);

        report_stopped(&status_handle)?;
        result
    }

    /// 上报 Stopped（尽力而为：上报失败不影响原错误上抛）。
    fn report_stopped(status_handle: &ServiceStatusHandle) -> Result<()> {
        status_handle
            .set_service_status(service_status(
                ServiceState::Stopped,
                0,
                Duration::from_secs(0),
            ))
            .context("Failed to report Stopped status")
    }

    fn service_status(state: ServiceState, checkpoint: u32, wait_hint: Duration) -> ServiceStatus {
        ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: match state {
                ServiceState::Running => {
                    ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
                }
                _ => ServiceControlAccept::empty(),
            },
            exit_code: windows_service::service::ServiceExitCode::Win32(0),
            checkpoint,
            wait_hint,
            process_id: None,
        }
    }

    fn require_admin() -> Result<()> {
        if unsafe { IsUserAnAdmin() }.as_bool() {
            Ok(())
        } else {
            bail!("Administrator required: run PowerShell/cmd as administrator and retry")
        }
    }

    fn read_stdin_password() -> Result<String> {
        use std::io::BufRead;
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .context("Failed to read password from stdin")?;
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }

    fn prompt_nonempty(prompt: &str) -> Result<String> {
        loop {
            let s = rpassword::prompt_password(prompt)?;
            let s = s.trim().to_string();
            if !s.is_empty() {
                return Ok(s);
            }
            println!("Input must not be empty, try again");
        }
    }

    fn service_binary() -> Result<PathBuf> {
        std::env::current_exe().context("Failed to get current executable path")
    }

    fn create_service(cfg_path: &Path) -> Result<()> {
        let manager = ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        )
        .context("Failed to connect to service manager (administrator required)")?;

        let info = ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from("GDUT Wired Network Client"),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: service_binary()?,
            launch_arguments: vec![
                OsString::from("--config"),
                OsString::from(cfg_path.as_os_str()),
                OsString::from("run"),
            ],
            dependencies: vec![],
            account_name: None,
            account_password: None,
        };
        // 幂等：已存在（1073）不报错——先 stop 再 update_config 刷新二进制路径/参数，重装场景友好。
        match manager.create_service(&info, ServiceAccess::CHANGE_CONFIG) {
            Ok(_) => {}
            Err(windows_service::Error::Winapi(e))
                if e.raw_os_error() == Some(ERROR_SERVICE_EXISTS.0 as i32) =>
            {
                let existing = manager
                    .open_service(
                        SERVICE_NAME,
                        ServiceAccess::CHANGE_CONFIG | ServiceAccess::QUERY_STATUS,
                    )
                    .context("Service already exists, failed to open")?;
                let _ = existing.stop();
                for _ in 0..20 {
                    if existing.query_status()?.current_state == ServiceState::Stopped {
                        break;
                    }
                    sleep(Duration::from_millis(500));
                }
                existing
                    .change_config(&info)
                    .context("Service already exists, failed to update config")?;
                println!("Service already exists, config refreshed");
            }
            Err(e) => return Err(anyhow!("Failed to create service: {e}")),
        }
        Ok(())
    }

    /// 3 段失败恢复：Restart 5s/30s/60s，失败计数 24h 后清零。
    /// windows-service 0.8 已封装 ChangeServiceConfig2W（update_failure_actions）。
    fn set_recovery_actions() -> Result<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .context("Failed to connect to service manager")?;
        let service = manager
            .open_service(SERVICE_NAME, ServiceAccess::CHANGE_CONFIG)
            .context("Failed to open service for CHANGE_CONFIG")?;
        service.update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(RESET_PERIOD_SECS)),
            reboot_msg: None,
            command: None,
            actions: Some(vec![
                ServiceAction {
                    action_type: ServiceActionType::Restart,
                    delay: Duration::from_secs(5),
                },
                ServiceAction {
                    action_type: ServiceActionType::Restart,
                    delay: Duration::from_secs(30),
                },
                ServiceAction {
                    action_type: ServiceActionType::Restart,
                    delay: Duration::from_secs(60),
                },
            ]),
        })
        .context("Failed to update service failure actions")?;
        Ok(())
    }

    fn wait_stopped(service: &windows_service::service::Service) -> Result<()> {
        let start = Instant::now();
        loop {
            if service.query_status()?.current_state == ServiceState::Stopped {
                return Ok(());
            }
            if start.elapsed() >= STOP_TIMEOUT {
                bail!("Timed out waiting for service to stop (10s); service may be marked for deletion, will take effect after reboot");
            }
            sleep(Duration::from_millis(500));
        }
    }

    /// ProgramData 根：配置路径的上级目录（缺省 C:\ProgramData\gdut-net）。
    fn program_data_dir(cfg_path: &Path) -> PathBuf {
        cfg_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData\gdut-net"))
    }
}

#[cfg(windows)]
pub use win::{install, service_main, uninstall};

#[cfg(windows)]
pub const SERVICE_NAME: &str = "gdut-net";
