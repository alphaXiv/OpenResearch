use super::local::signals::{
    configured, load, note_replaced, publish as publish_snapshot, validate_run,
};
use crate::engine::{Backend, ByteRange, FileIdent, LogFile, LogPage, WatchedRun};
use crate::error::{anyhow, bail, Result};
use orx_signals::Snapshot;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

fn collection_lock(path: &Path) -> Result<Arc<tokio::sync::Mutex<()>>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| anyhow!("Signal collection lock registry is poisoned"))?;
    locks.retain(|_, lock| lock.strong_count() > 0);
    let entry = locks.entry(path.to_path_buf()).or_default();
    if let Some(lock) = entry.upgrade() {
        return Ok(lock);
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    *entry = Arc::downgrade(&lock);
    Ok(lock)
}

/// The laptop's one place to publish a run's checkpoint, and whether `snapshot` is now the
/// published one. A checkpoint is never replaced by one that knows less: another generation
/// that covers fewer bytes is a rebuild, kept as a staging checkpoint beside the published one
/// until it covers them. Meanwhile the published one stands, marked rebuilding; if the rebuild
/// ends first, it stands marked as not read to the end and the staging one goes. Identity is
/// not compared: a spool refetched with other bytes keeps it.
pub(crate) fn publish(path: &Path, snapshot: &Snapshot) -> std::io::Result<bool> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut lock = fd_lock::RwLock::new(options.open(path.with_extension("lock"))?);
    let _guard = lock.try_write()?;
    let staging = path.with_extension("replay");
    let held = load(path)?;
    let rebuilt = held
        .as_ref()
        .is_some_and(|held| held.generation != snapshot.generation);
    let Some(mut held) = held.filter(|held| knows_less(snapshot, held)) else {
        publish_snapshot(path, snapshot)?;
        crate::engine::monitor::hand_over(path, snapshot);
        // Also what a crash after an earlier promotion left behind.
        if rebuilt || !rebuilding(snapshot) {
            remove_staging(&staging)?;
        }
        return Ok(true);
    };
    let before = held.clone();
    if finished(snapshot) {
        held.remove_reason("rebuilding");
        set_aside(&mut held, true);
    } else {
        publish_snapshot(&staging, snapshot)?;
        held.coverage.add_reason("rebuilding");
    }
    if held != before {
        publish_snapshot(path, &held)?;
        crate::engine::monitor::hand_over(path, &held);
    }
    if finished(snapshot) {
        remove_staging(&staging)?;
    }
    Ok(false)
}

fn remove_staging(staging: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(staging) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Marks a checkpoint whose log was not read to the end; `last` when nothing will read further.
/// Whether that changed it.
fn set_aside(held: &mut Snapshot, last: bool) -> bool {
    let before = held.clone();
    held.coverage.add_reason("log_replaced");
    if last && !finished(held) {
        held.extra.insert("terminal".into(), true.into());
        held.rev += 1;
    }
    *held != before
}

fn rebuilding(snapshot: &Snapshot) -> bool {
    snapshot.coverage.reasons.iter().any(|r| r == "rebuilding")
}

fn finished(snapshot: &Snapshot) -> bool {
    snapshot.extra.get("terminal") == Some(&serde_json::Value::Bool(true))
}

fn knows_less(new: &Snapshot, held: &Snapshot) -> bool {
    let covered = |&(from, to): &(u64, u64)| {
        new.coverage
            .ranges
            .iter()
            .any(|&(start, end)| start <= from && to <= end)
    };
    new.generation != held.generation
        && held.has_signals()
        && !held.coverage.ranges.iter().all(covered)
}

/// Older serve and adopted supervisors expose only the frozen byte-page log API.
pub(crate) async fn collect(
    data: &Path,
    backend: &dyn Backend,
    run: &WatchedRun,
    terminal: bool,
    now: i64,
) -> Result<bool> {
    collect_from(data, Some(backend), run, terminal, now, None).await
}

/// One pass over the kept log: it reads until `deadline`, at least one page, and publishes once.
pub(crate) async fn collect_kept(
    data: &Path,
    run: &WatchedRun,
    now: i64,
    as_of: i64,
    deadline: tokio::time::Instant,
) -> Result<bool> {
    collect_from(data, None, run, true, now, Some((as_of, deadline))).await
}

async fn page(
    data: &Path,
    backend: Option<&dyn Backend>,
    run: &WatchedRun,
    range: ByteRange,
    ident: Option<&FileIdent>,
) -> Result<LogPage> {
    if let Some(backend) = backend {
        return backend.read_log(run, LogFile::Log, range, ident).await;
    }
    let kept = crate::commands::up::api_v2::kept_log(data, &run.id)?;
    let Some(last) = kept.last() else {
        return Ok(LogPage::Reset { info: None });
    };
    // The host's identity, so a checkpoint the host folded resumes over the bytes kept from it.
    let actual = last.ident.clone();
    let len = last.offset + last.len;
    if range.start > len || ident.is_some_and(|e| *e != actual) {
        return Ok(LogPage::Reset {
            info: Some(orx_host_proto::LogInfo { ident: actual, len }),
        });
    }
    // A fold reads on from the next kept byte; the page's start says what it skipped.
    let start = kept
        .iter()
        .find(|r| range.start < r.offset + r.len)
        .map_or(len, |r| r.offset.max(range.start));
    let skipped = ByteRange {
        start,
        end: range.end.max(start),
    };
    let bytes = match crate::commands::up::api_v2::kept_page(&kept, &skipped, Some(&actual))? {
        Some(LogPage::Data { bytes, .. }) => bytes,
        Some(LogPage::Reset { .. }) => {
            bail!("Run {}'s saved log has more than one identity", run.id)
        }
        None => Vec::new(),
    };
    Ok(LogPage::Data {
        ident: actual,
        start,
        len: Some(len),
        eof: start + bytes.len() as u64 == len,
        bytes,
    })
}

/// Whether `old` is a fold of the log at `ident` that reading on from its cursor continues.
async fn continues(
    data: &Path,
    backend: Option<&dyn Backend>,
    run: &WatchedRun,
    old: &Snapshot,
    ident: &FileIdent,
    len: Option<u64>,
) -> Result<bool> {
    let same = old.cursor.ident == String::from(ident.clone())
        && len.is_none_or(|len| old.cursor.offset <= len)
        && (!finished(old) || len == Some(old.cursor.offset));
    let Some(anchor) = old
        .extra
        .get("anchorBase64")
        .and_then(serde_json::Value::as_str)
        .filter(|_| same)
    else {
        return Ok(same);
    };
    use base64::Engine;
    let expected = base64::engine::general_purpose::STANDARD.decode(anchor)?;
    let start = old.cursor.offset.saturating_sub(expected.len() as u64);
    let kept_anchor = backend.is_some()
        || crate::commands::up::api_v2::kept_log(data, &run.id)?
            .iter()
            .any(|r| r.offset <= start && old.cursor.offset <= r.offset + r.len);
    // A cursor inside a gap the laptop did not keep resumes on identity alone.
    if !kept_anchor {
        return Ok(true);
    }
    let range = ByteRange {
        start,
        end: old.cursor.offset,
    };
    let page = page(data, backend, run, range, Some(ident)).await?;
    Ok(matches!(page, LogPage::Data { bytes, .. } if bytes == expected))
}

async fn collect_from(
    data: &Path,
    backend: Option<&dyn Backend>,
    run: &WatchedRun,
    terminal: bool,
    now: i64,
    kept: Option<(i64, tokio::time::Instant)>,
) -> Result<bool> {
    validate_run(&run.id)?;
    let directory = data.join("run-signals");
    std::fs::create_dir_all(&directory)?;
    let path = std::fs::canonicalize(directory)?.join(format!("{}.json", run.id));
    let lock = collection_lock(&path)?;
    let _guard = lock.lock().await;
    let previous = load(&path);
    let previous = match previous {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
            let damaged = path.with_extension(format!("corrupt-{}.json", uuid::Uuid::new_v4()));
            std::fs::rename(&path, &damaged)?;
            eprintln!("run {}: {error}; retained at {}", run.id, damaged.display());
            None
        }
        Err(error) => return Err(error.into()),
    };
    let previous_error = previous
        .as_ref()
        .and_then(|s| s.extra.get("collectionError"))
        .cloned();
    let (config, patterns) = configured(&path)?;
    let hash = config.hash();
    // A host folds with the patterns it was given; one folded under others is given these.
    let reconfigured = previous
        .as_ref()
        .map_or(patterns.is_some(), |p| p.config_hash != hash);
    let native = if let Some(backend) = backend {
        if reconfigured {
            backend.configure_signals(run, &config).await?;
        }
        backend.signals(run, terminal, previous.as_ref()).await?
    } else {
        None
    };
    if let Some((mut snapshot, clock)) = native {
        let complete = finished(&snapshot);
        apply_incomplete(data, &run.id, &mut snapshot)?;
        let changed = previous.as_ref().is_none_or(|p| p != &snapshot);
        let accepted = !changed || publish(&path, &snapshot)?;
        if snapshot
            .coverage
            .reasons
            .iter()
            .any(|r| r == "collection_incomplete")
        {
            let reason = snapshot
                .extra
                .get("collectionError")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("The job host stopped before final signal collection finished");
            bail!("{reason}");
        }
        let clock = clock.filter(|_| accepted);
        collected(
            data,
            &run.id,
            now,
            snapshot.source_complete(),
            changed,
            clock,
        )?;
        return Ok(complete);
    }
    let held = previous;
    let info = match page(data, backend, run, ByteRange { start: 0, end: 0 }, None).await? {
        LogPage::Data { ident, len, .. } => Some((ident, len)),
        LogPage::Reset { info } => info.map(|i| (i.ident, Some(i.len))),
    };
    let Some((ident, len)) = info else {
        if backend.is_none() {
            return Ok(false);
        }
        if held.as_ref().is_some_and(finished) {
            return Ok(true);
        }
        if terminal || held.is_some() {
            bail!(
                "Run {} has no readable log for final signal collection",
                run.id
            );
        }
        return Ok(false);
    };
    let identity: String = ident.clone().into();
    let mut old = held.clone();
    let reconfigured = held.as_ref().is_some_and(|held| held.config_hash != hash);
    let mut resume = match &held {
        Some(held) => !reconfigured && continues(data, backend, run, held, &ident, len).await?,
        None => false,
    };
    if let Some(mut held) = held.clone().filter(|held| {
        !resume && !reconfigured && kept.is_some() && held.has_signals() && !rebuilding(held)
    }) {
        // The kept bytes are not the log the held checkpoint read: it stands, read no further.
        if set_aside(&mut held, true) {
            publish(&path, &held)?;
        }
        return Ok(true);
    }
    if !resume && held.as_ref().is_some_and(Snapshot::has_signals) {
        // Another log, folded from its start: a rebuild resumes its own staging checkpoint.
        old = match load(&path.with_extension("replay")) {
            Ok(staged) => staged,
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => None,
            Err(error) => return Err(error.into()),
        };
        resume = match &old {
            Some(staged) if staged.config_hash == hash => {
                continues(data, backend, run, staged, &ident, len).await?
            }
            _ => false,
        };
    }
    // A fold the laptop continues keeps the clock the checkpoint was written on.
    let offset = clock(data, &run.id)?.map_or(0, |(host, laptop)| host - laptop);
    let folded_at = now + offset;
    let kept = kept.map(|(as_of, deadline)| (as_of + offset, deadline));
    let mut snapshot = if resume {
        old.unwrap()
    } else {
        Snapshot::new(&run.id, &identity, &uuid::Uuid::new_v4().to_string())
    };
    if !resume && !reconfigured && held.as_ref().is_some_and(Snapshot::has_signals) {
        note_replaced(&mut snapshot, len.unwrap_or(0));
    }
    snapshot.configure(patterns);
    if !resume {
        snapshot.updated_at = folded_at;
        if let Some(cause) = previous_error {
            snapshot.extra.insert("collectionError".into(), cause);
        }
    }
    let observed = resume && backend.is_some();
    let before_rev = snapshot.rev;
    let before_coverage = snapshot.coverage.clone();
    let mut eof = false;
    for read in 1.. {
        let offset = snapshot.cursor.offset;
        let LogPage::Data {
            start,
            bytes,
            eof: last,
            ..
        } = page(
            data,
            backend,
            run,
            ByteRange {
                start: offset,
                end: offset + 256 * 1024,
            },
            Some(&FileIdent::try_from(identity.clone()).map_err(|e| anyhow!(e))?),
        )
        .await?
        else {
            bail!("Run {} log changed during signal replay", run.id)
        };
        if start < offset {
            bail!(
                "Run {} signal log page begins at {start}, expected {offset}",
                run.id
            );
        }
        if start > offset {
            snapshot.coverage.add_reason(if offset == 0 {
                "missing_prefix"
            } else {
                "missing_middle"
            });
            snapshot.cursor.offset = start;
            snapshot.cursor.frame_start = start;
            snapshot.cursor.pending_base64.clear();
        }
        eof = last;
        if terminal && eof {
            snapshot.remove_reason("collection_incomplete");
            if backend.is_none()
                && crate::commands::up::api_v2::kept_log(data, &run.id)?
                    .last()
                    .is_some_and(|r| !r.eof)
            {
                snapshot.coverage.add_reason("missing_suffix");
            }
        }
        snapshot
            .feed(&bytes, terminal && eof, folded_at, observed)
            .map_err(|e| anyhow!(e))?;

        // A kept fold is local work: it yields at its deadline instead of holding up other runs,
        // and publishes once per pass. A fetched page is published as it lands.
        let stop = eof
            || match kept {
                Some((_, deadline)) => tokio::time::Instant::now() >= deadline,
                None => read == 4,
            };
        if !stop && kept.is_some() {
            tokio::task::yield_now().await;
            continue;
        }
        // A kept log ended with the run; a page read carries no time the log last changed.
        let end = kept.map_or(folded_at, |(as_of, _)| as_of);
        if eof {
            snapshot.remove_reason("catching_up");
            snapshot.reached_end(end);
        } else {
            snapshot.coverage.add_reason("catching_up");
        }
        if kept.is_some() {
            snapshot.updated_at = end;
        }
        if snapshot.source_complete() {
            snapshot.extra.remove("collectionError");
            snapshot.extra.remove("hostGone");
        }
        use base64::Engine;
        let anchor = page(
            data,
            backend,
            run,
            ByteRange {
                start: snapshot.cursor.offset.saturating_sub(64),
                end: snapshot.cursor.offset,
            },
            Some(&ident),
        )
        .await?;
        if let LogPage::Data { bytes, .. } = anchor {
            snapshot.extra.insert(
                "anchorBase64".into(),
                serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(bytes)),
            );
        }
        apply_incomplete(data, &run.id, &mut snapshot)?;
        let changed = !resume || snapshot.rev != before_rev || snapshot.coverage != before_coverage;
        if changed {
            publish(&path, &snapshot)?;
        }
        collected(
            data,
            &run.id,
            now,
            snapshot.source_complete(),
            changed,
            None,
        )?;
        if stop {
            break;
        }
    }
    Ok(eof)
}

pub(crate) fn availability(
    data: &Path,
    run: &str,
    state: &str,
    reason: Option<&str>,
) -> Result<()> {
    validate_run(run)?;
    let directory = data.join("run-signals");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join(format!("{run}.status.json"));
    let previous = read_availability(data, run)?;
    if previous
        .as_ref()
        .is_some_and(|v| v["state"] == "incomplete")
    {
        return Ok(());
    }
    let reason = reason.map(|text| {
        let mut bounded = text.chars().take(4096).collect::<String>();
        if bounded.len() < text.len() {
            bounded.push_str("… (reason truncated)");
        }
        bounded
    });
    let mut next = previous.clone().unwrap_or_else(|| serde_json::json!({}));
    next["state"] = serde_json::json!(state);
    next["reason"] = serde_json::json!(reason);
    if previous.as_ref() != Some(&next) {
        crate::local::git::atomic_write_with_mode(&path, &serde_json::to_vec(&next)?, Some(0o600))?;
    }
    Ok(())
}

/// Records a successful fetch, and with it the `(host, laptop)` clocks of the answer that
/// brought it: the pair a checkpoint's host times are read through. A fetch that found nothing
/// new rewrites at most once a minute, so the time never trails the data it brought.
fn collected(
    data: &Path,
    run: &str,
    now: i64,
    complete: bool,
    changed: bool,
    clock: Option<(i64, i64)>,
) -> Result<()> {
    let previous = read_availability(data, run)?.unwrap_or_else(|| serde_json::json!({}));
    let mut status = previous.clone();
    if status["state"] != "incomplete" || complete {
        status["state"] = serde_json::json!("cached");
        status["reason"] = serde_json::Value::Null;
    }
    let recent = previous["lastCollectedAt"]
        .as_i64()
        .is_some_and(|at| (0..60_000).contains(&(now - at)));
    if status == previous && recent && !changed {
        return Ok(());
    }
    status["lastCollectedAt"] = serde_json::json!(now);
    if let Some(clock) = clock {
        status["clock"] = serde_json::json!(clock);
    }
    let path = data.join("run-signals").join(format!("{run}.status.json"));
    crate::local::git::atomic_write_with_mode(&path, &serde_json::to_vec(&status)?, Some(0o600))?;
    Ok(())
}

/// The `(host, laptop)` clocks stored by the last successful collection from a host.
pub(crate) fn clock(data: &Path, run: &str) -> Result<Option<(i64, i64)>> {
    let status = read_availability(data, run)?;
    Ok(status.and_then(|status| serde_json::from_value(status["clock"].clone()).ok()))
}

pub(crate) fn read_availability(data: &Path, run: &str) -> Result<Option<serde_json::Value>> {
    validate_run(run)?;
    let path = data.join("run-signals").join(format!("{run}.status.json"));
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(anyhow!(
                "Read signal availability {}: {error}",
                path.display()
            ))
        }
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| anyhow!("Parse signal availability {}: {error}", path.display()))?;
    if !value.is_object() || !value["state"].is_string() {
        bail!(
            "Signal availability {} is malformed; remove that cache file to regenerate it",
            path.display()
        );
    }
    Ok(Some(value))
}

fn apply_incomplete(data: &Path, run: &str, snapshot: &mut Snapshot) -> Result<()> {
    if snapshot.source_complete() {
        return Ok(());
    }
    if let Some(status) = read_availability(data, run)?.filter(|v| v["state"] == "incomplete") {
        snapshot.coverage.add_reason("collection_incomplete");
        snapshot
            .extra
            .insert("collectionError".into(), status["reason"].clone());
    }
    Ok(())
}

/// Records why collection is behind without settling it; a later fold may still finish. The
/// first cause stands. A run that has reported nothing has nothing to be behind on.
pub(crate) fn delayed(data: &Path, run: &str, cause: &str) -> Result<()> {
    behind(data, run, "collectionError", cause, false)
}

/// The host no longer has the run, in the words of how it went: a cause more specific than any
/// failure to reach it.
#[derive(Debug)]
pub(crate) struct HostGone(pub String);

impl std::fmt::Display for HostGone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HostGone {}

/// Records that the host no longer has the run, in the words of how it went: the most specific
/// cause, which a later settlement names over any earlier one.
pub(crate) fn host_gone(data: &Path, run: &str, cause: &str) -> Result<()> {
    behind(data, run, "hostGone", cause, true)
}

fn behind(data: &Path, run: &str, key: &str, cause: &str, replace: bool) -> Result<()> {
    validate_run(run)?;
    let held = load(&data.join("run-signals").join(format!("{run}.json")));
    if held.is_ok_and(|held| !held.is_some_and(|snapshot| snapshot.has_signals())) {
        return Ok(());
    }
    availability(data, run, "stale", Some(cause))?;
    with_checkpoint(data, run, |snapshot| {
        if snapshot.source_complete() {
            return false;
        }
        let entry = snapshot.extra.entry(key.to_owned());
        if replace {
            *entry.or_default() = serde_json::json!(cause);
        } else {
            entry.or_insert_with(|| serde_json::json!(cause));
        }
        true
    })
}

/// Settles collection as incomplete: nothing will read further, so nothing says it is reading.
pub(crate) fn settle_incomplete(data: &Path, run: &str, cause: &str) -> Result<()> {
    availability(data, run, "incomplete", Some(cause))?;
    with_checkpoint(data, run, |snapshot| {
        snapshot.remove_reason("catching_up");
        if rebuilding(snapshot) {
            snapshot.remove_reason("rebuilding");
            snapshot.coverage.add_reason("log_replaced");
        }
        apply_incomplete(data, run, snapshot).is_ok()
    })
}

/// Rewrites the published checkpoint unless a collection holds it; that collection republishes.
fn with_checkpoint(
    data: &Path,
    run: &str,
    change: impl FnOnce(&mut Snapshot) -> bool,
) -> Result<()> {
    let directory = data.join("run-signals");
    std::fs::create_dir_all(&directory)?;
    let path = std::fs::canonicalize(directory)?.join(format!("{run}.json"));
    let lock = collection_lock(&path)?;
    if let Ok(_guard) = lock.try_lock() {
        // An unreadable checkpoint has nothing to rewrite; its status says why.
        if let Ok(Some(mut snapshot)) = load(&path) {
            if change(&mut snapshot) {
                publish(&path, &snapshot)?;
            }
        }
    }
    Ok(())
}

/// Whether the host that ran a finished run can still be asked for its signals.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Host {
    Has,
    Unreachable(String),
    Gone(String),
}

/// The next step for a finished run's signals.
#[derive(Debug, PartialEq)]
pub(crate) enum Next {
    /// Ask the host to fold its log to the end and fetch that checkpoint.
    AskHost,
    /// Release the machine; nothing more is asked of it first.
    Release,
    /// Continue the fold over the log the laptop kept.
    FoldKept,
    /// Keep the debt open and try again with the log debt's backoff.
    Retry(String),
    /// Settle the debt: complete, or incomplete for this cause.
    Settle(Option<String>),
}

/// The end of a run as one decision over persisted state, so a restart decides the same.
/// The host's own checkpoint is the answer while the host has the run; once it cannot be
/// asked, the laptop continues over what it kept, only where that continues the checkpoint.
/// `held` is the checkpoint, or why it cannot be read: unknown, never absent.
pub(crate) fn decide(
    released: bool,
    host: &Host,
    held: Result<Option<&Snapshot>, &str>,
    kept: &[crate::commands::up::api_v2::KeptRange],
) -> Next {
    let snapshot = held.ok().flatten();
    let finished = snapshot.filter(|s| finished(s));
    if !released {
        return if finished.is_none() && *host == Host::Has {
            Next::AskHost
        } else {
            Next::Release
        };
    }
    // Nothing the payload reported is nothing to lose: such a run settles quietly.
    let reported = held.map_or(true, |held| held.is_some_and(Snapshot::has_signals));
    if let Some(snapshot) = finished {
        let short = reported && !snapshot.source_complete();
        return Next::Settle(short.then(|| shortfall(snapshot)));
    }
    let cause = match host {
        Host::Has => return Next::AskHost,
        Host::Unreachable(cause) => return Next::Retry(cause.clone()),
        Host::Gone(cause) => cause,
    };
    let held = match held {
        Ok(held) => held,
        Err(error) => return Next::Settle(Some(error.to_owned())),
    };
    // A rebuild under way goes on over the kept copy of the log it reads, which must reach as
    // far for the rebuild to replace the held checkpoint.
    let continues = held.is_none_or(|held| {
        !held.has_signals()
            || kept.last().is_some_and(|last| {
                (String::from(last.ident.clone()) == held.cursor.ident || rebuilding(held))
                    && last.offset + last.len >= held.cursor.offset
            })
    });
    if kept.is_empty() || !continues {
        let gone = held.and_then(|held| held.extra.get("hostGone"));
        let cause = gone.and_then(serde_json::Value::as_str).unwrap_or(cause);
        return Next::Settle(reported.then(|| cause.to_owned()));
    }
    Next::FoldKept
}

/// Why a finished fold is not whole: how its host went, else the first gap in its coverage,
/// else the first failure it recorded.
fn shortfall(snapshot: &Snapshot) -> String {
    let recorded = |key: &str| snapshot.extra.get(key).and_then(serde_json::Value::as_str);
    if let Some(cause) = recorded("hostGone") {
        return cause.to_owned();
    }
    let reasons = &snapshot.coverage.reasons;
    let has = |reason: &str| reasons.iter().any(|r| r == reason);
    if has("missing_prefix") || has("missing_middle") {
        "The saved log is missing part of the run's output".into()
    } else if has("missing_suffix") {
        "The end of the run's output was not saved".into()
    } else if has("log_missing") {
        "The run's log was removed before it was read to the end".into()
    } else if has("log_replaced") {
        "The run's log changed before it was read to the end".into()
    } else if let Some(cause) = recorded("collectionError") {
        cause.to_owned()
    } else {
        "Part of the run's output could not be read".into()
    }
}

#[cfg(test)]
pub(crate) fn cached(data: &Path, run: &str) -> (Option<Snapshot>, serde_json::Value) {
    let path = data.join("run-signals").join(format!("{run}.json"));
    let result = validate_run(run).and_then(|_| load(&path));
    let (mut snapshot, error) = match result {
        Ok(s) => (s, None),
        Err(e) => (None, Some(format!("Read {}: {e}", path.display()))),
    };
    let availability = cached_status(data, run, snapshot.is_some(), error);
    if availability["state"] == "incomplete" {
        if let Some(s) = snapshot.as_mut() {
            s.coverage.add_reason("collection_incomplete");
        }
    }
    (snapshot, availability)
}

/// A run's collection status beside whether a checkpoint was `present`, or the error reading it.
pub(crate) fn cached_status(
    data: &Path,
    run: &str,
    present: bool,
    error: Option<String>,
) -> serde_json::Value {
    let status = read_availability(data, run);
    let status = match status {
        Ok(v) => v,
        Err(e) => Some(serde_json::json!({"state":"unavailable","reason":e.to_string()})),
    };
    let mut availability =
        status.unwrap_or_else(|| serde_json::json!({"state":"cached","reason":null}));
    if let Some(error) = error {
        availability["state"] = serde_json::json!("unavailable");
        availability["reason"] = serde_json::json!(error);
    } else if !present && availability["reason"].is_null() {
        availability["state"] = serde_json::json!("empty");
    }
    availability
}

#[cfg(test)]
mod collection_tests {
    use super::*;

    #[test]
    fn the_first_recorded_cause_is_the_one_settled_and_reading_stops_being_said() {
        let data = crate::local::git::TemporaryDirectory::new("orx-specific-cause").unwrap();
        let path = data.path().join("run-signals/r1.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut snapshot = Snapshot::new("r1", "1-1", "g1");
        snapshot.latest.insert("loss".into(), serde_json::json!(2));
        snapshot.coverage.add_reason("catching_up");
        snapshot.coverage.add_reason("rebuilding");
        publish(&path, &snapshot).unwrap();
        let deleted = "The rental was deleted before final signal collection";
        delayed(data.path(), "r1", deleted).unwrap();
        delayed(data.path(), "r1", "The host did not answer in time").unwrap();
        assert_eq!(
            read_availability(data.path(), "r1").unwrap().unwrap()["state"],
            "stale"
        );
        let held = load(&path).unwrap().unwrap();
        assert_eq!(held.extra["collectionError"], deleted);
        assert!(!held
            .coverage
            .reasons
            .contains(&"collection_incomplete".into()));

        settle_incomplete(data.path(), "r1", deleted).unwrap();
        let (settled, status) = cached(data.path(), "r1");
        let settled = settled.unwrap();
        assert_eq!(status["state"], "incomplete");
        assert_eq!(status["reason"], deleted);
        assert_eq!(settled.extra["collectionError"], deleted);
        assert_eq!(
            settled.coverage.reasons,
            ["collection_incomplete", "log_replaced"]
        );
    }

    fn kept(ranges: &[(u64, u64, bool)]) -> Vec<crate::commands::up::api_v2::KeptRange> {
        ranges
            .iter()
            .map(
                |&(offset, len, eof)| crate::commands::up::api_v2::KeptRange {
                    offset,
                    len,
                    file: "unused".into(),
                    ident: FileIdent { dev: 1, ino: 1 },
                    eof,
                },
            )
            .collect()
    }

    /// A host checkpoint whose cursor lies in bytes the laptop did not keep resumes on identity,
    /// and the fold writes the host's times, read through the stored pair, never the laptop's.
    #[tokio::test]
    async fn a_kept_fold_resumes_a_cursor_inside_a_gap() {
        const DAY: i64 = 86_400_000;
        let data = crate::local::git::TemporaryDirectory::new("orx-kept-gap-cursor").unwrap();
        let logs = data.path().join("run-logs");
        std::fs::create_dir_all(&logs).unwrap();
        let head = b"::orx:: {\"step\":1}\n";
        let tail = b"::orx:: {\"step\":9}\n";
        std::fs::write(logs.join("run.log"), head).unwrap();
        std::fs::write(logs.join("run.tail.log"), tail).unwrap();
        let ranges = [
            (0, head.len() as u64, "run.log", false),
            (100, tail.len() as u64, "run.tail.log", true),
        ];
        crate::commands::up::api_v2::keep_ranges(
            data.path(),
            "run",
            ranges
                .iter()
                .map(
                    |&(offset, len, file, eof)| crate::commands::up::api_v2::KeptRange {
                        offset,
                        len,
                        file: logs.join(file),
                        ident: FileIdent { dev: 1, ino: 1 },
                        eof,
                    },
                )
                .collect(),
        )
        .unwrap();
        let mut held = Snapshot::new("run", "1-1", "g1");
        held.feed(head, false, 5000 + DAY, true).unwrap();
        held.feed(&[b'\n'; 40], false, 5000 + DAY, true).unwrap();
        held.extra.insert("anchorBase64".into(), "Cgo=".into());
        let path = data.path().join("run-signals/run.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        publish(&path, &held).unwrap();
        collected(
            data.path(),
            "run",
            5000,
            false,
            true,
            Some((5000 + DAY, 5000)),
        )
        .unwrap();
        let run = WatchedRun {
            id: "run".into(),
            handle: crate::engine::Handle::Local {
                dir: "unused".into(),
            },
        };
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
        assert!(collect_kept(data.path(), &run, 6000, 6000, deadline)
            .await
            .unwrap());
        let folded = load(&path).unwrap().unwrap();
        assert_eq!(folded.generation, "g1");
        assert_eq!(folded.phases[0].started_at, Some(5000 + DAY));
        assert_eq!(folded.progress().unwrap().done, Some(9.));
        assert_eq!(folded.coverage.reasons, vec!["missing_middle"]);
        assert_eq!(
            folded.updated_at,
            6000 + DAY,
            "the run's end on the host's clock"
        );
        assert_eq!(
            folded.progress().unwrap().last_point,
            Some(((6000 + DAY) as f64 / 1000.0, 9.)),
            "its last advance sits no later than the run's end"
        );
    }

    /// The fold, `orx logs` pages and a released rental read one copy, partial files included.
    #[tokio::test]
    async fn a_kept_fold_reads_bytes_still_being_kept() {
        let data = crate::local::git::TemporaryDirectory::new("orx-kept-partial").unwrap();
        let logs = data.path().join("run-logs");
        std::fs::create_dir_all(&logs).unwrap();
        let bytes = b"::orx:: {\"step\":4,\"m\":{\"score\":3}}\nnot yet synced";
        std::fs::write(logs.join("run.log.part"), bytes).unwrap();
        let synced = bytes.len() as u64 - 14;
        std::fs::write(
            logs.join("run.log.part.json"),
            serde_json::to_vec(&(FileIdent { dev: 1, ino: 1 }, 0u64, synced)).unwrap(),
        )
        .unwrap();
        let run = WatchedRun {
            id: "run".into(),
            handle: crate::engine::Handle::Local {
                dir: "unused".into(),
            },
        };
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
        assert!(collect_kept(data.path(), &run, 6000, 6000, deadline)
            .await
            .unwrap());
        let folded = load(&data.path().join("run-signals/run.json"))
            .unwrap()
            .unwrap();
        assert_eq!(folded.cursor.ident, "1-1");
        assert_eq!(folded.cursor.offset, synced);
        assert_eq!(folded.latest["score"], 3);
        assert_eq!(folded.coverage.reasons, vec!["missing_suffix"]);
    }

    /// Another generation that covers fewer bytes is staged while the held checkpoint stands,
    /// marked rebuilding; if it ends short, the held one is final and incomplete and the staging
    /// one goes; one that covers the held bytes replaces it.
    #[test]
    fn a_checkpoint_is_never_replaced_by_one_that_knows_less() {
        let data = crate::local::git::TemporaryDirectory::new("orx-knows-less").unwrap();
        let path = data.path().join("run-signals/r1.json");
        let staging = path.with_extension("replay");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let log = b"::orx:: {\"phase\":\"train\",\"step\":1,\"of\":4,\"t\":1}\n::orx:: {\"step\":2,\"t\":2}\n";
        let mut held = Snapshot::new("r1", "1-1", "g1");
        held.feed(log, false, 5000, true).unwrap();
        assert!(publish(&path, &held).unwrap());

        let mut prefix = Snapshot::new("r1", "1-1", "g2");
        prefix.feed(&log[..20], false, 6000, false).unwrap();
        assert!(!publish(&path, &prefix).unwrap());
        let stood = load(&path).unwrap().unwrap();
        assert_eq!(stood.generation, "g1");
        assert_eq!(stood.timing.progress_points, held.timing.progress_points);
        assert_eq!(stood.coverage.reasons, ["rebuilding"]);
        assert_ne!(stood.extra.get("terminal"), Some(&true.into()));
        assert_eq!(load(&staging).unwrap().unwrap(), prefix);

        prefix.feed(&[], true, 6000, false).unwrap();
        publish(&path, &prefix).unwrap();
        let last = load(&path).unwrap().unwrap();
        assert_eq!(last.generation, "g1");
        assert_eq!(last.coverage.reasons, ["log_replaced"]);
        assert_eq!(last.extra["terminal"], true);
        assert!(last.rev > stood.rev);
        assert!(!staging.exists(), "a rebuild that ended short is dropped");

        let mut longer = Snapshot::new("r1", "2-2", "g3");
        longer.feed(&log[..20], false, 7000, false).unwrap();
        publish(&path, &longer).unwrap();
        longer.feed(&log[20..], true, 7000, false).unwrap();
        assert!(publish(&path, &longer).unwrap());
        assert_eq!(load(&path).unwrap().unwrap().generation, "g3");
        assert!(!staging.exists(), "a promoted rebuild leaves no staging");

        // Only what the held one covered counts, every range of it.
        let mut gapped = Snapshot::new("r1", "3-3", "g4");
        gapped.feed(&log[..20], false, 8000, false).unwrap();
        gapped.coverage.ranges.push((60, 70));
        gapped.latest.insert("loss".into(), serde_json::json!(1));
        std::fs::remove_file(&path).unwrap();
        publish(&path, &gapped).unwrap();
        let mut first_range = Snapshot::new("r1", "4-4", "g5");
        first_range.feed(&log[..30], false, 9000, false).unwrap();
        assert!(
            !publish(&path, &first_range).unwrap(),
            "every range, not any"
        );

        let mut silent = Snapshot::new("r1", "5-5", "g6");
        silent
            .feed(
                b"plain output, more than thirty bytes long\n",
                false,
                9000,
                false,
            )
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        publish(&path, &silent).unwrap();
        assert!(
            publish(&path, &first_range).unwrap(),
            "a checkpoint that reported nothing has nothing to lose"
        );
    }

    #[test]
    fn the_end_of_a_run_is_one_decision() {
        let silent = Snapshot::new("r1", "1-1", "g1");
        let mut reading = silent.clone();
        reading.latest.insert("loss".into(), serde_json::json!(2));
        let mut whole = reading.clone();
        whole.feed(&[], true, 0, false).unwrap();
        let mut short = whole.clone();
        short.coverage.add_reason("missing_middle");
        let mut refused = reading.clone();
        refused
            .extra
            .insert("collectionError".into(), "connection refused".into());
        let mut deleted = refused.clone();
        deleted
            .extra
            .insert("hostGone".into(), "rental deleted".into());
        let mut ahead = reading.clone();
        ahead.cursor.offset = 125;
        let gone = Host::Gone("released".into());
        let away = Host::Unreachable("connection refused".into());
        let some = kept(&[(0, 10, true)]);
        let prefix = kept(&[(0, 57, true)]);
        let reaching = kept(&[(0, 57, false), (100, 25, true)]);
        let mut emptied = kept(&[(0, 0, true)]);
        emptied[0].ident = FileIdent { dev: 0, ino: 0 };
        let none = kept(&[]);
        let settle = |cause: Option<&str>| Next::Settle(cause.map(str::to_owned));
        let unreadable = Err("Run r1's signal checkpoint cannot be read");
        type Row<'a> = (
            bool,
            &'a Host,
            Result<Option<&'a Snapshot>, &'a str>,
            &'a [crate::commands::up::api_v2::KeptRange],
            Next,
        );
        let table: Vec<Row> = vec![
            (false, &Host::Has, Ok(None), &none, Next::AskHost),
            (false, &Host::Has, Ok(Some(&reading)), &some, Next::AskHost),
            (false, &Host::Has, Ok(Some(&whole)), &some, Next::Release),
            (false, &Host::Has, unreadable, &some, Next::AskHost),
            (false, &gone, Ok(Some(&reading)), &some, Next::Release),
            (true, &Host::Has, Ok(Some(&reading)), &some, Next::AskHost),
            (true, &Host::Has, Ok(None), &some, Next::AskHost),
            (true, &Host::Has, unreadable, &none, Next::AskHost),
            (true, &Host::Has, Ok(Some(&whole)), &none, settle(None)),
            (
                true,
                &away,
                Ok(Some(&reading)),
                &some,
                Next::Retry("connection refused".into()),
            ),
            (
                true,
                &away,
                unreadable,
                &some,
                Next::Retry("connection refused".into()),
            ),
            (true, &away, Ok(Some(&whole)), &some, settle(None)),
            (true, &gone, Ok(Some(&reading)), &some, Next::FoldKept),
            (true, &gone, Ok(None), &some, Next::FoldKept),
            (true, &gone, Ok(Some(&whole)), &some, settle(None)),
            (
                true,
                &gone,
                Ok(Some(&short)),
                &some,
                settle(Some("The saved log is missing part of the run's output")),
            ),
            (
                true,
                &gone,
                Ok(Some(&reading)),
                &none,
                settle(Some("released")),
            ),
            (
                true,
                &gone,
                Ok(Some(&refused)),
                &none,
                settle(Some("released")),
            ),
            (
                true,
                &gone,
                Ok(Some(&deleted)),
                &none,
                settle(Some("rental deleted")),
            ),
            (
                true,
                &gone,
                Ok(Some(&ahead)),
                &prefix,
                settle(Some("released")),
            ),
            (
                true,
                &gone,
                Ok(Some(&deleted)),
                &emptied,
                settle(Some("rental deleted")),
            ),
            (
                true,
                &gone,
                Ok(Some(&reading)),
                &emptied,
                settle(Some("released")),
            ),
            (true, &gone, Ok(Some(&ahead)), &reaching, Next::FoldKept),
            (
                true,
                &gone,
                unreadable,
                &none,
                settle(Some("Run r1's signal checkpoint cannot be read")),
            ),
            (
                true,
                &gone,
                unreadable,
                &some,
                settle(Some("Run r1's signal checkpoint cannot be read")),
            ),
            (true, &gone, Ok(None), &none, settle(None)),
            (true, &gone, Ok(Some(&silent)), &none, settle(None)),
            (true, &gone, Ok(Some(&silent)), &some, Next::FoldKept),
            (true, &gone, Ok(Some(&silent)), &emptied, Next::FoldKept),
        ];
        let mut quiet = silent.clone();
        quiet.feed(&[], true, 0, false).unwrap();
        quiet.coverage.add_reason("missing_middle");
        assert_eq!(decide(true, &gone, Ok(Some(&quiet)), &some), settle(None));
        for (index, (released, host, held, ranges, next)) in table.into_iter().enumerate() {
            assert_eq!(decide(released, host, held, ranges), next, "row {index}");
        }
    }

    struct Pages {
        source: crate::engine::fake::FakeBackend,
        pause: std::sync::atomic::AtomicBool,
        blocked: tokio::sync::Notify,
    }
    #[async_trait::async_trait]
    impl Backend for Pages {
        async fn launch(
            &self,
            _: &crate::engine::RunIntent,
            _: &crate::engine::Fence<'_>,
        ) -> std::result::Result<crate::engine::Handle, crate::engine::LaunchError> {
            unreachable!()
        }
        async fn recover(&self, _: &crate::engine::RunIntent) -> crate::engine::Recovery {
            unreachable!()
        }
        async fn observe(
            &self,
            _: &[WatchedRun],
            _: std::time::Duration,
        ) -> Vec<(String, crate::engine::Observation)> {
            unreachable!()
        }
        async fn cancel(&self, _: &WatchedRun) -> crate::engine::CancelOutcome {
            unreachable!()
        }
        async fn read_log(
            &self,
            run: &WatchedRun,
            file: LogFile,
            range: ByteRange,
            ident: Option<&FileIdent>,
        ) -> Result<LogPage> {
            if range.start == 256 * 1024
                && range.end - range.start > 64
                && self.pause.load(std::sync::atomic::Ordering::SeqCst)
            {
                self.blocked.notify_one();
                std::future::pending::<()>().await;
            }
            self.source.read_log(run, file, range, ident).await
        }
    }
    #[tokio::test]
    async fn interrupted_fallback_keeps_completed_pages_and_independent_coverage() {
        let data = crate::local::git::TemporaryDirectory::new("orx-page-checkpoint").unwrap();
        let backend = Pages {
            source: Default::default(),
            pause: true.into(),
            blocked: Default::default(),
        };
        let mut bytes = b"::orx:: {\"row\":\"eval\",\"m\":{\"score\":1}}\n".to_vec();
        for i in 0..65 {
            bytes.extend_from_slice(format!("::orx:: {{\"m\":{{\"m{i}\":1}}}}\n").as_bytes());
        }
        bytes.extend_from_slice(&vec![b'\n'; 512 * 1024]);
        backend.source.log.lock().unwrap().bytes = bytes;
        let run = WatchedRun {
            id: "r1".into(),
            handle: crate::engine::Handle::Local {
                dir: "unused".into(),
            },
        };
        let mut task = Box::pin(collect(data.path(), &backend, &run, false, 123));
        tokio::select! {result=&mut task=>panic!("unexpected early finish {result:?}"),_=backend.blocked.notified()=>{}}
        drop(task);
        let saved = load(&data.path().join("run-signals/r1.json"))
            .unwrap()
            .unwrap();
        assert_eq!(saved.cursor.offset, 256 * 1024);
        assert!(saved.coverage.reasons.contains(&"truncated".into()));
        assert!(saved.coverage.reasons.contains(&"catching_up".into()));
        backend
            .pause
            .store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(collect(data.path(), &backend, &run, true, 124)
            .await
            .unwrap());
        let saved = load(&data.path().join("run-signals/r1.json"))
            .unwrap()
            .unwrap();
        assert_eq!(saved.rows["eval"].len(), 1);
        assert!(!saved.coverage.complete);
        assert_eq!(saved.coverage.reasons, vec!["truncated"]);
    }
    #[tokio::test]
    async fn fallback_detects_same_inode_rewrites() {
        let data = crate::local::git::TemporaryDirectory::new("orx-page-anchor").unwrap();
        let backend = crate::engine::fake::FakeBackend::default();
        let run = WatchedRun {
            id: "r1".into(),
            handle: crate::engine::Handle::Local {
                dir: "unused".into(),
            },
        };
        backend.log.lock().unwrap().bytes = b"::orx:: {\"m\":{\"score\":1}}\n".to_vec();
        collect(data.path(), &backend, &run, false, 1)
            .await
            .unwrap();
        let path = data.path().join("run-signals/r1.json");
        let before = load(&path).unwrap().unwrap();
        backend.log.lock().unwrap().bytes = b"::orx:: {\"m\":{\"score\":9}}\n".to_vec();
        collect(data.path(), &backend, &run, false, 2)
            .await
            .unwrap();
        let after = load(&path).unwrap().unwrap();
        assert_ne!(before.generation, after.generation);
        assert_eq!(after.latest["score"], 9);
        collect(data.path(), &backend, &run, true, 3).await.unwrap();
        let terminal = load(&path).unwrap().unwrap();
        backend
            .log
            .lock()
            .unwrap()
            .bytes
            .extend_from_slice(b"::orx:: {\"m\":{\"score\":10}}\n");
        collect(data.path(), &backend, &run, true, 4).await.unwrap();
        let appended = load(&path).unwrap().unwrap();
        assert_ne!(terminal.generation, appended.generation);
        assert_eq!(appended.latest["score"], 10);
    }
    #[tokio::test]
    async fn failed_collection_and_contended_incomplete_keep_the_last_good_checkpoint() {
        let data = crate::local::git::TemporaryDirectory::new("orx-signal-status").unwrap();
        let path = data.path().join("run-signals/r1.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        availability(data.path(), "r1", "stale", Some("first fetch failed")).unwrap();
        let (missing, status) = cached(data.path(), "r1");
        assert!(missing.is_none());
        assert_eq!(status["state"], "stale");
        assert_eq!(status["reason"], "first fetch failed");
        assert!(status["lastCollectedAt"].is_null());
        let mut snapshot = Snapshot::new("r1", "1-100", "g1");
        snapshot.updated_at = 123;
        snapshot.latest.insert("loss".into(), serde_json::json!(2));
        publish(&path, &snapshot).unwrap();
        collected(data.path(), "r1", 500, false, true, Some((123, 500))).unwrap();
        availability(data.path(), "r1", "stale", Some("host disconnected")).unwrap();
        let (cached, status) = cached(data.path(), "r1");
        assert_eq!(cached.unwrap().latest["loss"], 2);
        assert_eq!(status["lastCollectedAt"], 500);
        assert_eq!(clock(data.path(), "r1").unwrap(), Some((123, 500)));
        assert_eq!(status["state"], "stale");
        collected(data.path(), "r1", 90_000, false, false, None).unwrap();
        let (_, status) = super::cached(data.path(), "r1");
        assert_eq!(status["lastCollectedAt"], 90_000);
        assert_eq!(
            clock(data.path(), "r1").unwrap(),
            Some((123, 500)),
            "a fold on this machine keeps the host's pair"
        );
        let lock = collection_lock(&std::fs::canonicalize(&path).unwrap()).unwrap();
        let guard = lock.lock().await;
        settle_incomplete(data.path(), "r1", "rental released").unwrap();
        assert_eq!(
            read_availability(data.path(), "r1").unwrap().unwrap()["state"],
            "incomplete"
        );
        availability(data.path(), "r1", "stale", Some("late live timeout")).unwrap();
        assert_eq!(
            read_availability(data.path(), "r1").unwrap().unwrap()["state"],
            "incomplete"
        );
        drop(guard);
        std::fs::write(&path, b"{broken").unwrap();
        let (snapshot, status) = super::cached(data.path(), "r1");
        assert!(snapshot.is_none());
        assert_eq!(status["state"], "unavailable");
        assert_eq!(status["lastCollectedAt"], 90_000);
        assert!(status["reason"].as_str().unwrap().contains("r1.json"));
    }

    fn padded(steps: std::ops::Range<u32>) -> Vec<u8> {
        let pad = "x".repeat(400);
        steps
            .map(|step| {
                format!("::orx:: {{\"step\":{step},\"of\":8000,\"m\":{{\"pad\":\"{pad}\"}}}}\n")
            })
            .collect::<String>()
            .into_bytes()
    }

    fn local_run() -> WatchedRun {
        WatchedRun {
            id: "r1".into(),
            handle: crate::engine::Handle::Local {
                dir: "unused".into(),
            },
        }
    }

    /// A log that changed identity is folded again from its start into a staging checkpoint
    /// that resumes across passes and interruptions; the held one stands, marked rebuilding,
    /// until the rebuild covers its bytes and replaces it.
    #[tokio::test]
    async fn a_rebuild_resumes_its_staging_checkpoint_until_it_covers_the_held_one() {
        let data = crate::local::git::TemporaryDirectory::new("orx-rebuild").unwrap();
        let path = data.path().join("run-signals/r1.json");
        let staging = path.with_extension("replay");
        let backend = Pages {
            source: Default::default(),
            pause: false.into(),
            blocked: Default::default(),
        };
        let run = local_run();
        backend.source.log.lock().unwrap().bytes = padded(0..5000);
        let mut passes = 0;
        while !collect(data.path(), &backend, &run, false, 1000)
            .await
            .unwrap()
        {
            passes += 1;
        }
        let held = load(&path).unwrap().unwrap();
        assert!(passes > 1 && held.cursor.offset > 2_000_000);
        assert_eq!(held.progress().unwrap().done, Some(4999.));

        {
            let mut log = backend.source.log.lock().unwrap();
            log.ino += 1;
            log.bytes = padded(0..8000);
        }
        backend
            .pause
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let mut task = Box::pin(collect(data.path(), &backend, &run, false, 2000));
        tokio::select! {result=&mut task=>panic!("unexpected early finish {result:?}"),_=backend.blocked.notified()=>{}}
        drop(task);
        assert_eq!(load(&staging).unwrap().unwrap().cursor.offset, 256 * 1024);
        let stood = load(&path).unwrap().unwrap();
        assert_eq!(stood.generation, held.generation);
        assert_eq!(stood.coverage.reasons, ["rebuilding"]);
        backend
            .pause
            .store(false, std::sync::atomic::Ordering::SeqCst);

        let mut offsets = Vec::new();
        for pass in 0.. {
            assert!(pass < 20, "the rebuild never caught up");
            let eof = collect(data.path(), &backend, &run, pass > 10, 3000)
                .await
                .unwrap();
            let published = load(&path).unwrap().unwrap();
            if published.generation != held.generation {
                break;
            }
            offsets.push(load(&staging).unwrap().unwrap().cursor.offset);
            assert!(!eof);
        }
        assert_eq!(
            offsets.first(),
            Some(&(256 * 1024 + 1024 * 1024)),
            "the interrupted pass resumed"
        );
        assert!(!staging.exists());
        while !collect(data.path(), &backend, &run, true, 4000)
            .await
            .unwrap()
        {}
        let caught_up = load(&path).unwrap().unwrap();
        assert_eq!(caught_up.progress().unwrap().done, Some(7999.));
        assert!(caught_up.source_complete(), "{:?}", caught_up.coverage);
    }

    /// A rebuild whose log ends short of what the held checkpoint read leaves that checkpoint
    /// standing, final and incomplete with the reason, and drops the staging one.
    #[tokio::test]
    async fn a_rebuild_that_ends_short_leaves_the_held_checkpoint_incomplete() {
        let data = crate::local::git::TemporaryDirectory::new("orx-rebuild-short").unwrap();
        let path = data.path().join("run-signals/r1.json");
        let backend = crate::engine::fake::FakeBackend::default();
        let run = local_run();
        backend.log.lock().unwrap().bytes = padded(0..100);
        assert!(collect(data.path(), &backend, &run, false, 1000)
            .await
            .unwrap());
        let held = load(&path).unwrap().unwrap();
        {
            let mut log = backend.log.lock().unwrap();
            log.ino += 1;
            log.bytes = padded(0..10);
        }
        assert!(collect(data.path(), &backend, &run, true, 2000)
            .await
            .unwrap());
        let settled = load(&path).unwrap().unwrap();
        assert_eq!(settled.generation, held.generation);
        assert_eq!(settled.progress().unwrap().done, Some(99.));
        assert_eq!(settled.extra["terminal"], true);
        assert_eq!(settled.coverage.reasons, ["log_replaced"]);
        assert_eq!(
            shortfall(&settled),
            "The run's log changed before it was read to the end"
        );
        assert!(!path.with_extension("replay").exists());
    }

    /// A staging checkpoint that cannot be read is no reason to stop: the rebuild starts over,
    /// and the checkpoint it promotes says the log was replaced.
    #[tokio::test]
    async fn a_corrupt_staging_checkpoint_restarts_the_rebuild() {
        let data = crate::local::git::TemporaryDirectory::new("orx-corrupt-staging").unwrap();
        let path = data.path().join("run-signals/r1.json");
        let staging = path.with_extension("replay");
        let backend = crate::engine::fake::FakeBackend::default();
        let run = local_run();
        backend.log.lock().unwrap().bytes = padded(0..100);
        collect(data.path(), &backend, &run, false, 1000)
            .await
            .unwrap();
        let held = load(&path).unwrap().unwrap();
        std::fs::write(&staging, b"{ not a checkpoint").unwrap();
        {
            let mut log = backend.log.lock().unwrap();
            log.ino += 1;
            log.bytes = padded(0..200);
        }
        assert!(collect(data.path(), &backend, &run, false, 2000)
            .await
            .unwrap());
        let rebuilt = load(&path).unwrap().unwrap();
        assert_ne!(rebuilt.generation, held.generation);
        assert_eq!(rebuilt.progress().unwrap().done, Some(199.));
        assert_eq!(rebuilt.diagnostics.counts["log_replaced"], 1);
        assert!(!staging.exists());
    }

    /// A staging checkpoint left by a crash after its promotion, or by a run settled mid-rebuild,
    /// goes with the next publication of a checkpoint not rebuilding.
    #[test]
    fn a_staging_checkpoint_goes_once_nothing_is_rebuilding() {
        let data = crate::local::git::TemporaryDirectory::new("orx-staging-left").unwrap();
        let path = data.path().join("run-signals/r1.json");
        let staging = path.with_extension("replay");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut held = Snapshot::new("r1", "1-1", "g1");
        held.feed(b"::orx:: {\"m\":{\"loss\":1}}\n", false, 0, false)
            .unwrap();
        held.coverage.add_reason("rebuilding");
        publish(&path, &held).unwrap();
        std::fs::write(&staging, b"{}").unwrap();
        publish(&path, &held).unwrap();
        assert!(staging.exists(), "a rebuild under way keeps its own");

        settle_incomplete(data.path(), "r1", "released").unwrap();
        let settled = load(&path).unwrap().unwrap();
        assert!(settled.coverage.reasons.contains(&"log_replaced".into()));
        assert!(!staging.exists());

        std::fs::write(&staging, b"{}").unwrap();
        held.remove_reason("rebuilding");
        held.feed(b"\n", false, 0, false).unwrap();
        publish(&path, &held).unwrap();
        assert!(!staging.exists());
    }

    /// A fold the laptop continues over a host's checkpoint writes the host's clock, through
    /// the stored pair, for a phase that opens in what it reads.
    #[tokio::test]
    async fn a_fold_the_laptop_continues_keeps_the_hosts_clock() {
        const DAY: i64 = 86_400_000;
        let data = crate::local::git::TemporaryDirectory::new("orx-continued-clock").unwrap();
        let path = data.path().join("run-signals/r1.json");
        let backend = crate::engine::fake::FakeBackend::default();
        let run = local_run();
        backend.log.lock().unwrap().bytes =
            b"::orx:: {\"phase\":\"train\",\"step\":1,\"of\":2}\n".to_vec();
        collect(data.path(), &backend, &run, false, 1000 + DAY)
            .await
            .unwrap();
        collected(
            data.path(),
            "r1",
            1000,
            false,
            true,
            Some((1000 + DAY, 1000)),
        )
        .unwrap();
        backend
            .log
            .lock()
            .unwrap()
            .bytes
            .extend_from_slice(b"::orx:: {\"phase\":\"eval\",\"step\":0,\"of\":4}\n");
        collect(data.path(), &backend, &run, false, 7000)
            .await
            .unwrap();
        let folded = load(&path).unwrap().unwrap();
        assert_eq!(folded.phases[0].ended_at, Some(7000 + DAY));
        assert_eq!(folded.phases[1].started_at, Some(7000 + DAY));
        assert_eq!(folded.updated_at, 7000 + DAY);
    }

    /// A byte-page host gone mid-rebuild: the rebuild goes on over the log the laptop kept,
    /// which reaches as far, and replaces the held checkpoint instead of setting it aside.
    #[tokio::test]
    async fn a_rebuild_under_way_finishes_over_the_kept_log() {
        let data = crate::local::git::TemporaryDirectory::new("orx-kept-rebuild").unwrap();
        let path = data.path().join("run-signals/r1.json");
        let staging = path.with_extension("replay");
        let backend = Pages {
            source: Default::default(),
            pause: false.into(),
            blocked: Default::default(),
        };
        let run = local_run();
        backend.source.log.lock().unwrap().bytes = padded(0..5000);
        while !collect(data.path(), &backend, &run, false, 1000)
            .await
            .unwrap()
        {}
        let replaced = padded(0..8000);
        {
            let mut log = backend.source.log.lock().unwrap();
            log.ino += 1;
            log.bytes = replaced.clone();
        }
        assert!(!collect(data.path(), &backend, &run, false, 2000)
            .await
            .unwrap());
        let staged = load(&staging).unwrap().unwrap();
        let held = load(&path).unwrap().unwrap();
        assert_eq!(held.coverage.reasons, ["rebuilding"]);

        let logs = data.path().join("run-logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("r1.log"), &replaced).unwrap();
        let range = crate::commands::up::api_v2::KeptRange {
            offset: 0,
            len: replaced.len() as u64,
            file: logs.join("r1.log"),
            ident: FileIdent { dev: 2, ino: 1 },
            eof: true,
        };
        crate::commands::up::api_v2::keep_ranges(data.path(), "r1", vec![range]).unwrap();
        let whole = crate::commands::up::api_v2::kept_log(data.path(), "r1").unwrap();
        let gone = Host::Gone("released".into());
        assert_eq!(decide(true, &gone, Ok(Some(&held)), &whole), Next::FoldKept);
        let short = kept(&[(0, 1000, true)]);
        assert_eq!(
            decide(true, &gone, Ok(Some(&held)), &short),
            Next::Settle(Some("released".into())),
            "a kept log that does not reach as far settles"
        );

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
        while !collect_kept(data.path(), &run, 3000, 3000, deadline)
            .await
            .unwrap()
        {}
        let finished = load(&path).unwrap().unwrap();
        assert_eq!(finished.generation, staged.generation);
        assert_eq!(finished.progress().unwrap().done, Some(7999.));
        assert!(finished.source_complete(), "{:?}", finished.coverage);
        assert_eq!(finished.diagnostics.counts["log_replaced"], 1);
        assert!(!staging.exists());
    }

    /// Answers as a host ten minutes ahead of the laptop: a checkpoint and when it was asked.
    struct Answers(std::sync::Mutex<std::collections::VecDeque<Result<(Snapshot, i64)>>>);
    #[async_trait::async_trait]
    impl Backend for Answers {
        async fn launch(
            &self,
            _: &crate::engine::RunIntent,
            _: &crate::engine::Fence<'_>,
        ) -> std::result::Result<crate::engine::Handle, crate::engine::LaunchError> {
            unreachable!()
        }
        async fn recover(&self, _: &crate::engine::RunIntent) -> crate::engine::Recovery {
            unreachable!()
        }
        async fn observe(
            &self,
            _: &[WatchedRun],
            _: std::time::Duration,
        ) -> Vec<(String, crate::engine::Observation)> {
            unreachable!()
        }
        async fn cancel(&self, _: &WatchedRun) -> crate::engine::CancelOutcome {
            unreachable!()
        }
        async fn read_log(
            &self,
            _: &WatchedRun,
            _: LogFile,
            _: ByteRange,
            _: Option<&FileIdent>,
        ) -> Result<LogPage> {
            unreachable!()
        }
        async fn signals(
            &self,
            _: &WatchedRun,
            _: bool,
            _: Option<&Snapshot>,
        ) -> Result<Option<(Snapshot, Option<(i64, i64)>)>> {
            let (answer, laptop) = self.0.lock().unwrap().pop_front().unwrap()?;
            Ok(Some((answer, Some((laptop + 600_000, laptop)))))
        }
    }

    /// The pair of host and laptop clocks moves with every successful collection and only
    /// then: a silent run keeps a correct clock, a staged rebuild or a failure moves nothing,
    /// and a catch-up after an outage is current at once.
    #[tokio::test]
    async fn the_hosts_clock_moves_only_with_a_successful_collection() {
        let data = crate::local::git::TemporaryDirectory::new("orx-host-clock").unwrap();
        let run = local_run();
        let skew = 600_000;
        let host = |laptop: i64| laptop + skew;
        let mut folded = Snapshot::new("r1", "1-1", "g1");
        for step in 0..=5 {
            let line = format!("::orx:: {{\"step\":{step},\"of\":100,\"t\":{step}}}\n");
            folded.feed(line.as_bytes(), false, host(0), true).unwrap();
        }
        folded.reached_end(host(0));
        let answers = Answers(Default::default());
        let ask = |at: i64, reply: Result<Snapshot>| {
            answers.0.lock().unwrap().push_back(reply.map(|s| (s, at)));
            collect(data.path(), &answers, &run, false, at)
        };
        ask(0, Ok(folded.clone())).await.unwrap();
        assert_eq!(clock(data.path(), "r1").unwrap(), Some((host(0), 0)));
        // Each answer is the same checkpoint; only the host's clock moves.
        for at in (5_000..=600_000).step_by(5_000) {
            ask(at, Ok(folded.clone())).await.unwrap();
        }
        let (host_at, laptop_at) = clock(data.path(), "r1").unwrap().unwrap();
        assert_eq!(host_at - laptop_at, skew);
        assert!(
            laptop_at >= 540_000,
            "a silent run's clock trails by at most a minute"
        );
        let held = load(&data.path().join("run-signals/r1.json"))
            .unwrap()
            .unwrap();
        let input = crate::engine::estimate::with_signals(
            run_facts(),
            &held,
            &[],
            Some((host_at as f64, laptop_at as f64)),
            600_000.0,
        );
        let out = crate::engine::estimate::estimate(&input).unwrap();
        assert_eq!(out.stall_ms, Some(600_000.0), "ten silent minutes show");

        let mut rebuilt = Snapshot::new("r1", "2-2", "g2");
        rebuilt.updated_at = host(700_000);
        ask(700_000, Ok(rebuilt)).await.unwrap();
        assert_eq!(
            clock(data.path(), "r1").unwrap(),
            Some((host_at, laptop_at))
        );
        assert!(ask(710_000, Err(anyhow!("host unreachable")))
            .await
            .is_err());
        assert_eq!(
            clock(data.path(), "r1").unwrap(),
            Some((host_at, laptop_at))
        );

        let after_outage = 600_000 + 3_600_000;
        ask(after_outage, Ok(folded.clone())).await.unwrap();
        assert_eq!(
            clock(data.path(), "r1").unwrap(),
            Some((host(after_outage), after_outage))
        );
    }

    fn run_facts() -> crate::engine::estimate::Run {
        crate::engine::estimate::Run {
            started_at: 0.0,
            contact_lost: false,
            ends_at: None,
            machine: crate::engine::estimate::Machine::Unpriced,
            gpu_count: None,
            spend: crate::engine::estimate::Spend::NeverReported,
            spend_per_hour: None,
        }
    }

    /// How the host went is the most specific cause: the latest word on it stands.
    #[test]
    fn the_latest_word_on_how_the_host_went_stands() {
        let data = crate::local::git::TemporaryDirectory::new("orx-host-gone").unwrap();
        let path = data.path().join("run-signals/r1.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut snapshot = Snapshot::new("r1", "1-1", "g1");
        snapshot.latest.insert("loss".into(), serde_json::json!(2));
        publish(&path, &snapshot).unwrap();
        delayed(data.path(), "r1", "host unreachable").unwrap();
        delayed(data.path(), "r1", "still unreachable").unwrap();
        host_gone(data.path(), "r1", "The rental was released").unwrap();
        host_gone(data.path(), "r1", "The rental was deleted").unwrap();
        let held = load(&path).unwrap().unwrap();
        assert_eq!(held.extra["collectionError"], "host unreachable");
        assert_eq!(held.extra["hostGone"], "The rental was deleted");
    }
}
