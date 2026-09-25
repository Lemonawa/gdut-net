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
    "clash-campus-dns.ps1",
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
fn payload_scripts_are_ascii_only() {
    for name in EXPECTED
        .iter()
        .filter(|n| n.ends_with(".bat") || n.ends_with(".ps1"))
    {
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

/// 回家模式必须同时摘掉托盘自启（HKCU Run 的 `gdut-net-tray`），否则回家重启后
/// 托盘又自己起来（2026-09-25 用户在家的实测反馈）；回校模式把同一值写回。
/// 值名/键路径与 `tray::register_autostart` 对齐，改这里等于改产品行为。
#[test]
fn home_and_campus_mode_toggle_tray_autostart() {
    const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    let home = fs::read_to_string(payload_dir().join("home.bat")).unwrap();
    let campus = fs::read_to_string(payload_dir().join("campus.bat")).unwrap();
    let delete = format!(r#"reg delete "{RUN_KEY}" /v gdut-net-tray /f"#);
    let add = format!(r#"reg add "{RUN_KEY}" /v gdut-net-tray /t REG_SZ"#);
    assert!(
        home.contains(&delete),
        "home.bat must remove the tray autostart"
    );
    assert!(
        campus.contains(&add),
        "campus.bat must restore the tray autostart"
    );
}

/// 回家/回校模式要同时切换 Clash 的校园 DNS 策略（家里校园 DNS 不可达，policy 串行查询
/// 会白等 5s；2026-09-25 实测）。真正的切换逻辑在 payload 的 clash-campus-dns.ps1。
#[test]
fn home_and_campus_mode_toggle_clash_campus_dns() {
    let home = fs::read_to_string(payload_dir().join("home.bat")).unwrap();
    let campus = fs::read_to_string(payload_dir().join("campus.bat")).unwrap();
    assert!(
        home.contains("clash-campus-dns.ps1\" off"),
        "home.bat must disable the Clash campus-DNS block"
    );
    assert!(
        campus.contains("clash-campus-dns.ps1\" on"),
        "campus.bat must re-enable the Clash campus-DNS block"
    );
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
