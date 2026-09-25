use std::fs;
use std::path::Path;

const EXPECTED: &[&str] = &[
    "status.bat",
    "campus.bat",
    "home.bat",
    "tray.bat",
    "wireless-test.bat",
    "open-logs.bat",
    "proxy-check.bat",
    "说明.txt",
];

fn payload_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/packaging/payload"))
}

#[test]
fn payload_dir_is_exactly_the_expected_set() {
    let mut got: Vec<String> = fs::read_dir(payload_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    got.sort();
    let mut want: Vec<String> = EXPECTED.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(got, want, "payload dir diverged from the shipped list");
}

#[test]
fn bats_are_ascii_only() {
    for name in EXPECTED.iter().filter(|n| n.ends_with(".bat")) {
        let bytes = fs::read(payload_dir().join(name)).unwrap();
        assert!(
            bytes.iter().all(|b| *b < 0x80),
            "{name} contains non-ASCII bytes"
        );
    }
}

#[test]
fn scripts_never_hardcode_desktop_paths_or_plaintext_passwords() {
    for name in EXPECTED {
        let text = fs::read_to_string(payload_dir().join(name)).unwrap();
        assert!(!text.contains("Lemonawa"), "{name} hardcodes a user path");
        assert!(
            !text.contains("pw.txt"),
            "{name} references the plaintext password file"
        );
        assert!(
            !text.contains("--password-stdin"),
            "{name} should not pipe plaintext"
        );
    }
}

#[test]
fn bats_use_script_relative_paths() {
    for name in ["status.bat", "tray.bat", "campus.bat", "wireless-test.bat"] {
        let text = fs::read_to_string(payload_dir().join(name)).unwrap();
        assert!(
            text.contains("%~dp0"),
            "{name} must be location-independent (%~dp0)"
        );
    }
}

/// 提权脚本（campus.bat）拉起托盘必须经 explorer 去提权：直接 start 会把托盘
/// 以管理员身份常驻，违背"托盘 = 普通用户会话进程"设计（CONTEXT 工程陷阱）。
#[test]
fn campus_bat_de_elevates_tray_launch() {
    let text = fs::read_to_string(payload_dir().join("campus.bat")).unwrap();
    assert!(
        text.contains("explorer.exe"),
        "campus.bat must relaunch the tray via explorer (de-elevated)"
    );
    assert!(
        !text.contains("\"%~dp0gdut-net.exe\" tray"),
        "campus.bat must not start the tray directly (it runs elevated)"
    );
}
