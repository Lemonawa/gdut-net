//! Bounded command mailbox for UI producers. A stalled transport retains at most
//! one redial and the latest requested mode. No lock is held during IPC work.

use std::sync::{mpsc, Arc, Mutex};

use super::protocol::{Command, NetMode};

#[derive(Default)]
struct Pending {
    redial: bool,
    mode: Option<NetMode>,
}

#[derive(Clone)]
pub(crate) struct CommandSender {
    pending: Arc<Mutex<Pending>>,
    wake: mpsc::SyncSender<()>,
}

pub(crate) struct CommandReceiver {
    pending: Arc<Mutex<Pending>>,
    wake: mpsc::Receiver<()>,
}

pub(crate) fn channel() -> (CommandSender, CommandReceiver) {
    let pending = Arc::new(Mutex::new(Pending::default()));
    let (tx, rx) = mpsc::sync_channel(1);
    (
        CommandSender {
            pending: Arc::clone(&pending),
            wake: tx,
        },
        CommandReceiver { pending, wake: rx },
    )
}

impl CommandSender {
    pub(crate) fn send(&self, command: Command) {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        match command {
            Command::Redial => pending.redial = true,
            Command::SetMode { mode } => pending.mode = Some(mode),
        }
        // Full means a wake is already pending. This never waits for the worker.
        let _ = self.wake.try_send(());
    }
}

impl CommandReceiver {
    pub(crate) fn recv(&self) -> Option<Vec<Command>> {
        self.wake.recv().ok()?;
        let pending = std::mem::take(&mut *self.pending.lock().unwrap_or_else(|e| e.into_inner()));
        let mut commands = Vec::with_capacity(2);
        if let Some(mode) = pending.mode {
            commands.push(Command::SetMode { mode });
        }
        if pending.redial {
            commands.push(Command::Redial);
        }
        Some(commands)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stalled_worker_coalesces_commands_and_preserves_latest_mode() {
        let (tx, rx) = channel();
        for _ in 0..10_000 {
            tx.send(Command::Redial);
            tx.send(Command::SetMode {
                mode: NetMode::WiredExclusive,
            });
        }
        tx.send(Command::SetMode {
            mode: NetMode::WiredPlusStandby,
        });
        assert_eq!(
            rx.recv().unwrap(),
            vec![
                Command::SetMode {
                    mode: NetMode::WiredPlusStandby
                },
                Command::Redial,
            ]
        );
        tx.send(Command::Redial);
        assert_eq!(rx.recv().unwrap(), vec![Command::Redial]);
        drop(tx);
        assert!(rx.recv().is_none());
    }

    #[test]
    fn updates_during_transport_work_wake_the_next_batch() {
        let (tx, rx) = channel();
        tx.send(Command::Redial);
        let _in_flight = rx.recv().unwrap();
        let producer = tx.clone();
        std::thread::spawn(move || {
            producer.send(Command::SetMode {
                mode: NetMode::WiredPlusStandby,
            });
        })
        .join()
        .unwrap();
        assert_eq!(
            rx.recv().unwrap(),
            vec![Command::SetMode {
                mode: NetMode::WiredPlusStandby
            }]
        );
    }
}
