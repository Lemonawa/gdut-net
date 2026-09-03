//! 命名管道 IPC server：多客户端广播状态快照，转发客户端命令。
//!
//! 协议见 [`crate::ipc::protocol`]（JSON-line：`encode_frame` 编码带尾部
//! `\n`，`FrameDecoder` 切帧；喂给 serde_json 时对尾随空白宽容，直接解析）。
//!
//! 每个连接一个 task：连接即推当前快照，之后 snapshot 变更即推
//! （`watch::changed`）；读侧收 `ClientMsg::Cmd` 转发 `cmd_tx` 给 runtime
//! 主循环。server 生命周期随 `stop` 退出。
//!
//! 仅 Windows 编译，以 `cargo check --target x86_64-pc-windows-msvc` 验证。

#[cfg(windows)]
mod win {
    use anyhow::Result;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
    use tokio::sync::{mpsc, watch};
    use tokio_util::sync::CancellationToken;

    use windows::core::PCWSTR;
    use windows::Win32::Security::SECURITY_ATTRIBUTES;

    use crate::ipc::protocol::{
        encode_frame, ClientMsg, Command, FrameDecoder, ServerMsg, StateSnapshot,
    };

    /// 服务端管道名。
    pub const PIPE_NAME: &str = r"\\.\pipe\gdut-net";

    /// 单个客户端连接的读写循环。连接即推当前快照，随后变更即推；
    /// 收到命令转发 `cmd_tx`。客户端断开或出错即结束。
    async fn serve_client(
        mut pipe: NamedPipeServer,
        mut snapshot_rx: watch::Receiver<StateSnapshot>,
        cmd_tx: mpsc::Sender<Command>,
    ) {
        // 连接即推当前快照（客户端 connect 后第一个 next_state 立即返回）。
        {
            let snap = snapshot_rx.borrow().clone();
            let frame = encode_frame(&ServerMsg::State { state: snap });
            if pipe.write_all(&frame).await.is_err() {
                return;
            }
        }

        let mut decoder = FrameDecoder::default();
        let mut buf = [0u8; 4096];
        loop {
            tokio::select! {
                changed = snapshot_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let snap = snapshot_rx.borrow().clone();
                    let frame = encode_frame(&ServerMsg::State { state: snap });
                    if pipe.write_all(&frame).await.is_err() {
                        break;
                    }
                }
                read = pipe.read(&mut buf) => {
                    // 客户端只在发命令时写数据；0 字节 = 对端断开。
                    match read {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            for frame in decoder.feed(&buf[..n]) {
                                match serde_json::from_slice::<ClientMsg>(&frame) {
                                    Ok(ClientMsg::Cmd { c }) => {
                                        let _ = cmd_tx.send(c).await;
                                    }
                                    Err(e) => {
                                        log::debug!("Ignoring invalid client message: {e}");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        let _ = pipe.flush().await;
    }

    /// 建管道用的 SECURITY_ATTRIBUTES：SDDL 给 Authenticated Users 开
    /// GRGW。服务跑在 SYSTEM（Session 0），托盘/`status` 跑在用户会话；
    /// 默认 DACL 会让普通用户 open 直接报 os error 5（拒绝访问）。
    /// 返回的 SECURITY_ATTRIBUTES 只借用其内部的 SD 缓冲，调用方须在
    /// create 返回后用 LocalFree 释放。
    fn pipe_security() -> Result<(SECURITY_ATTRIBUTES, *mut std::ffi::c_void)> {
        use windows::Win32::Security::Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
        };
        use windows::Win32::Security::PSECURITY_DESCRIPTOR;

        const SDDL: &str = "D:(A;;GRGW;;;AU)";
        let wide: Vec<u16> = SDDL.encode_utf16().chain(std::iter::once(0)).collect();
        let mut sd = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
        // SAFETY: wide 以 NUL 结尾且在调用期间存活；sd 接收新分配的缓冲。
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(wide.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
        }
        .map_err(|e| anyhow::anyhow!("ConvertStringSecurityDescriptor failed: {e}"))?;
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: false.into(),
        };
        Ok((sa, sd.0))
    }

    /// 创建一个管道实例并等待首个客户端连接。建管（含 SD 分配释放）收敛在
    /// 一个同步块内完成，不让裸指针活过 await（否则 spawn 的 future 非 Send）。
    async fn listen_once() -> Result<NamedPipeServer> {
        use windows::Win32::Foundation::LocalFree;
        use windows::Win32::Foundation::HLOCAL;

        let server = {
            let (mut sa, sd) = pipe_security()?;
            let mut opts = ServerOptions::new();
            opts.first_pipe_instance(false);
            // SAFETY: sa 在此次调用期间存活；其余参数与旧 create 路径一致。
            let r = unsafe {
                opts.create_with_security_attributes_raw(
                    PIPE_NAME,
                    &mut sa as *mut _ as *mut std::ffi::c_void,
                )
            };
            // SAFETY: sd 来自 ConvertStringSecurityDescriptorToSecurityDescriptorW，
            // create 已返回，不再使用。
            unsafe {
                let _ = LocalFree(Some(HLOCAL(sd)));
            }
            r?
        };
        server.connect().await?;
        Ok(server)
    }

    /// IPC server 主体：循环 accept 多客户端，每连接独立 task。
    /// `stop` 取消后退出（进行中的连接随 snapshot_rx 关闭自然终止）。
    pub fn spawn_server(
        snapshot_rx: watch::Receiver<StateSnapshot>,
        cmd_tx: mpsc::Sender<Command>,
        stop: CancellationToken,
    ) {
        tokio::spawn(async move {
            loop {
                let pipe = tokio::select! {
                    _ = stop.cancelled() => break,
                    p = listen_once() => match p {
                        Ok(pipe) => pipe,
                        Err(e) => {
                            log::warn!("IPC pipe create/connect failed, retry in 1s: {e}");
                            tokio::select! {
                                _ = stop.cancelled() => break,
                                _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
                            }
                            continue;
                        }
                    }
                };
                let snapshot_rx = snapshot_rx.clone();
                tokio::spawn(serve_client(pipe, snapshot_rx, cmd_tx.clone()));
            }
            log::info!("IPC server exiting");
        });
    }
}

#[cfg(windows)]
pub use win::{spawn_server, PIPE_NAME};
