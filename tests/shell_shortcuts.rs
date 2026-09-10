//! spec §10：开始菜单快捷方式清单与打包 payload 的一致性（纯逻辑，Linux 可跑）。
//!
//! 清单在 `src/shell_shortcuts.rs`（cfg-free），Windows 的 `shell.rs` 消费同一份数据；
//! 此测试钉死条目集合、管理员标志、参数与目标文件是否真实出货。

use std::collections::BTreeSet;
use std::path::Path;

use gdut_net::shell_shortcuts::{EXTRA_TARGETS, SHORTCUTS};

const EXPECTED_NAMES: &[&str] = &[
    "GDUT Net",
    "状态查看",
    "回校模式",
    "回家模式",
    "启动托盘",
    "无线体检",
    "打开日志",
    "代理检查",
    "说明",
    "卸载 GDUT Net",
];

const EXPECTED_ADMIN: &[&str] = &["回校模式", "回家模式", "无线体检", "卸载 GDUT Net"];

/// payload 里唯一刻意使用中文文件名的目标（spec §4 的"说明"项）。
/// ASCII 断言豁免它；豁免集合由下面的测试单独钉死，新增非 ASCII 目标仍会失败。
const NON_ASCII_TARGETS: &[&str] = &["说明.txt"];

fn payload_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/packaging/payload"))
}

#[test]
fn shortcut_inventory_is_exactly_the_expected_set() {
    assert_eq!(SHORTCUTS.len(), 10, "spec §4 defines exactly 10 shortcuts");
    let names: BTreeSet<&str> = SHORTCUTS.iter().map(|s| s.name).collect();
    assert_eq!(
        names.len(),
        SHORTCUTS.len(),
        "shortcut names must be unique"
    );
    let want: BTreeSet<&str> = EXPECTED_NAMES.iter().copied().collect();
    assert_eq!(names, want, "shortcut names diverged from spec §4");
}

#[test]
fn admin_shortcuts_are_exactly_the_elevated_four() {
    let admin: BTreeSet<&str> = SHORTCUTS
        .iter()
        .filter(|s| s.run_as_admin)
        .map(|s| s.name)
        .collect();
    let want: BTreeSet<&str> = EXPECTED_ADMIN.iter().copied().collect();
    assert_eq!(admin, want, "run-as-admin set diverged from spec §4");
}

#[test]
fn uninstall_shortcut_passes_uninstall_flag() {
    let uninstall = SHORTCUTS
        .iter()
        .find(|s| s.name == "卸载 GDUT Net")
        .expect("uninstall shortcut must be present");
    assert_eq!(uninstall.args, "--uninstall");
}

#[test]
fn names_are_nonempty_and_args_ascii() {
    for s in SHORTCUTS {
        assert!(!s.name.trim().is_empty(), "shortcut name must not be empty");
        assert!(
            s.args.is_ascii(),
            "{} args must be ASCII, got {:?}",
            s.name,
            s.args
        );
    }
}

#[test]
fn every_target_is_relative_and_shipped() {
    for s in SHORTCUTS {
        assert!(!s.target.is_empty(), "{} target must not be empty", s.name);
        assert!(
            !s.target.contains('/') && !s.target.contains('\\'),
            "{} target {:?} must be a bare file name",
            s.name,
            s.target
        );
        if EXTRA_TARGETS.contains(&s.target) {
            continue; // packer 注入（gdut-net.exe / gdut-net-setup.exe），不在 payload 目录。
        }
        let path = payload_dir().join(s.target);
        assert!(
            path.is_file(),
            "{} target {:?} missing from packaging/payload/",
            s.name,
            s.target
        );
    }
}

#[test]
fn targets_are_ascii_except_documented_chinese_readme() {
    for s in SHORTCUTS {
        if s.target.is_ascii() {
            continue;
        }
        assert!(
            NON_ASCII_TARGETS.contains(&s.target),
            "{} target {:?} is non-ASCII and not in the documented exemption list",
            s.name,
            s.target
        );
    }
    // 豁免只能是"说明"这一份中文文件——防止豁免列表悄悄扩大。
    let non_ascii: BTreeSet<&str> = SHORTCUTS
        .iter()
        .map(|s| s.target)
        .filter(|t| !t.is_ascii())
        .collect();
    let want: BTreeSet<&str> = NON_ASCII_TARGETS.iter().copied().collect();
    assert_eq!(non_ascii, want, "non-ASCII target exemption drifted");
}
