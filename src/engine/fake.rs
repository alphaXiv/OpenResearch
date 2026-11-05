use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;

use orx_host_proto::{LogInfo, MAX_PAGE_LEN};

use super::{
    Backend, ByteRange, CancelOutcome, Fence, FileIdent, Handle, LaunchError, LogFile, LogPage,
    Observation, Recovery, RunIntent, WatchedRun,
};
use crate::error::{anyhow, Result};
use crate::store::LaunchSpec;

/// Answers from scripted queues and records every call; an unscripted call panics.
#[derive(Default)]
pub struct FakeBackend {
    pub launches: Mutex<VecDeque<std::result::Result<Handle, LaunchError>>>,
    pub recoveries: Mutex<VecDeque<Recovery>>,
    pub cancels: Mutex<VecDeque<CancelOutcome>>,
    /// Observations the feed reports, in order, for whichever of them are watched.
    pub feed: Mutex<VecDeque<(String, Observation)>>,
    pub log: Mutex<HostFile>,
    pub notify: Mutex<HostFile>,
    /// Every launch intent the backend was asked to launch or recover.
    pub intents: Mutex<Vec<LaunchSpec>>,
    /// Child pid each launch records as its submitter.
    pub submitter: Mutex<Option<u32>>,
    /// Runs inside the next launch, after its fence and before its answer.
    pub during_launch: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Launches never answer, as when the caller's future is dropped mid-launch.
    pub hang_launch: AtomicBool,
    pub calls: Mutex<Vec<String>>,
    pub unwatched: Mutex<Vec<String>>,
    /// Runs whose logs are held for reading, until forgotten.
    pub held: Mutex<Vec<String>>,
}

impl FakeBackend {
    pub fn then_launch(&self, result: std::result::Result<Handle, LaunchError>) -> &Self {
        self.launches.lock().unwrap().push_back(result);
        self
    }

    pub fn then_recover(&self, recovery: Recovery) -> &Self {
        self.recoveries.lock().unwrap().push_back(recovery);
        self
    }

    pub fn then_cancel(&self, outcome: CancelOutcome) -> &Self {
        self.cancels.lock().unwrap().push_back(outcome);
        self
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// Records the call and yields, so concurrent callers interleave as with a real backend.
    async fn call(&self, name: String) {
        self.calls.lock().unwrap().push(name);
        tokio::task::yield_now().await;
    }
}

fn next<T>(queue: &Mutex<VecDeque<T>>, call: &str) -> T {
    queue
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| panic!("unscripted {call}"))
}

#[async_trait]
impl Backend for FakeBackend {
    async fn launch(
        &self,
        intent: &RunIntent,
        fence: &Fence<'_>,
    ) -> std::result::Result<Handle, LaunchError> {
        assert!(!intent.spec_digest.is_empty());
        self.intents.lock().unwrap().push(intent.launch.clone());
        self.call(format!("launch {} #{}", intent.run.id, intent.attempt))
            .await;
        let submitter = *self.submitter.lock().unwrap();
        if let Some(pid) = submitter {
            fence.record_submitter(pid).unwrap();
        }
        if self.hang_launch.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        let during = self.during_launch.lock().unwrap().take();
        if let Some(during) = during {
            during();
        }
        next(&self.launches, "launch")
    }

    async fn recover(&self, intent: &RunIntent) -> Recovery {
        self.intents.lock().unwrap().push(intent.launch.clone());
        self.call(format!("recover {} #{}", intent.run.id, intent.attempt))
            .await;
        next(&self.recoveries, "recover")
    }

    async fn observe(&self, runs: &[WatchedRun], _: Duration) -> Vec<(String, Observation)> {
        let mut feed = self.feed.lock().unwrap();
        let (watched, rest) = feed
            .drain(..)
            .partition(|(id, _)| runs.iter().any(|run| run.id == *id));
        *feed = rest;
        watched.into()
    }

    async fn cancel(&self, run: &WatchedRun) -> CancelOutcome {
        let handle = serde_json::to_value(&run.handle).unwrap();
        self.call(format!(
            "cancel {} {}",
            run.id,
            handle["type"].as_str().unwrap()
        ))
        .await;
        next(&self.cancels, "cancel")
    }

    async fn unwatch(&self, run: &WatchedRun) {
        self.unwatched.lock().unwrap().push(run.id.clone());
    }

    fn forget_logs(&self, forgotten: &dyn Fn(&str) -> bool) {
        self.held.lock().unwrap().retain(|run| !forgotten(run));
    }

    async fn read_log(
        &self,
        _: &WatchedRun,
        file: LogFile,
        range: ByteRange,
        ident: Option<&FileIdent>,
    ) -> Result<LogPage> {
        tokio::task::yield_now().await;
        match file {
            LogFile::Log => self.log.lock().unwrap().read(2, ident, &range),
            LogFile::Notify => self.notify.lock().unwrap().read(3, ident, &range),
        }
    }
}

/// A run file as `orx-host` serves it (`crates/orx-host/src/log.rs`): its identity is its
/// inode, which a replacement bumps, and requests are validated the same way.
#[derive(Default)]
pub struct HostFile {
    pub bytes: Vec<u8>,
    pub ino: u64,
}

impl HostFile {
    pub fn read(&self, dev: u64, ident: Option<&FileIdent>, range: &ByteRange) -> Result<LogPage> {
        if range.end < range.start || range.end - range.start > MAX_PAGE_LEN {
            return Err(anyhow!("invalid log range"));
        }
        let info = LogInfo {
            ident: FileIdent { dev, ino: self.ino },
            len: self.bytes.len() as u64,
        };
        if ident.is_some_and(|ident| *ident != info.ident) || info.len < range.start {
            return Ok(LogPage::Reset { info: Some(info) });
        }
        let end = range.end.min(info.len);
        Ok(LogPage::Data {
            len: Some(info.len),
            ident: info.ident,
            start: range.start,
            bytes: self.bytes[range.start as usize..end as usize].to_vec(),
            eof: end >= info.len,
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Replays the engine's request shapes against the real host and compares page shapes,
    /// with identities renamed by first appearance on each side.
    #[test]
    fn the_fake_validates_and_pages_like_orx_host() {
        let dir = std::env::temp_dir().join(format!("orx-fake-host-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notify");
        let mut fake = HostFile::default();
        let mut idents: [Vec<FileIdent>; 2] = Default::default();
        let mut name = |side: usize, ident: &FileIdent| {
            let seen = &mut idents[side];
            if !seen.contains(ident) {
                seen.push(ident.clone());
            }
            seen.iter().position(|known| known == ident).unwrap()
        };
        let mut shape = |side: usize, page: std::io::Result<LogPage>| match page {
            Err(_) => "error".to_string(),
            Ok(LogPage::Reset { info }) => {
                format!(
                    "reset {:?}",
                    info.map(|info| (name(side, &info.ident), info.len))
                )
            }
            Ok(LogPage::Data {
                len: _,
                ident,
                start,
                bytes,
                eof,
            }) => {
                format!("data {} {start} {bytes:?} {eof}", name(side, &ident))
            }
        };

        let mut last: [Option<FileIdent>; 2] = [None, None];
        let steps: [(&str, u64, u64, bool); 8] = [
            ("hello", 0, 5, false),
            ("hello world", 5, 11, true),
            ("hello world", 11, 11, true),
            ("hello world", 12, 11, true),
            ("hello world", 20, 30, true),
            ("jello world", 11, 11, true),
            ("bye", 0, 3, true),
            ("bye", 0, 3, false),
        ];
        for (content, start, end, with_ident) in steps {
            std::fs::write(&path, content).unwrap();
            fake.bytes = content.as_bytes().to_vec();
            let range = ByteRange { start, end };
            let real =
                orx_host::log::read(&path, last[0].as_ref().filter(|_| with_ident), &range, None);
            let modeled = fake.read(3, last[1].as_ref().filter(|_| with_ident), &range);
            for (side, page) in [
                (0, &real.as_ref().ok().cloned()),
                (1, &modeled.as_ref().ok().cloned()),
            ] {
                if let Some(
                    LogPage::Data { ident, .. }
                    | LogPage::Reset {
                        info: Some(LogInfo { ident, .. }),
                    },
                ) = page
                {
                    last[side] = Some(ident.clone());
                }
            }
            let modeled = modeled.map_err(|error| std::io::Error::other(error.to_string()));
            assert_eq!(
                shape(0, real),
                shape(1, modeled),
                "{content:?} {start}..{end}"
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
