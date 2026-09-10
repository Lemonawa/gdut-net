//! silent 模式占位：`--silent` / `--silent --uninstall` 的真实流程在 Task 9。

use anyhow::{bail, Result};

use super::{Mode, SetupArgs};

pub fn run(_args: &SetupArgs, _mode: Mode) -> Result<()> {
    bail!("silent mode lands in a later task")
}
