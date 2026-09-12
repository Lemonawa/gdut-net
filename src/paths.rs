//! 盘上布局的唯一来源（安装态；开发态由 setup 的 payload 回退单独处理）。
//!
//! `--config` 显式覆盖仍以参数为准；这里只定义缺省布局。

use std::path::PathBuf;

/// ProgramData 根。
pub const DATA_DIR: &str = r"C:\ProgramData\gdut-net";
/// 缺省配置路径（CLI `--config` 默认值与服务无参回退共用）。
pub const CONFIG_PATH: &str = r"C:\ProgramData\gdut-net\config.toml";
/// 拨号电话簿路径（config.rs 的 `dial.pbk_path` 缺省）。
pub const PBK_PATH: &str = r"C:\ProgramData\gdut-net\gdut.pbk";
/// 日志目录（服务、托盘、安装器共用）。
pub const LOGS_DIR: &str = r"C:\ProgramData\gdut-net\logs";
/// 安装目录（发布形态；环境变量缺省值）。
pub const INSTALL_DIR: &str = r"C:\Program Files\gdut-net";

/// 安装目录：`%ProgramFiles%\gdut-net`（环境变量缺失时回退缺省值）。
pub fn install_dir() -> PathBuf {
    std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"))
        .join("gdut-net")
}

/// 安装后的主程序。
pub fn install_exe() -> PathBuf {
    install_dir().join("gdut-net.exe")
}

/// 安装后的安装器。
pub fn setup_exe() -> PathBuf {
    install_dir().join("gdut-net-setup.exe")
}
