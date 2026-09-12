//! 命名管道 IPC 会话：连接重试、握手（连接即收首帧快照）与命令确认的唯一
//! implementation；CLI `status`、托盘、安装器共用。
//!
//! 服务端可能正在重建管道实例（accept 间隙），`Session::connect` 带重试；
//! 帧协议与服务端一致（JSON-line）。
//!
//! 连接重试的 sleep 走 `tokio::time::sleep`，`connect` 因此可在任意 async
//! 上下文内直接 await 而不阻塞线程——旧实现用 `std::thread::sleep`，在
//! executor 内复用会卡死整个 runtime，该 hazard 已由本 module 消除。
//!
//! 仅 Windows 编译，以 `cargo check --target x86_64-pc-windows-msvc` 验证。

#[cfg(windows)]
mod win {
    use std::collections::VecDeque;
    use std::time::Duration;

    use anyhow::{bail, Context, Result};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::ClientOptions;

    use crate::ipc::protocol::{
        encode_frame, ClientMsg, Command, FrameDecoder, NetMode, ServerMsg, StateSnapshot,
    };
    use crate::ipc::server::PIPE_NAME;

    /// connect 重试参数：覆盖服务端 accept 间隙（每实例一次只能接一个客户端）。
    const CONNECT_RETRIES: u32 = 20;
    const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(250);

    /// 与服务管道的一条连接：重试策略、握手（连接即收首帧快照）与命令确认
    /// 都是本 module 的 implementation。
    pub struct Session {
        pipe: tokio::net::windows::named_pipe::NamedPipeClient,
        decoder: FrameDecoder,
        buf: VecDeque<Vec<u8>>,
    }

    impl Session {
        /// async 连接（须在 tokio 上下文；重试 20×250ms，用 tokio sleep）。
        pub async fn connect() -> Result<Self> {
            let mut last_err = None;
            for _ in 0..CONNECT_RETRIES {
                match ClientOptions::new().open(PIPE_NAME) {
                    Ok(pipe) => {
                        return Ok(Self {
                            pipe,
                            decoder: FrameDecoder::default(),
                            buf: VecDeque::new(),
                        })
                    }
                    Err(e) => last_err = Some(e),
                }
                tokio::time::sleep(CONNECT_RETRY_DELAY).await;
            }
            let io_err =
                last_err.unwrap_or_else(|| std::io::Error::other("No available pipe instance"));
            Err(io_err)
                .with_context(|| format!("Failed to connect to {PIPE_NAME} (service not running?)"))
        }

        /// 读下一帧快照（跳过非法帧）。
        pub async fn next_snapshot(&mut self) -> Result<StateSnapshot> {
            loop {
                // 先吃缓冲里已有的完整帧。
                while let Some(frame) = self.buf.pop_front() {
                    match serde_json::from_slice::<ServerMsg>(&frame) {
                        Ok(ServerMsg::State { state }) => return Ok(state),
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
                    bail!("Server closed the connection");
                }
                self.buf.extend(self.decoder.feed(&chunk[..n]));
            }
        }

        /// 发送命令（不等回显）。
        pub async fn send(&mut self, c: Command) -> Result<()> {
            let frame = encode_frame(&ClientMsg::Cmd { c });
            self.pipe
                .write_all(&frame)
                .await
                .context("Failed to write to pipe")?;
            self.pipe.flush().await.context("Failed to flush pipe")?;
            Ok(())
        }

        /// 发送命令并读回确认快照：读帧直到谓词为真或读满 `max_frames` 帧。
        /// 谓词命中返回该帧；预算读满仍不命中返回最后一帧，由调用方判定
        /// 确认失败（传输错误原样上抛）。
        pub async fn send_and_confirm<F>(
            &mut self,
            cmd: Command,
            confirm: F,
            max_frames: usize,
        ) -> Result<StateSnapshot>
        where
            F: Fn(&StateSnapshot) -> bool,
        {
            self.send(cmd).await?;
            let mut last = None;
            for _ in 0..max_frames {
                let s = self.next_snapshot().await?;
                if confirm(&s) {
                    return Ok(s);
                }
                last = Some(s);
            }
            last.ok_or_else(|| anyhow::anyhow!("No frames read while awaiting confirmation"))
        }
    }

    /// 同步外观（CLI/安装器无线程 runtime）：内部自建 current_thread runtime。
    pub struct SyncSession {
        rt: tokio::runtime::Runtime,
        inner: Session,
    }

    impl SyncSession {
        pub fn connect() -> Result<Self> {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let inner = rt.block_on(Session::connect())?;
            Ok(Self { rt, inner })
        }

        pub fn next_snapshot(&mut self) -> Result<StateSnapshot> {
            self.rt.block_on(self.inner.next_snapshot())
        }

        pub fn send(&mut self, cmd: Command) -> Result<()> {
            self.rt.block_on(self.inner.send(cmd))
        }

        pub fn send_and_confirm<F>(
            &mut self,
            cmd: Command,
            confirm: F,
            max_frames: usize,
        ) -> Result<StateSnapshot>
        where
            F: Fn(&StateSnapshot) -> bool,
        {
            self.rt
                .block_on(self.inner.send_and_confirm(cmd, confirm, max_frames))
        }
    }

    /// `status` 子命令：连接管道 → 读一帧快照（人类可读打印留在 CLI）。
    pub fn status_snapshot() -> Result<StateSnapshot> {
        let mut session = SyncSession::connect()?;
        session.next_snapshot()
    }

    /// `wireless off/standby`：发 SetMode 并等回显（消费首帧 → 发 → ≤5 帧），
    /// 确认失败返回与现状相同的错误文本。
    pub fn set_mode_confirmed(mode: NetMode) -> Result<StateSnapshot> {
        let mut session = SyncSession::connect()?;
        // 服务端连接即推一帧（命令前的 mode）：先消费，否则确认行会读到旧 mode。
        session.next_snapshot()?;
        // 之后的帧可能是主循环周期推送、仍带旧 mode；最多读 5 帧直到请求的
        // mode 出现。
        let snapshot =
            session.send_and_confirm(Command::SetMode { mode }, |s| s.mode == mode, 5)?;
        if snapshot.mode != mode {
            bail!("Service did not confirm mode switch (check `status` output)");
        }
        Ok(snapshot)
    }
}

#[cfg(windows)]
pub use win::{set_mode_confirmed, status_snapshot, Session, SyncSession};
