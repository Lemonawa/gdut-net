//! 回家模式守卫（纯逻辑，Linux 可测）。
//!
//! 回家模式（`home.bat`）把服务设成"按需"并停掉；但 Windows 仍可能在下次
//! 登录把托盘重新拉起来——2026-09-26 实测：Run 键已摘干净、启动文件夹与计划
//! 任务里都没有 gdut-net，explorer 仍在登录后 37 秒拉起 `gdut-net.exe tray`
//! （时间点夹在 lghub 与 GameViewer 之间，最像 Win11 的"重启后自动重新打开
//! 应用"）。堵启动源治不了本，让托盘自己认状态：这种状态下不该常驻。
//!
//! 判定放这里，Win32 侧（`service::home_mode_standby`）只负责读 SCM。

/// Windows SCM 启动类型（`winnt.h` 的 `SERVICE_*_START`）。
pub const START_AUTO: u32 = 2;
pub const START_DEMAND: u32 = 3;
pub const START_DISABLED: u32 = 4;

/// 托盘启动时是否应直接退出：服务"按需/禁用"且没在跑 = 回家模式。
///
/// - 服务在跑（哪怕启动类型是手动，例如有人手工 `net start`）→ 常驻；
/// - 自动/引导启动但暂时停止（崩溃、被 kill）→ 常驻，那时灰色的托盘图标
///   正是用户需要的信号。
pub fn tray_should_exit(start_type: u32, running: bool) -> bool {
    !running && matches!(start_type, START_DEMAND | START_DISABLED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_mode_is_manual_or_disabled_and_not_running() {
        assert!(tray_should_exit(START_DEMAND, false));
        assert!(tray_should_exit(START_DISABLED, false));
    }

    #[test]
    fn auto_start_or_running_service_keeps_the_tray() {
        assert!(
            !tray_should_exit(START_AUTO, false),
            "auto + stopped is a fault worth showing"
        );
        assert!(!tray_should_exit(START_AUTO, true));
        assert!(
            !tray_should_exit(START_DEMAND, true),
            "hand-started service keeps the tray"
        );
        assert!(!tray_should_exit(0, false), "boot start");
        assert!(!tray_should_exit(1, false), "system start");
    }
}
