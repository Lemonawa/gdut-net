#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        gdut_net::setup::entry()
    }
    #[cfg(not(windows))]
    {
        anyhow::bail!("gdut-net-setup is Windows-only")
    }
}
