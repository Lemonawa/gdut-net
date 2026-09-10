//! 开始菜单快捷方式清单（纯逻辑，Linux 可测）：spec §10 要求快捷方式与
//! 打包 payload 保持一致——目标文件必须存在、名字与实际出货对齐。
//! Windows 侧 `shell.rs` 消费此表；`EXTRA_TARGETS` 是 packer 注入的额外文件。

pub struct Shortcut {
    pub name: &'static str,
    pub target: &'static str,
    pub args: &'static str,
    pub run_as_admin: bool,
}

/// 开始菜单 10 项（spec §4）。target 相对安装目录。
pub const SHORTCUTS: &[Shortcut] = &[
    Shortcut {
        name: "GDUT Net",
        target: "gdut-net.exe",
        args: "",
        run_as_admin: false,
    },
    Shortcut {
        name: "状态查看",
        target: "status.bat",
        args: "",
        run_as_admin: false,
    },
    Shortcut {
        name: "回校模式",
        target: "campus.bat",
        args: "",
        run_as_admin: true,
    },
    Shortcut {
        name: "回家模式",
        target: "home.bat",
        args: "",
        run_as_admin: true,
    },
    Shortcut {
        name: "启动托盘",
        target: "gdut-net.exe",
        args: "tray",
        run_as_admin: false,
    },
    Shortcut {
        name: "无线体检",
        target: "wireless-test.bat",
        args: "",
        run_as_admin: true,
    },
    Shortcut {
        name: "打开日志",
        target: "open-logs.bat",
        args: "",
        run_as_admin: false,
    },
    Shortcut {
        name: "代理检查",
        target: "proxy-check.bat",
        args: "",
        run_as_admin: false,
    },
    Shortcut {
        name: "说明",
        target: "说明.txt",
        args: "",
        run_as_admin: false,
    },
    Shortcut {
        name: "卸载 GDUT Net",
        target: "gdut-net-setup.exe",
        args: "--uninstall",
        run_as_admin: true,
    },
];

/// packer 注入安装目录的额外目标（不在 `packaging/payload/` 内）。
pub const EXTRA_TARGETS: &[&str] = &["gdut-net.exe", "gdut-net-setup.exe"];
