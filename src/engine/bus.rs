use tokio::sync::broadcast;

use super::Phase;
use crate::store::Contact;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEvent {
    LogCollection {
        run_id: String,
    },
    Host {
        alias: String,
        root: String,
        state: super::transport::HostState,
    },
    Changed {
        run_id: String,
        phase: Phase,
        contact: Contact,
        cancelling: bool,
    },
    /// New undelivered notices for these chats.
    Notices {
        chats: Vec<String>,
    },
    Grew {
        run_id: String,
        file: super::LogFile,
        len: u64,
    },
}

/// Events published after their transaction commits. A lagged subscriber resyncs from the store.
pub struct Bus(broadcast::Sender<RunEvent>);

impl Bus {
    pub fn new() -> Self {
        Self(broadcast::channel(1024).0)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RunEvent> {
        self.0.subscribe()
    }

    pub fn publish(&self, event: RunEvent) {
        // No subscribers is fine: every event is already durable in the store.
        let _ = self.0.send(event);
    }
}
