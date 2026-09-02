//! 日志初始化：服务路径（文件滚动 + 可选事件日志）与 CLI 辅助路径（stderr）。
//!
//! 两条互斥路径（log 全局 logger 每进程只能 set 一次）：
//! - 服务（run → runtime::start_all）：[`init_service_logging`] 按 LogCfg 初始化——
//!   info 级、文件输出（`log.dir`）、按大小滚动（`log.max_size_mb`，保留
//!   `log.rotate_keep` 份历史）、`log.event_log=true` 时叠加自定义 `log::Log`
//!   包装把 warn/error 转发到 Windows 事件日志（Task 11 EventLog）。
//! - CLI 辅助命令（install/uninstall/status/tray）：[`init_cli_logging`]
//!   info 级直接 stderr。
//!
//! 组合 logger 方案：`Logger::build()` 拿到 flexi_logger 的 `Box<dyn log::Log>`
//! 后手动安装（flexi_logger 的 additional_writers 需要特殊 target 语法，
//! 无法按级别转发全部模块，故不采用）。窗口期（config 加载前）的启动错误
//! 由 service_run 的 stderr 兜底输出。
//!
//! 服务路径消费 EventLog（仅 Windows），以
//! `cargo check --target x86_64-pc-windows-msvc` 验证。

use anyhow::{Context, Result};
use flexi_logger::{Cleanup, Criterion, FileSpec, Logger, LoggerHandle, Naming};

use crate::config::LogCfg;

/// CLI 辅助命令：info 级直接 stderr（flexi_logger 默认彩色格式）。
///
/// flexi_logger 文档：`log_to_stderr` 场景下立即丢弃 LoggerHandle 是安全的
/// （无后台 flush 资源）。初始化失败不阻断命令主流程。
pub fn init_cli_logging() {
    let _ = Logger::try_with_str("info").map(|logger| logger.log_to_stderr().start());
}

/// 服务路径：按 LogCfg 初始化文件滚动日志（+ 可选事件日志镜像）。
///
/// 返回的 [`LoggerHandle`] 必须持有到进程退出——Drop 会 flush 并关闭
/// FileLogWriter，提前丢弃等于掐断后续所有文件日志。
pub fn init_service_logging(cfg: &LogCfg) -> Result<LoggerHandle> {
    std::fs::create_dir_all(&cfg.dir)
        .with_context(|| format!("Failed to create log directory: {}", cfg.dir))?;

    let (inner, handle) = Logger::try_with_str("info")
        .context("Failed to parse log spec")?
        .log_to_file(FileSpec::default().directory(&cfg.dir))
        .format(flexi_logger::detailed_format)
        // 服务重启后续写 rCURRENT，而非截断丢历史（Size 滚动与 append 兼容）。
        .append()
        .rotate(
            Criterion::Size(rotation_bytes(cfg.max_size_mb)),
            Naming::Timestamps,
            Cleanup::KeepLogFiles(cfg.rotate_keep as usize),
        )
        .build()
        .context("Failed to build logger (log dir not writable?)")?;

    install_logger(inner, cfg.event_log)?;
    Ok(handle)
}

/// 滚动阈值 MB → 字节；下限钳到 1MiB（0/极小值会导致逐行滚动）。
fn rotation_bytes(max_size_mb: u64) -> u64 {
    max_size_mb.saturating_mul(1024 * 1024).max(1024 * 1024)
}

#[cfg(windows)]
fn install_logger(inner: Box<dyn log::Log>, event_log: bool) -> Result<()> {
    let sink = if event_log {
        // 打不开事件源（未 install 注册？）降级为纯文件日志：服务可用性优先，
        // 且此刻全局 logger 尚未安装，只能 stderr 提示。
        match crate::eventlog::EventLog::open() {
            Ok(log) => Some(EventLogSink(log)),
            Err(e) => {
                eprintln!("Failed to open event log, falling back to file only: {e:#}");
                None
            }
        }
    } else {
        None
    };

    log::set_boxed_logger(Box::new(ServiceLogger { inner, sink }))
        .context("Failed to register global logger (already initialized?)")?;
    log::set_max_level(log::LevelFilter::Info);
    Ok(())
}

#[cfg(not(windows))]
fn install_logger(inner: Box<dyn log::Log>, _event_log: bool) -> Result<()> {
    // 非 Windows 无事件日志，仅文件输出（本分支仅为编译/测试存在）。
    log::set_boxed_logger(inner)
        .context("Failed to register global logger (already initialized?)")?;
    log::set_max_level(log::LevelFilter::Info);
    Ok(())
}

/// 事件日志 sink：windows 0.62 的 `HANDLE` 是裸指针（!Send/!Sync），而
/// `log::Log` 要求 Send + Sync。
///
/// Safety：handle 仅按值传给 ReportEventW（Copy 读取，无内部可变性），
/// 事件源句柄由 Win32 保证可多线程并发上报；DeregisterEventSource 只在
/// Drop 发生（进程退出前）。与 runtime.rs 的 SendSession 同一先例。
#[cfg(windows)]
struct EventLogSink(crate::eventlog::EventLog);

#[cfg(windows)]
unsafe impl Send for EventLogSink {}

#[cfg(windows)]
unsafe impl Sync for EventLogSink {}

/// 组合 logger：文件输出全权交给 flexi_logger（含其规格过滤），
/// warn/error 叠加镜像到事件日志。
#[cfg(windows)]
struct ServiceLogger {
    inner: Box<dyn log::Log>,
    sink: Option<EventLogSink>,
}

#[cfg(windows)]
impl log::Log for ServiceLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // 宽松放行（inner.log 内部还会按规格自滤）；debug/trace 由
        // set_max_level 在宏处拦截。
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        self.inner.log(record);
        if record.level() > log::Level::Warn {
            return;
        }
        let Some(sink) = &self.sink else {
            return;
        };
        let level = match record.level() {
            log::Level::Error => crate::eventlog::EventLevel::Error,
            _ => crate::eventlog::EventLevel::Warning,
        };
        let msg = format!(
            "[{}] {}",
            record.module_path().unwrap_or("gdut-net"),
            record.args()
        );
        // 事件日志写失败不得 log::warn!（递归），回落 stderr。
        if let Err(e) = sink.0.report(level, &msg) {
            eprintln!("Failed to write event log: {e:#}");
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::rotation_bytes;

    #[test]
    fn rotation_bytes_clamps() {
        const MIB: u64 = 1024 * 1024;
        assert_eq!(rotation_bytes(5), 5 * MIB);
        // 0/极小值钳到 1MiB
        assert_eq!(rotation_bytes(0), MIB);
        assert_eq!(rotation_bytes(1), MIB);
        // 溢出饱和，不回绕
        assert_eq!(rotation_bytes(u64::MAX), u64::MAX);
    }
}
