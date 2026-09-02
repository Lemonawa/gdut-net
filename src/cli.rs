#[cfg(not(windows))]
use anyhow::bail;
use anyhow::Result;
use clap::{Parser, Subcommand};
#[derive(Parser)]
#[command(
    name = "gdut-net",
    version,
    about = "GDUT wired network third-party client"
)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        default_value = r"C:\ProgramData\gdut-net\config.toml"
    )]
    pub config: std::path::PathBuf,

    /// install 时从 stdin 读密码（非交互，适合脚本）
    #[arg(long, requires = "cmd", global = true)]
    pub password_stdin: bool,

    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// 以 Windows 服务方式运行（内部使用）
    Run,
    /// 安装服务、创建拨号条目、写配置
    Install,
    /// 卸载并清理
    Uninstall {
        /// 同时删除 ProgramData 下的配置与日志
        #[arg(long)]
        purge: bool,
    },
    /// 显示当前状态
    Status,
    /// 启动托盘（用户会话）
    Tray,
}

pub fn dispatch() -> Result<()> {
    let cli = Cli::parse();
    // Run 分支（服务分发器）不装 CLI 日志：真实日志由 run_service 拿到
    // 配置后按 LogCfg 初始化文件滚动日志（crate::logging::init_service_logging）；
    // 其余子命令（install/uninstall/status/tray）info 级 stderr 即可。
    if !matches!(cli.cmd, Cmd::Run) {
        crate::logging::init_cli_logging();
    }
    // Windows 控制台默认代码页 GBK，而我们的字符串字面量是 UTF-8，
    // 不设 65001 会输出乱码（如"Please enter password?"）。服务分支无需控制台。
    #[cfg(windows)]
    if !matches!(cli.cmd, Cmd::Run) {
        unsafe {
            // 失败静默：终端不支持 65001 时维持现状，好过中断。
            let _ = windows::Win32::System::Console::SetConsoleOutputCP(65001);
            let _ = windows::Win32::System::Console::SetConsoleCP(65001);
        }
    }
    match cli.cmd {
        #[cfg(windows)]
        Cmd::Run => crate::service::service_main(),
        #[cfg(not(windows))]
        Cmd::Run => bail!("run is only supported on Windows"),
        #[cfg(windows)]
        Cmd::Install => crate::service::install(&cli.config, cli.password_stdin),
        #[cfg(not(windows))]
        Cmd::Install => bail!("install is only supported on Windows"),
        #[cfg(windows)]
        Cmd::Uninstall { purge } => crate::service::uninstall(&cli.config, purge),
        #[cfg(not(windows))]
        Cmd::Uninstall { purge: _ } => bail!("uninstall is only supported on Windows"),
        #[cfg(windows)]
        Cmd::Status => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(crate::ipc::client::status_once()),
        #[cfg(not(windows))]
        Cmd::Status => bail!("status is only supported on Windows"),
        #[cfg(windows)]
        Cmd::Tray => crate::tray::run_tray(),
        #[cfg(not(windows))]
        Cmd::Tray => bail!("tray is only supported on Windows"),
    }
}
