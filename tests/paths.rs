use gdut_net::config::LogCfg;
use gdut_net::logging::{LOG_KEEP_FILES, LOG_MAX_SIZE_MB};
use gdut_net::paths;

#[test]
fn layout_constants_are_consistent() {
    assert_eq!(
        format!("{}\\config.toml", paths::DATA_DIR),
        paths::CONFIG_PATH
    );
    assert_eq!(format!("{}\\logs", paths::DATA_DIR), paths::LOGS_DIR);
    assert_eq!(format!("{}\\gdut.pbk", paths::DATA_DIR), paths::PBK_PATH);
    assert!(paths::install_dir().ends_with("gdut-net"));
    assert!(paths::install_exe().ends_with("gdut-net.exe"));
    assert!(paths::setup_exe().ends_with("gdut-net-setup.exe"));
}

#[test]
fn log_policy_has_one_source() {
    let cfg = LogCfg::default();
    assert_eq!(cfg.max_size_mb, LOG_MAX_SIZE_MB);
    assert_eq!(cfg.rotate_keep as usize, LOG_KEEP_FILES);
    assert_eq!(cfg.dir, paths::LOGS_DIR);
}
