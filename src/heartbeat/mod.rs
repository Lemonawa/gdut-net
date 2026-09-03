//! 心跳兼容模式模块（默认关闭，由配置选择）。
//!
//! `spec` 子模块为纯报文规格（无 IO）；`session` 为实际 UDP 会话循环。

pub mod session;
pub mod spec;
