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
                                        log::debug!("忽略非法客户端消息: {e}");
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

    /// 创建一个管道实例并等待首个客户端连接。
    async fn listen_once() -> Result<NamedPipeServer> {
        let server = ServerOptions::new()
            .first_pipe_instance(false)
            .create(PIPE_NAME)?;
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
                            log::warn!("IPC 管道创建/连接失败，1s 后重试: {e}");
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
            log::info!("IPC server 退出");
        });
    }
}

#[cfg(windows)]
pub use win::{spawn_server, PIPE_NAME};
