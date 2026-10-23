#[path = "../src/setup/file_backup.rs"]
mod file_backup;

use file_backup::FileBackup;

#[test]
fn interrupted_upgrade_restores_old_files_and_removes_new_ones() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("gdut-net.exe");
    let config = dir.path().join("config.toml");
    let new_file = dir.path().join("new.bat");
    std::fs::write(&exe, b"old executable").unwrap();
    std::fs::write(&config, b"old encrypted config").unwrap();
    let backup = FileBackup::capture([exe.clone(), config.clone(), new_file.clone()]).unwrap();
    std::fs::write(&exe, b"partial new executable").unwrap();
    std::fs::write(&config, b"new config").unwrap();
    std::fs::write(&new_file, b"new script").unwrap();
    backup.restore().unwrap();
    assert_eq!(std::fs::read(exe).unwrap(), b"old executable");
    assert_eq!(std::fs::read(config).unwrap(), b"old encrypted config");
    assert!(!new_file.exists());
}

#[test]
fn restoration_attempts_other_files_even_when_one_path_cannot_be_restored() {
    let dir = tempfile::tempdir().unwrap();
    let recoverable = dir.path().join("config.toml");
    let blocked = dir.path().join("gdut-net.exe");
    std::fs::write(&recoverable, b"old config").unwrap();
    std::fs::write(&blocked, b"old exe").unwrap();
    let backup = FileBackup::capture([recoverable.clone(), blocked.clone()]).unwrap();
    std::fs::write(&recoverable, b"new config").unwrap();
    std::fs::remove_file(&blocked).unwrap();
    std::fs::create_dir(&blocked).unwrap();
    assert!(backup.restore().is_err());
    assert_eq!(std::fs::read(recoverable).unwrap(), b"old config");
}

#[test]
fn unreadable_backup_aborts_before_any_change() {
    let dir = tempfile::tempdir().unwrap();
    assert!(FileBackup::capture([dir.path().to_path_buf()]).is_err());
}

#[test]
fn replacement_publishes_complete_files_and_leaves_no_staging_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gdut-net.exe");
    file_backup::replace_file(&path, b"first complete version").unwrap();
    file_backup::replace_file(&path, b"second complete version").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"second complete version");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}
