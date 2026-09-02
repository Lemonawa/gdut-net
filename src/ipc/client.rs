//! 命名管道 IPC 客户端：`status` 子命令与托盘（Task 14）共用。
//!
//! 服务端可能正在重建管道实例（accept 间隙），`connect` 带重试；
//! 帧协议与服务端一致（JSON-line）。
//!
//! 仅 Windows 编译，以 `cargo check --target x86_64-pc-windows-msvc` 验证。

#[cfg(windows)]
mod win {
    use std::collections::VecDeque;
    use std::time::Duration;

    use anyhow::{Context, Result};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::ClientOptions;

    use crate::ipc::protocol::{encode_frame, ClientMsg, Command, ServerMsg, StateSnapshot};
    use crate::ipc::server::PIPE_NAME;

    /// connect 重试参数：覆盖服务端 accept 间隙（每实例一次只能接一个客户端）。
    const CONNECT_RETRIES: u32 = 20;
    const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(250);

    pub struct PipeClient {
        pipe: tokio::net::windows::named_pipe::NamedPipeClient,
        decoder: crate::ipc::protocol::FrameDecoder,
        buf: VecDeque<Vec<u8>>,
    }

    impl PipeClient {
        /// 连接服务管道；服务端建管间隙内重试若干次。
        pub fn connect() -> Result<Self> {
            let mut last_err = None;
            for _ in 0..CONNECT_RETRIES {
                match ClientOptions::new().open(PIPE_NAME) {
                    Ok(pipe) => {
                        return Ok(Self {
                            pipe,
                            decoder: crate::ipc::protocol::FrameDecoder::default(),
                            buf: VecDeque::new(),
                        })
                    }
                    Err(e) => last_err = Some(e),
                }
                // 仅 CLI 同步上下文使用；executor 内复用须换 tokio sleep（Task 14）。
                std::thread::sleep(CONNECT_RETRY_DELAY);
            }
            let io_err =
                last_err.unwrap_or_else(|| std::io::Error::other("No available pipe instance"));
            Err(io_err)
                .with_context(|| format!("Failed to connect to {PIPE_NAME} (service not running?)"))
        }

        /// 读一帧 `ServerMsg::State` 并返回快照。Ack 或非法帧跳过继续读。
        pub async fn next_state(&mut self) -> Result<StateSnapshot> {
            loop {
                // 先吃缓冲里已有的完整帧。
                while let Some(frame) = self.buf.pop_front() {
                    match serde_json::from_slice::<ServerMsg>(&frame) {
                        Ok(ServerMsg::State { state }) => return Ok(state),
                        Ok(ServerMsg::Ack) => continue,
                        Err(e) => log::debug!("Ignoring invalid server frame: {e}"),
                    }
                }
                let mut chunk = [0u8; 4096];
                let n = self
                    .pipe
                    .read(&mut chunk)
                    .await
                    .context("Failed to read pipe (service exited?)")?;
                if n == 0 {
                    anyhow::bail!("Server closed the connection");
                }
                self.buf.extend(self.decoder.feed(&chunk[..n]));
            }
        }

        /// 发送一条命令给服务端。
        pub async fn send_cmd(&mut self, c: Command) -> Result<()> {
            let frame = encode_frame(&ClientMsg::Cmd { c });
            self.pipe
                .write_all(&frame)
                .await
                .context("Failed to write to pipe")?;
            self.pipe.flush().await.context("Failed to flush pipe")?;
            Ok(())
        }
    }

    /// `status` 子命令：连接管道 → 读一帧快照 → 人类可读打印。
    pub async fn status_once() -> Result<()> {
        let mut client = PipeClient::connect()?;
        let s = client.next_state().await?;

        println!("Status:     {}", s.status_text());
        println!("Uptime:   {}", s.uptime_text());
        println!("IP:       {}", s.ip.as_deref().unwrap_or("—"));
        println!(
            "Drop reason: {}",
            s.last_drop_reason.as_deref().unwrap_or("—")
        );
        println!("Redial attempts: {}", s.redial_attempts);
        println!("Heartbeat: {}", s.heartbeat_text());
        Ok(())
    }
}

#[cfg(windows)]
pub use win::{status_once, PipeClient};
