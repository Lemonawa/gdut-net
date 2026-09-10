#[cfg(windows)]
use crate::ipc::protocol::{Command, NetMode};
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
    Install {
        /// Reuse the stored DPAPI password (no prompt, no plaintext)
        #[arg(long)]
        keep_password: bool,
    },
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
    /// Wireless (campus WiFi) utilities
    Wireless {
        #[command(subcommand)]
        action: WirelessAction,
    },
}

#[derive(Subcommand)]
pub enum WirelessAction {
    /// Join campus SSID once, try one portal login, print reply (field check)
    Test,
    /// Switch service to wired-exclusive mode now
    Off,
    /// Switch service to wired+wireless standby mode now
    Standby,
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
        Cmd::Install { keep_password } => {
            crate::service::install(&cli.config, cli.password_stdin, keep_password)
        }
        #[cfg(not(windows))]
        Cmd::Install { keep_password: _ } => bail!("install is only supported on Windows"),
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
        #[cfg(windows)]
        Cmd::Wireless {
            action: WirelessAction::Test,
        } => crate::wireless::test::cli_test(&cli.config),
        #[cfg(not(windows))]
        Cmd::Wireless {
            action: WirelessAction::Test,
        } => bail!("wireless is only supported on Windows"),
        #[cfg(windows)]
        Cmd::Wireless {
            action: WirelessAction::Off,
        } => wireless_set_mode(NetMode::WiredExclusive),
        #[cfg(windows)]
        Cmd::Wireless {
            action: WirelessAction::Standby,
        } => wireless_set_mode(NetMode::WiredPlusStandby),
        #[cfg(not(windows))]
        Cmd::Wireless { action: _ } => bail!("wireless is only supported on Windows"),
    }
}

/// Build a current-thread runtime, connect to the service pipe, send one
/// SetMode command, then read state snapshots until the service confirms
/// the new mode.
#[cfg(windows)]
fn wireless_set_mode(mode: NetMode) -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let mut client = crate::ipc::client::PipeClient::connect()?;
            // The server pushes a snapshot immediately on connect (pre-command
            // mode): consume it first or the confirmation would print the old
            // mode.
            client.next_state().await?;
            client.send_cmd(Command::SetMode { mode }).await?;
            // Wait for the post-command broadcast; intermediate frames may be
            // periodic main-loop pushes still carrying the old mode, so read
            // up to 5 snapshots until the requested one shows up.
            for _ in 0..5 {
                let s = client.next_state().await?;
                if s.mode == mode {
                    println!("Mode set: {}", s.mode_text());
                    return Ok(());
                }
            }
            anyhow::bail!("Service did not confirm mode switch (check `status` output)");
        })
}
