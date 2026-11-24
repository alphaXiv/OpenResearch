use crate::signals::{load, scan, validate_run};
use orx_signals::Snapshot;
use std::{collections::VecDeque, io, path::Path, sync::Mutex};

struct Pinned {
    run: String,
    summary: orx_host_proto::SignalsSummary,
    bytes: Vec<u8>,
}

#[derive(Default)]
pub(crate) struct Pages(Mutex<VecDeque<Pinned>>);

fn summary(snapshot: &Snapshot) -> orx_host_proto::SignalsSummary {
    orx_host_proto::SignalsSummary {
        generation: snapshot.generation.clone(),
        revision: snapshot.rev,
        error: snapshot
            .coverage
            .reasons
            .iter()
            .any(|r| r == "log_missing")
            .then(|| {
                format!(
                    "Run {}: raw log is missing; its last checkpoint is retained",
                    snapshot.run_id
                )
            }),
    }
}

pub(crate) fn inspect(
    root: &Path,
    run: &str,
    terminal: bool,
    now: i64,
) -> io::Result<orx_host_proto::SignalsSummary> {
    validate_run(run)?;
    let dir = root.join("runs").join(run);
    scan(
        &dir.join("log"),
        &root.join("run-signals").join(format!("{run}.json")),
        run,
        terminal,
        now,
        crate::wrapper::started_at(&dir),
    )
    .map(|s| {
        s.as_ref().map(summary).unwrap_or_else(|| {
            unavailable(format!("No log or signal checkpoint exists for run {run}"))
        })
    })
}

pub(crate) fn unavailable(error: String) -> orx_host_proto::SignalsSummary {
    orx_host_proto::SignalsSummary {
        generation: String::new(),
        revision: 0,
        error: Some(error),
    }
}

impl Pages {
    pub(crate) fn read(
        &self,
        root: &Path,
        run: &str,
        version: (&str, u64),
        offset: u64,
        length: u64,
        answered_at: i64,
    ) -> io::Result<orx_host_proto::SignalsPage> {
        let (generation, revision) = version;
        validate_run(run)?;
        if length == 0 || length > 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "signal page exceeds 1 MiB",
            ));
        }
        let pinned = {
            let mut pages = self
                .0
                .lock()
                .map_err(|_| io::Error::other("Signal page cache is poisoned"))?;
            pages
                .iter()
                .position(|p| {
                    p.run == run
                        && p.summary.generation == generation
                        && p.summary.revision == revision
                })
                .map(|index| pages.remove(index).unwrap())
        };
        let pinned = if let Some(pinned) = pinned {
            pinned
        } else {
            let path = root.join("run-signals").join(format!("{run}.json"));
            let snapshot = load(&path)?.ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "signals not yet available")
            })?;
            Pinned {
                run: run.to_owned(),
                summary: summary(&snapshot),
                bytes: serde_json::to_vec(&snapshot)?,
            }
        };
        let bytes = &pinned.bytes;
        let page = if pinned.summary.generation != generation || pinned.summary.revision != revision
        {
            orx_host_proto::SignalsPage::Reset {
                summary: pinned.summary.clone(),
                answered_at,
            }
        } else {
            let start = usize::try_from(offset).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "signal offset too large")
            })?;
            if start > bytes.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "signal offset past snapshot",
                ));
            }
            let end = (start + length as usize).min(bytes.len());
            orx_host_proto::SignalsPage::Data {
                generation: pinned.summary.generation.clone(),
                revision: pinned.summary.revision,
                offset,
                total: bytes.len() as u64,
                bytes: bytes[start..end].to_vec(),
            }
        };
        let mut pages = self
            .0
            .lock()
            .map_err(|_| io::Error::other("Signal page cache is poisoned"))?;
        pages.retain(|p| p.run != run);
        while pages.len() >= 64
            || pages.iter().map(|p| p.bytes.len()).sum::<usize>() + bytes.len() > 64 * 1024 * 1024
        {
            pages.pop_front();
        }
        pages.push_back(pinned);
        Ok(page)
    }
}
