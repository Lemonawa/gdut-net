//! setup 命令行参数（纯逻辑，Linux 可测）。GUI 默认；其余模式见 Mode。

use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Gui,
    Repair,
    Uninstall,
    StartService,
    SilentInstall,
    SilentUninstall,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetupArgs {
    pub silent: bool,
    pub uninstall: bool,
    pub repair: bool,
    pub start_service: bool,
    pub keep_password: bool,
    pub purge: bool,
}

impl SetupArgs {
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self> {
        let mut out = Self::default();
        for a in args {
            match a.as_str() {
                "--silent" => out.silent = true,
                "--uninstall" => out.uninstall = true,
                "--repair" => out.repair = true,
                "--start-service" => out.start_service = true,
                "--keep-password" => out.keep_password = true,
                "--purge" => out.purge = true,
                other => bail!("unknown argument {other:?}"),
            }
        }
        // 组合合法性（模式互斥、--keep-password/--purge 前置条件）在 parse 阶段就校验，
        // 保证 entry 拿到的参数已验证；mode() 可重复调用。
        out.mode()?;
        Ok(out)
    }

    pub fn mode(&self) -> Result<Mode> {
        if self.keep_password && !self.silent {
            bail!("--keep-password requires --silent");
        }
        if self.purge && !self.uninstall {
            bail!("--purge requires --uninstall");
        }
        let exclusive = [self.uninstall, self.repair, self.start_service]
            .iter()
            .filter(|b| **b)
            .count();
        if exclusive > 1 {
            bail!("--uninstall/--repair/--start-service are mutually exclusive");
        }
        if self.silent && self.start_service {
            bail!("--silent cannot combine with --start-service");
        }
        Ok(
            match (self.silent, self.uninstall, self.repair, self.start_service) {
                (true, true, _, _) => Mode::SilentUninstall,
                (true, false, _, _) => Mode::SilentInstall,
                (false, true, _, _) => Mode::Uninstall,
                (false, _, true, _) => Mode::Repair,
                (false, _, _, true) => Mode::StartService,
                _ => Mode::Gui,
            },
        )
    }
}
