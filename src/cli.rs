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

    /// Read password from stdin for install (non-interactive, for scripts)
    #[arg(long, requires = "cmd", global = true)]
    pub password_stdin: bool,

    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Run as Windows service (internal)
    Run,
    /// Install service, create dial entry, write config
    Install,
    /// Uninstall and clean up
    Uninstall {
        /// Also remove config and logs under ProgramData
        #[arg(long)]
        purge: bool,
    },
    /// Show current status
    Status,
    /// Start tray (user session)
    Tray,
}

pub fn dispatch() -> Result<()> {
    // Set UTF-8 code page before clap parses --help (clap prints and exits
    // before we reach later init; default GBK would garble UTF-8 help text).
    #[cfg(windows)]
    unsafe {
        let _ = windows::Win32::System::Console::SetConsoleOutputCP(65001);
        let _ = windows::Win32::System::Console::SetConsoleCP(65001);
    }
    let cli = Cli::parse();
    // Run branch (service dispatcher) does not install CLI logger: real
    // logger is init after loading config via init_service_logging; other
    // subcommands (install/uninstall/status/tray) use info-level stderr.
    if !matches!(cli.cmd, Cmd::Run) {
        crate::logging::init_cli_logging();
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
