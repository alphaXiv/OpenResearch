//! A run's monitor: what its line, strip, monitor route and `orx exp status` read. One projection
//! from the fold's checkpoint, the estimator and the run's lifecycle, evaluated on the server and
//! numbered by `rev`, which only grows and survives restarts through `run_monitor_state`.

use super::adapters::signals::{cached_status, REFOLD_FAILED};
use super::estimate;
use super::Phase;
use crate::error::{anyhow, Context, Result};
use crate::local::share::find_secret;
use crate::store::{Contact, Store, StoredRun};
use orx_signals::Snapshot;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

/// Collection rewrites its time at most once a minute while nothing changes, so only a longer
/// silence means the laptop has not watched the run.
const STALE_AFTER_MS: i64 = 90_000;

const FINAL_LOG_GRACE_MS: i64 = 15_000;
const READING_FINAL_LOG: &str = "The final log is still being read.";

/// The monitor route's bound; larger series are paged.
const MAX_BODY: usize = 4 * 1024 * 1024;

const SENTINELS: [&str; 3] = ["NaN", "Infinity", "-Infinity"];
/// A stage on course to take this many times as long as the run that timed it is said to be slow.
const PACE_NOTED: f64 = 2.0;

fn interval(seconds: u64) -> i64 {
    crate::config::paced(Duration::from_secs(seconds)).as_millis() as i64
}

/// A run's checkpoint, or why it could not be read, and its collection status.
#[derive(Clone)]
struct Fold {
    snapshot: Option<Arc<Snapshot>>,
    error: Option<String>,
    /// Only what the projection reads of the status, so a rewrite that changes none of it is
    /// no change.
    status: Value,
}

type FileStamp = (u64, i128, u64);

fn stamp_of(path: &Path) -> Option<FileStamp> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i128);
    #[cfg(unix)]
    let inode = std::os::unix::fs::MetadataExt::ino(&meta);
    #[cfg(not(unix))]
    let inode = 0;
    Some((meta.len(), modified, inode))
}

fn signal_paths(data: &Path, run: &str) -> [PathBuf; 2] {
    let directory = data.join("run-signals");
    [
        directory.join(format!("{run}.json")),
        directory.join(format!("{run}.status.json")),
    ]
}

type Received = HashMap<String, (Option<FileStamp>, Arc<Snapshot>)>;

/// Checkpoints the laptop has just published, each until the projection takes it.
fn received() -> &'static Mutex<Received> {
    static RECEIVED: OnceLock<Mutex<Received>> = OnceLock::new();
    RECEIVED.get_or_init(Mutex::default)
}

/// Hands a checkpoint the laptop has just written at `path` to the projection, which then never
/// reads it back. Best effort: one not taken is read from the file.
pub(crate) fn hand_over(path: &Path, snapshot: &Snapshot) {
    let Some(run) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return;
    };
    let stamp = stamp_of(path);
    if let Ok(mut received) = received().lock() {
        // Runs the projection does not watch, such as in another data directory, are not kept.
        if received.len() >= 256 && !received.contains_key(run) {
            received.clear();
        }
        received.insert(run.to_owned(), (stamp, Arc::new(snapshot.clone())));
    }
}

#[cfg(test)]
thread_local! {
    static CHECKPOINT_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Breaks the next projection on this thread, as a bug would.
    static BREAK_NEXT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static LOGGED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A line for the developer's log about a run's monitor.
fn log(line: std::fmt::Arguments) {
    #[cfg(test)]
    LOGGED.with(|logged| logged.set(logged.get() + 1));
    eprintln!("orx up: {line}");
}

/// The run's checkpoint as it is at `stamp`: the one held, the one just handed over, or else read
/// from its file.
fn fold_at(data: &Path, run: &str, stamp: Option<FileStamp>, held: Option<&Entry>) -> Fold {
    let held = held
        .filter(|held| held.live && held.stamps[0] == stamp)
        .map(|held| (held.fold.snapshot.clone(), held.fold.error.clone()));
    // Taken whether or not it is used, so one handed over mid-projection is not kept.
    let handed = received()
        .lock()
        .ok()
        .and_then(|mut received| received.remove(run))
        .filter(|(at, _)| stamp.is_some() && *at == stamp)
        .map(|(_, snapshot)| snapshot);
    let (snapshot, error) = match held {
        Some(held) => held,
        None => match handed {
            Some(snapshot) => (Some(snapshot), None),
            None => {
                let [path, _] = signal_paths(data, run);
                #[cfg(test)]
                CHECKPOINT_READS
                    .with(|reads| reads.set(reads.get() + usize::from(stamp.is_some())));
                match super::adapters::local::signals::validate_run(run)
                    .and_then(|_| super::adapters::local::signals::load(&path))
                {
                    Ok(snapshot) => (snapshot.map(Arc::new), None),
                    Err(error) => (None, Some(format!("Read {}: {error}", path.display()))),
                }
            }
        },
    };
    let status = fold_status(data, run, snapshot.is_some(), error.clone());
    Fold {
        snapshot,
        error,
        status,
    }
}

/// What the projection reads of the run's collection status.
fn fold_status(data: &Path, run: &str, present: bool, error: Option<String>) -> Value {
    let status = cached_status(data, run, present, error);
    json!({
        "state": status["state"],
        "reason": status["reason"],
        "lastCollectedAt": status["lastCollectedAt"],
        "clock": status["clock"],
        "sample": status["sample"],
    })
}

/// What the store says of a run besides its signals, on the laptop's clock.
struct Lifecycle {
    run: StoredRun,
    phase: Option<Phase>,
    contact: Contact,
    started_at: Option<i64>,
    timeout_ms: Option<i64>,
    kind: String,
    /// What `orx runs` and `orx exp wait` print for this state too.
    state: String,
    retry_at: Option<i64>,
    slug: String,
    /// The experiment's run monitor settings as stored, the view's markup among them.
    settings: Option<String>,
    /// The parent experiment's run this one is compared with.
    parent: Option<StoredRun>,
    /// The experiment's own last run that finished before this one, which times its stages first.
    earlier: Option<StoredRun>,
    /// Why a failed run that recorded no message failed, as its log's last error says.
    log_error: Option<String>,
    /// The rules the run was launched with, or why they cannot be read.
    rules: std::result::Result<Vec<super::rules::Rule>, String>,
    /// Who stopped the run, once a cancel was accepted, or why that cannot be read.
    stop: std::result::Result<Option<crate::store::StopContext>, String>,
    /// The chat's agent is repairing the experiment after this run stopped.
    revising: bool,
    /// What the watcher said of the run, and whether and by what it is watched.
    watch: Value,
    /// The run's flags, oldest first, with what became of each.
    flags: Vec<Value>,
}

impl Lifecycle {
    fn read(store: &Store, row: &StoredRun) -> Result<Self> {
        let state = store
            .engine_run_state(&row.id)?
            .filter(|_| super::is_engine_row(row));
        let launch = state.as_ref().and_then(|state| state.launch.as_ref());
        let backend = crate::store::public_backend(&row.backend_json, launch).unwrap_or_default();
        let run = crate::plane::Run::from_store(store, row)?;
        let state_word = run.display_status().into_owned();
        let experiment = store.get_local_experiment(&row.experiment_id)?;
        let parent = match experiment
            .as_ref()
            .and_then(|exp| exp.parent_experiment_id.as_deref())
        {
            Some(parent) => parent_run(store, parent)?,
            None => None,
        };
        let slug = experiment.map_or_else(|| row.experiment_id.clone(), |exp| exp.slug);
        let earlier = store
            .list_runs_by_experiment(&row.experiment_id)?
            .into_iter()
            .filter(|run| run.created_at < row.created_at)
            .take(32)
            .find(|run| Phase::parse(&run.status) == Some(Phase::Done));
        let mut flags = super::flags::read(store, row)?;
        if let Some(flag) = flags
            .iter_mut()
            .rev()
            .find(|flag| !super::flags::settled(flag))
        {
            let look = flag["lookId"].as_str().and_then(|id| id.parse().ok());
            let rows = look
                .map(|id| store.watch_look_runs(id))
                .transpose()?
                .unwrap_or_default();
            if let Some(look) = rows.into_iter().find(|look| look.run_id == row.id) {
                flag["bindings"] = look.basis["bindings"].clone();
            }
        }
        Ok(Self {
            phase: Phase::parse(&row.status),
            contact: run.contact.clone(),
            started_at: state.as_ref().and_then(|state| state.started_at),
            timeout_ms: backend["timeoutSecs"]
                .as_i64()
                .map(|secs| secs.saturating_mul(1000)),
            kind: backend["kind"].as_str().unwrap_or_default().to_owned(),
            state: state_word,
            retry_at: store.signal_collection_retry_at(&row.id)?,
            slug,
            settings: store.run_monitor_json(&row.experiment_id)?,
            parent,
            earlier,
            log_error: run.log_error,
            rules: super::rules::frozen(&row.id, store.run_monitor_config(&row.id)?.as_deref())
                .map_err(|error| format!("{error:#}")),
            stop: store
                .run_stop(&row.id)
                .map_err(|error| format!("{error:#}")),
            revising: store.run_revising(row)?,
            watch: super::watch::run_facts(store, row).unwrap_or_else(|error| {
                json!({
                    "state": "unknown", "model": null, "looks": [], "spentUsd": null,
                    "error": {"code": "unread", "message": format!("Couldn't read what the watcher said: {error:#}")},
                    "sentence": null, "counts": {},
                })
            }),
            flags,
            run: row.clone(),
        })
    }

    fn live(&self) -> bool {
        !self.phase.is_some_and(Phase::is_terminal)
    }

    fn started(&self) -> bool {
        !matches!(self.phase, None | Some(Phase::Submitting | Phase::Starting))
    }

    fn contact_lost(&self) -> bool {
        matches!(
            self.contact,
            Contact::Unreachable { .. } | Contact::NeedsAuth { .. } | Contact::Unreconciled { .. }
        )
    }

    fn start(&self) -> i64 {
        self.started_at.unwrap_or(self.run.created_at)
    }

    fn end(&self) -> i64 {
        self.run.ended_at.unwrap_or(self.run.updated_at)
    }

    fn final_log_warning_due(&self, now: i64) -> bool {
        self.retry_at.is_some()
            && !self.live()
            && now.saturating_sub(self.end()) >= FINAL_LOG_GRACE_MS
    }

    fn ends_at(&self) -> Option<i64> {
        self.timeout_ms
            .filter(|_| self.started())
            .map(|limit| self.start() + limit)
    }

    /// Changes whenever anything the projection reads from the store does.
    fn key(&self) -> String {
        format!(
            "{}|{}|{:?}|{:?}|{}|{:?}|{}|{:?}|{:?}|{:?}|{:?}|{}|{}|{:?}",
            self.run.updated_at,
            self.state,
            self.contact,
            self.started_at,
            self.slug,
            self.retry_at,
            super::view::etag(self.settings.as_deref()),
            self.parent.as_ref().map(|p| (&p.id, p.updated_at)),
            self.earlier.as_ref().map(|p| (&p.id, p.updated_at)),
            self.log_error,
            self.stop,
            self.revising,
            self.watch,
            self.flags,
        )
    }
}

/// The parent experiment's run a run is compared with: its latest that finished with signals,
/// else its newest with signals, including a checkpoint not yet projected into a summary.
fn parent_run(store: &Store, experiment: &str) -> Result<Option<StoredRun>> {
    let mut newest = None;
    for run in store
        .list_runs_by_experiment(experiment)?
        .into_iter()
        .take(32)
    {
        let published = store
            .run_monitor_state(&run.id)?
            .and_then(|state| Published::read(&state["published"]));
        let bound = match published {
            Some(p) => p.summary["bindings"]
                .as_object()
                .is_some_and(|b| b.values().any(|v| !v.is_null())),
            None => checkpoint(store, &run.id)?.is_some_and(|s| s.has_signals()),
        };
        if !bound {
            continue;
        }
        if Phase::parse(&run.status) == Some(Phase::Done) {
            return Ok(Some(run));
        }
        newest.get_or_insert(run);
    }
    Ok(newest)
}

/// The summary's tone in each lifecycle phase.
fn tone(phase: Option<Phase>) -> &'static str {
    match phase {
        Some(Phase::Submitting | Phase::Starting) | None => "starting",
        Some(Phase::Running) => "running",
        Some(Phase::Done) => "done",
        Some(Phase::Failed | Phase::Lost) => "failed",
        Some(Phase::Cancelled) => "stopped",
    }
}

/// How fresh what the projection shows is, with the reason when it is not.
fn availability(fold: &Fold, life: &Lifecycle, now: i64) -> (&'static str, Option<String>) {
    let status = &fold.status;
    let reason = status["reason"].as_str().map(str::to_owned);
    let rebuilding = fold
        .snapshot
        .as_ref()
        .is_some_and(|s| s.coverage.reasons.iter().any(|r| r == "rebuilding"));
    match status["state"].as_str() {
        Some("unavailable") => ("unavailable", reason),
        Some(REFOLD_FAILED) => ("stale", reason),
        _ if rebuilding => (
            "rebuilding",
            Some("OpenResearch is reading the log again. The previous metrics are shown until it catches up.".into()),
        ),
        Some("stale") => ("stale", reason),
        Some("incomplete") => ("live", reason),
        _ if life.retry_at.is_some() => (
            "stale",
            life.final_log_warning_due(now).then(|| READING_FINAL_LOG.into()),
        ),
        _ => match status["lastCollectedAt"].as_i64() {
            Some(at) if life.live() && now - at > STALE_AFTER_MS => (
                "stale",
                Some(format!("No update from the run since {}.", clock_text(at))),
            ),
            _ => ("live", None),
        },
    }
}

/// The result notice checks collection when its chat is ready to receive it.
pub(crate) fn terminal_signal_note(store: &Store, run: &str) -> Result<Option<String>> {
    let status = super::adapters::signals::read_availability(store.data_dir(), run)?;
    if let Some(reason) = status.as_ref().and_then(|value| value["reason"].as_str()) {
        return Ok(Some(crate::store::RunNotice::terminal_signal_details(
            run, reason,
        )));
    }
    Ok(store
        .signal_collection_pending(run)?
        .then(|| READING_FINAL_LOG.to_owned()))
}

/// The default view, derived from the order in which the run first reported each signal.
pub(crate) fn default_view(snapshot: Option<&Snapshot>) -> Value {
    let Some(snapshot) = snapshot else {
        return json!({"signals": {}, "headline": [], "blocks": []});
    };
    let order = &snapshot.signal_order;
    let measured = |name: &&String| {
        snapshot.latest.get(*name).is_some_and(|value| {
            value.is_number() || value.as_str().is_some_and(|s| SENTINELS.contains(&s))
        })
    };
    // The stage line already says the counter: "round" under "Round 12 of 24" is no headline.
    let counter = snapshot
        .progress()
        .map(|phase| counted(phase.unit.as_deref().unwrap_or_default()));
    let headline = |names: &mut dyn Iterator<Item = &String>| {
        names
            .filter(|name| counter.as_deref() != Some(counted(name).as_str()))
            .find(|name| measured(name))
            .cloned()
    };
    // A row's columns live in its table; what the run printed on its own heads the view.
    let columns: BTreeSet<&String> = snapshot.row_columns.values().flatten().collect();
    let own: Vec<&String> = order
        .iter()
        .filter(|name| !columns.contains(name))
        .collect();
    let own_headline = headline(&mut own.iter().copied());
    let signals: serde_json::Map<String, Value> = order
        .iter()
        .map(|name| (name.clone(), json!({"label": name})))
        .collect();
    let mut blocks = Vec::new();
    if let Some(headline) = &own_headline {
        blocks.push(json!({"chart": [headline], "ghost": "parent"}));
        let first_seen = |columns: &Vec<String>| {
            columns
                .iter()
                .filter_map(|column| order.iter().position(|name| name == column))
                .min()
        };
        let mut tables: Vec<(&String, &Vec<String>)> = snapshot
            .row_columns
            .iter()
            .filter(|(name, _)| snapshot.rows.contains_key(*name))
            .collect();
        tables.sort_by_key(|(_, columns)| first_seen(columns));
        for (name, columns) in tables {
            blocks.push(json!({"table": name, "columns": columns}));
        }
        let stats: Vec<&String> = own.into_iter().filter(|name| *name != headline).collect();
        if !stats.is_empty() {
            blocks.push(json!({"stats": stats}));
        }
        return json!({"signals": signals, "headline": [headline], "blocks": blocks});
    }
    // A run that reports only rows leads with the table its first value came in.
    let headline = headline(&mut order.iter());
    let table = order.first().and_then(|first| {
        snapshot
            .row_columns
            .iter()
            .find(|(_, columns)| columns.contains(first))
    });
    if let Some((name, columns)) = table {
        blocks.push(json!({"table": name, "columns": columns}));
    } else if let Some(headline) = &headline {
        blocks.push(json!({"chart": [headline], "ghost": "parent"}));
    }
    let rest: Vec<&String> = order
        .iter()
        .filter(|name| Some(*name) != headline.as_ref())
        .collect();
    if !rest.is_empty() || (blocks.is_empty() && headline.is_some()) {
        let stats: Vec<&String> = if blocks.is_empty() {
            order.iter().collect()
        } else {
            rest
        };
        blocks.push(json!({"stats": stats}));
    }
    json!({"signals": signals, "headline": headline.into_iter().collect::<Vec<_>>(), "blocks": blocks})
}

/// The view a run is drawn with, where it comes from, and why its experiment's saved view could
/// not be read: then the default one is drawn.
fn view_of(life: &Lifecycle, snapshot: Option<&Snapshot>) -> (Value, Value, Option<String>) {
    let experiment = &life.run.experiment_id;
    let problem = match super::view::stored(experiment, life.settings.as_deref()) {
        Ok(None) => None,
        Ok(Some(markup)) => match super::view::compile(&markup) {
            Ok(compiled) => {
                let from = json!({"experimentId": experiment, "markup": markup, "isDefault": false});
                return (compiled.tree, from, None);
            }
            Err(problem) => Some(problem.text(
                &format!("of {experiment}"),
                &format!("The default view is shown. Run `orx exp view set {experiment} --file PATH` with a view this orx reads."),
            )),
        },
        Err(error) => Some(format!("{error:#}")),
    };
    let tree = default_view(snapshot);
    let from = json!({"experimentId": experiment, "markup": super::view::markup(&tree), "isDefault": true});
    (tree, from, problem)
}

/// How a signal is compared with its parent's run: at the same stage and step, or with the value
/// the parent ended with.
#[derive(Clone, Copy, PartialEq)]
enum Compare {
    Step,
    Final,
}

/// Compare each directed signal at its latest reported step in the matching stage, or at the end.
fn versus(
    view: &Value,
    snapshot: Option<&Snapshot>,
    parent: Option<(&Snapshot, bool)>,
    by: Compare,
) -> serde_json::Map<String, Value> {
    let (Some(snapshot), Some((parent, ended))) = (snapshot, parent) else {
        return serde_json::Map::new();
    };
    if by == Compare::Final && !ended {
        return serde_json::Map::new();
    }
    let finite = |value: Option<&Value>| value.and_then(Value::as_f64).filter(|v| v.is_finite());
    let mut versus = serde_json::Map::new();
    // A table's column holds its last row, which is no result to compare.
    let columns: BTreeSet<&String> = snapshot.row_columns.values().flatten().collect();
    for (name, spec) in view["signals"].as_object().into_iter().flatten() {
        let Some(better) = spec["better"].as_str().filter(|_| !columns.contains(name)) else {
            continue;
        };
        let Some(value) = finite(snapshot.latest.get(name)) else {
            continue;
        };
        let (theirs, step) = match by {
            Compare::Step => {
                if !snapshot.series_stages.contains_key(name)
                    || !parent.series_stages.contains_key(name)
                    || !same_stage(snapshot, parent, name)
                {
                    continue;
                }
                let Some(&(x, point)) = snapshot.series.get(name).and_then(|points| points.last())
                else {
                    continue;
                };
                let report = snapshot
                    .metric_reports
                    .get(name)
                    .and_then(|r| r.tail.back());
                if point != Some(value) || report.is_some_and(|report| report.x != Some(x)) {
                    continue;
                }
                let Some(theirs) = parent
                    .series
                    .get(name)
                    .and_then(|points| parent_at(points, x))
                else {
                    continue;
                };
                (theirs, Some(x))
            }
            Compare::Final => {
                let Some(theirs) = finite(parent.latest.get(name)) else {
                    continue;
                };
                (theirs, None)
            }
        };
        if !theirs.is_finite() {
            continue;
        }
        let d = value - theirs;
        let size = (d * spec["scale"].as_f64().unwrap_or(1.0)).abs();
        let mag = match spec["digits"].as_i64() {
            Some(digits) => {
                let unit = 10f64.powi(digits as i32);
                (size * unit).round() / unit
            }
            None => size,
        };
        let good = if better == "lower" { d < 0.0 } else { d > 0.0 };
        let mut compared =
            json!({"d": d, "good": good, "flat": mag == 0.0, "mag": mag, "parent": theirs});
        if let Some(step) = step {
            compared["step"] = json!(step);
        }
        versus.insert(name.clone(), compared);
    }
    versus
}

/// The parent's value at step `x` of its `points`, interpolated only between two steps it
/// reported; never extrapolated.
/// The stage a run's series on the step axis belongs to: its name, and how many stages of that
/// name came before it. A series with none yet counts in the stage the run reports steps in now.
pub(super) fn series_stage(run: &Snapshot, name: &str) -> Option<(String, usize)> {
    let at = match run.series_stages.get(name) {
        Some(occurrence) => run
            .phases
            .iter()
            .position(|p| p.occurrence == *occurrence)?,
        None => run.phases.iter().rposition(|p| p.done.is_some())?,
    };
    let stage = &run.phases[at].name;
    let before = run.phases[..at].iter().filter(|p| p.name == *stage).count();
    Some((stage.clone(), before))
}

/// The first counted stage or displayed point. An inner None means work preceded the retained stages.
fn first_work_stage(snapshot: &Snapshot, names: &[&str], planned: bool) -> Option<Option<usize>> {
    let point = names
        .iter()
        .filter_map(|name| snapshot.first_series_stages.get(*name).copied())
        .min();
    let count = snapshot.first_omitted_counted_stage.or_else(|| {
        snapshot
            .phases
            .iter()
            .find(|phase| {
                phase.has_reported_count()
                    || !planned && phase.source == "detected" && phase.done.is_some()
            })
            .map(|phase| phase.occurrence)
    });
    let first = point.into_iter().chain(count.map(Some)).min()?;
    Some(first.and_then(|occurrence| {
        snapshot
            .phases
            .iter()
            .position(|phase| phase.occurrence == occurrence)
    }))
}

/// Whether the parent's series `name` is of the same stage as the run's, so their steps can be
/// compared: the fold restarts a series at each stage, so a finished parent holds its last. A run
/// that reports no steps is placed by elapsed seconds, which never match another run's steps.
pub(super) fn same_stage(run: &Snapshot, parent: &Snapshot, name: &str) -> bool {
    let stepped = |snapshot: &Snapshot| snapshot.phases.iter().any(|p| p.done.is_some());
    let same_axis = stepped(run) == stepped(parent);

    same_axis
        && series_stage(run, name)
            .zip(series_stage(parent, name))
            .is_none_or(|(mine, theirs)| mine.0.is_empty() || theirs.0.is_empty() || mine == theirs)
}

pub(super) fn parent_at(points: &[orx_signals::Point], x: f64) -> Option<f64> {
    let after = points.iter().position(|point| point.0 >= x)?;
    let (x1, y1) = points[after];
    if x1 == x {
        return y1;
    }
    let (x0, y0) = points[after.checked_sub(1)?];
    let (y0, y1) = (y0?, y1?);
    Some(y0 + (y1 - y0) * (x - x0) / (x1 - x0))
}

/// The signal a run leads with. A value that went wrong is never hidden by the view's order: the
/// first non-finite one, else the first that fell to exactly zero where the parent's did not, among
/// the view's headline and then its other signals. Otherwise the first headline with a number,
/// else the first.
fn headline<'a>(
    view: &'a Value,
    latest: &BTreeMap<String, Value>,
    versus: &serde_json::Map<String, Value>,
) -> Option<&'a str> {
    let named: Vec<&str> = view["headline"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let others = view["signals"]
        .as_object()
        .into_iter()
        .flat_map(|signals| signals.keys())
        .map(String::as_str)
        .filter(|name| !named.contains(name));
    let candidates: Vec<&str> = named.iter().copied().chain(others).collect();
    let non_finite = |name: &&str| {
        latest
            .get(*name)
            .and_then(Value::as_str)
            .is_some_and(|value| SENTINELS.contains(&value))
    };
    let collapsed = |name: &&str| {
        latest.get(*name).and_then(Value::as_f64) == Some(0.0)
            && versus
                .get(*name)
                .and_then(|vs| vs["parent"].as_f64())
                .is_some_and(|parent| parent != 0.0)
    };
    candidates
        .iter()
        .copied()
        .find(non_finite)
        .or_else(|| candidates.iter().copied().find(collapsed))
        .or_else(|| {
            named
                .iter()
                .copied()
                .find(|name| latest.get(*name).is_some_and(Value::is_number))
        })
        .or(named.first().copied())
}

fn periodic_pause(run: &Snapshot, history: &Snapshot, silence: f64) -> bool {
    let Some(stage) = run.phases.last() else {
        return false;
    };
    let Some(done) = stage.done else { return false };
    let occurrence = run.phases.iter().filter(|p| p.name == stage.name).count() - 1;
    let Some(previous) = history
        .phases
        .iter()
        .filter(|p| p.name == stage.name)
        .nth(occurrence)
    else {
        return false;
    };
    let mut points: Vec<&(usize, f64, f64)> = Vec::new();
    for point in history
        .timing
        .progress_points
        .iter()
        .filter(|point| point.0 == previous.occurrence)
    {
        if points.last().is_none_or(|last| point.2 > last.2) {
            points.push(point);
        }
    }
    let mut gaps: Vec<_> = points
        .windows(2)
        .filter_map(|pair| {
            let gap = pair[1].1 - pair[0].1;
            (gap > 0.0 && pair[1].2 > pair[0].2).then_some(gap / (pair[1].2 - pair[0].2))
        })
        .collect();
    if gaps.len() < 4 {
        return false;
    }
    gaps.sort_by(f64::total_cmp);
    let typical = gaps[gaps.len() / 2];
    let pauses: Vec<_> = points
        .windows(2)
        .filter_map(|pair| {
            let step = pair[0].2;
            let gap = pair[1].1 - pair[0].1;
            (step > 0.0 && pair[1].2 > step && gap > 4.0 * typical * (pair[1].2 - step))
                .then_some((step, gap))
        })
        .collect();
    pauses.iter().enumerate().any(|(index, &(step, gap))| {
        let interval = pauses.get(index + 1).map_or(step, |next| next.0 - step);
        // A known pause earns at most twice its observed duration at its next interval.
        interval > 0.0
            && done >= interval
            && (done % interval).abs() < 1e-6
            && silence <= 2_000.0 * gap
    })
}

/// The estimator's inputs for this run, and the offset that maps its host times to the laptop.
fn estimate_input(
    fold: &Fold,
    life: &Lifecycle,
    stages: &[estimate::Stage],
    now: i64,
) -> (estimate::Input, i64) {
    let clock: Option<(i64, i64)> = serde_json::from_value(fold.status["clock"].clone()).ok();
    let offset = clock.map_or(0, |(host, laptop)| host - laptop);
    let snapshot = fold.snapshot.as_deref();
    let run = estimate::Run {
        started_at: life.start() as f64,
        contact_lost: life.contact_lost(),
        ends_at: life.ends_at().map(|at| at as f64),
        machine_cost_unknown: matches!(
            life.kind.as_str(),
            "openresearch_job" | "hf_job" | "modal_job" | "vercel_job" | "boxd_job"
        ),
    };
    let empty;
    let signals = match snapshot {
        Some(snapshot) => snapshot,
        None => {
            empty = Snapshot::new(&life.run.id, "", "");
            &empty
        }
    };
    let clock = clock.map(|(host, laptop)| (host as f64, laptop as f64));
    let mut input = estimate::with_signals(run, signals, stages, clock, now as f64);
    if snapshot.is_none() {
        // Nothing reported: the run has been going since it started, on the laptop's clock.
        input.as_of = now as f64;
        input.now = now as f64;
    }
    // An ended run's last signals are often folded after it ended; it is evaluated at its end.
    input.as_of = input.as_of.min(input.now);
    (input, offset)
}

/// One evaluation: the summary and the monitor, without the numbers that publishing assigns.
struct Evaluated {
    summary: Value,
    body: Value,
    /// Slices of progress the estimate timed.
    slices: usize,
}

/// `parent` and `earlier` are the checkpoints of the runs `life` names so.
fn evaluate(
    fold: &Fold,
    life: &Lifecycle,
    parent: Option<&Snapshot>,
    earlier: Option<&Snapshot>,
    now: i64,
) -> Evaluated {
    let snapshot = fold.snapshot.as_deref();
    let (state, reason) = availability(fold, life, now);
    let last_collected = fold.status["lastCollectedAt"].as_i64();
    // Time the laptop did not watch is unknown: nothing advances past the last collection.
    let at = if !life.live() {
        life.end()
    } else if state == "live" {
        now
    } else {
        last_collected.unwrap_or(now).min(now)
    };
    // When the data last changed or was last confirmed, so a quiet run publishes nothing new.
    let known = last_collected.unwrap_or(0).max(life.run.updated_at);
    let as_of = if life.live() {
        known.min(at)
    } else {
        known.max(at)
    };
    // The run's own plan, when it declared one, else its experiment's.
    let (configured, plan_problem) =
        match super::view::stored_plan(&life.run.experiment_id, life.settings.as_deref()) {
            Ok(plan) => (plan, None),
            Err(error) => (Vec::new(), Some(format!("{error:#}"))),
        };
    let plan = snapshot
        .and_then(|s| s.plan.as_deref())
        .unwrap_or(&configured);
    let planned = !plan.is_empty();
    // A plan that counts work the run reports only as rows: its bar has no step to move by.
    let disagrees = snapshot
        .filter(|s| planned && !s.phases.iter().any(|phase| phase.done.is_some()))
        .and_then(|s| {
            let (table, rows) = s.rows.iter().max_by_key(|(_, rows)| rows.len())?;
            let stage = plan.iter().find(|stage| stage.of.is_some())?;
            Some(json!({
                "stage": stage.phase, "of": stage.of, "unit": stage.unit,
                "table": table, "rows": rows.len(),
            }))
        });
    let parent_done = life
        .parent
        .as_ref()
        .is_some_and(|p| Phase::parse(&p.status) == Some(Phase::Done));
    let timed: Vec<_> = earlier
        .map(|run| (run, true, estimate::TimedBy::Earlier))
        .into_iter()
        .chain(parent.map(|run| (run, parent_done, estimate::TimedBy::Parent)))
        .collect();
    let stages = estimate::stages(plan, snapshot, life.phase == Some(Phase::Done), &timed);
    let (input, offset) = estimate_input(fold, life, &stages, at);
    let laptop = |host: f64| (host - offset as f64).round() as i64;
    let output = estimate::estimate(&input);
    let reasons: Vec<String> = match &output {
        Ok(output) => output.errors.iter().map(ToString::to_string).collect(),
        Err(error) => vec![error.to_string()],
    };
    let mut output = output.ok();
    if let (Some(snapshot), Some(output)) = (snapshot, output.as_mut()) {
        if output.stall_ms.is_some_and(|silence| {
            silence > 0.0
                && std::iter::once(snapshot)
                    .chain(parent)
                    .any(|history| periodic_pause(snapshot, history, silence))
        }) {
            output.stall_ms = Some(0.0);
        }
    }
    let started = life.started();
    let last_phase = snapshot.and_then(|s| s.phases.len().checked_sub(1));
    // The stage the run is in, or with a plan its first before any began.
    let current = stages
        .iter()
        .position(|stage| stage.phase.is_some() && stage.phase == last_phase);
    // How many times as long a step of this stage takes as one of the run that timed it, once
    // four slices of its own pace are measured.
    let pace = (|| {
        let prior = stages.get(current?)?.prior?;
        output.as_ref().filter(|o| o.completed_slices >= 4)?;
        let (first, last) = (input.points.first()?, input.points.last()?);
        let mine = (last.at - first.at) / (last.done - first.done);
        let theirs = prior.per_unit_ms?;
        let times = mine / theirs;
        (life.live() && times.is_finite() && times >= PACE_NOTED).then(|| {
            let than = match prior.by {
                estimate::TimedBy::Earlier => "earlier",
                estimate::TimedBy::Parent => "parent",
            };
            json!({"times": (times * 2.0).round() / 2.0, "than": than})
        })
    })();

    // A run that finished did all it had to, however much that was.
    let done = life.phase == Some(Phase::Done);
    let finished = done && snapshot.and_then(Snapshot::progress).is_some();
    // Every stage of a plan that finished is done; the ones it skipped are not segments.
    let planned_done = done && planned;
    let finished = |fill: Option<f64>| {
        if planned_done {
            Some(1.0)
        } else {
            fill.or(finished.then_some(1.0))
        }
    };
    let segments: Vec<Value> = output.as_ref().map_or_else(Vec::new, |o| {
        if o.whole.is_none() {
            let fill = snapshot
                .and_then(Snapshot::progress)
                .and_then(|phase| Some(phase.done? / phase.of.filter(|of| *of > 0.0)?))
                .map(|fill| fill.clamp(0.0, 1.0))
                .or(o.phase_fraction);

            return vec![json!({"weight": 1.0, "fill": finished(fill)})];
        }
        o.segments
            .iter()
            .map(|s| json!({"weight": s.weight, "fill": finished(s.fill)}))
            .collect()
    });
    let progressed = snapshot.and_then(Snapshot::progress);
    let progress = if !planned {
        progressed.map(|phase| (last_phase.unwrap_or(0), phase.done))
    } else {
        Some((
            current.unwrap_or(0),
            current.and(snapshot.and_then(|s| s.phases.last()?.done)),
        ))
    };
    let progress = progress.map(|(index, done)| {
        let last_at = input
            .points
            .last()
            .filter(|_| input.placed)
            .map(|p| laptop(p.at));
        json!({
            "phase": index,
            "done": done,
            "segments": segments,
            "whole": finished(output.as_ref().and_then(|o| o.whole)),
            "lastAt": last_at,
            "stallMs": output.as_ref().and_then(|o| o.stall_ms).map(|ms| ms.round() as i64),
        })
    });
    let whole = finished(output.as_ref().and_then(|o| o.whole)).filter(|_| started);
    // A bar or counter read from the log times only itself: more work may follow it, unless a
    // declared plan says what does.
    let floor = !planned
        && snapshot.is_some_and(|s| {
            s.progress().is_some_and(|phase| {
                !phase.name.is_empty() || matches!(phase.source.as_str(), "detected" | "pattern")
            }) && !s.several_bars()
        });
    let estimating = || {
        json!({
            "loMs": null, "midMs": null, "hiMs": null, "at": at, "pending": true,
            "pastLimitMs": null, "floor": floor,
        })
    };
    let estimate = match output.as_ref() {
        Some(output) => output
            .estimate
            .as_ref()
            .map(|eta| {
                let range = eta.range;
                let mut estimate = json!({
                    "loMs": range.map(|r| r.lo_ms.round() as i64),
                    "midMs": range.map(|r| r.mid_ms.round() as i64),
                    "hiMs": range.map(|r| r.hi_ms.round() as i64),
                    "at": laptop(eta.at),
                    "pending": eta.pending,
                    "pastLimitMs": eta.past_limit_ms.map(|ms| ms.round() as i64),
                    "floor": floor || eta.floor,
                });
                if eta.floor {
                    // The stages, this one or to come, that no earlier run timed.
                    let from = current.unwrap_or(0);
                    estimate["untimed"] = stages
                        .iter()
                        .enumerate()
                        .filter(|(index, stage)| {
                            !stage.skipped
                                && stage.prior.is_none()
                                && (*index > from || *index == from && stage.of.is_none())
                        })
                        .map(|(_, stage)| json!(stage.name))
                        .collect();
                }
                estimate
            })
            ,
        // Timing the estimator cannot read leaves the estimate pending, saying why.
        None => Some(estimating()),
    };
    let estimate = estimate.filter(|_| started && life.live());
    let rules = life.rules.as_deref().unwrap_or_default();
    let budget = rules.iter().find_map(|rule| match rule.when {
        super::rules::When::CostAbove { usd } => Some(usd),
        _ => None,
    });
    let cost = output.as_ref().map(|o| &o.cost).and_then(|cost| {
        (input.machine_cost_unknown || cost.spend.is_some()).then(|| {
            json!({
                "soFar": cost.so_far,
                "projected": cost.projected,
                "spend": cost.spend,
                "budget": budget,
            })
        })
    });
    let limit = life.ends_at().map(|ends_at| json!({"endsAt": ends_at}));
    let phases: Vec<Value> = if planned {
        let occurrences = snapshot.map_or(&[][..], |s| &s.phases);
        // A finished stage's end is when the next began; the last one of an ended run, its log's.
        let log_end = snapshot
            .and_then(|s| s.timing.last_log_at)
            .filter(|_| !life.live());
        stages
            .iter()
            .map(|stage| {
                let occurrence = stage.phase.and_then(|index| occurrences.get(index));
                let last = stage.phase.is_some() && stage.phase == last_phase;
                let ended = occurrence
                    .and_then(|o| o.ended_at)
                    .or(log_end.filter(|_| last));
                let mut entry = json!({
                    "name": stage.name,
                    "of": stage.of,
                    "unit": stage.unit,
                    "priorMs": stage.prior.map(|prior| prior.ms.round() as i64),
                    "startedAt": occurrence.and_then(|o| o.started_at).map(|ms| ms - offset),
                    "endedAt": ended.map(|ms| ms - offset),
                    "planned": stage.planned,
                });
                if stage.skipped {
                    entry["skipped"] = json!(true);
                }
                entry
            })
            .collect()
    } else {
        snapshot.map_or_else(Vec::new, |s| {
            s.phases
                .iter()
                .zip(&stages)
                .map(|(p, stage)| {
                    json!({
                        "name": p.name,
                        "of": p.of,
                        "unit": p.unit,
                        "priorMs": stage.prior.map(|prior| prior.ms.round() as i64),
                        "startedAt": p.started_at.map(|ms| ms - offset),
                        "endedAt": p.ended_at.map(|ms| ms - offset),
                    })
                })
                .collect()
        })
    };
    let (view, view_from, view_problem) = view_of(life, snapshot);
    let parent_ended = life
        .parent
        .as_ref()
        .is_some_and(|p| Phase::parse(&p.status).is_some_and(Phase::is_terminal));
    let compared = parent.map(|parent| (parent, parent_ended));
    let at_step = versus(&view, snapshot, compared, Compare::Step);
    let at_end = if life.live() {
        serde_json::Map::new()
    } else {
        versus(&view, snapshot, compared, Compare::Final)
    };
    // Each signal compares its own way, else at the same step while the run goes and with what the
    // parent ended with once it ended.
    let versus: serde_json::Map<String, Value> = view["signals"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, spec)| {
            let step = match spec["vs"].as_str() {
                Some(vs) => vs == "step",
                None => life.live(),
            };
            let by = if step { &at_step } else { &at_end };
            by.get(name).map(|vs| (name.clone(), vs.clone()))
        })
        .collect();
    let stepped = snapshot.is_some_and(|s| s.phases.iter().any(|p| p.done.is_some()));
    let x_name = if stepped { "step" } else { "elapsed" };
    let x_max = snapshot
        .and_then(Snapshot::progress)
        .and_then(|p| p.of)
        .or(current.and_then(|index| stages[index].of))
        .filter(|_| stepped);
    let latest = snapshot.map_or_else(BTreeMap::new, |s| s.latest.clone());
    let blocks = view["blocks"].as_array().map_or(&[][..], Vec::as_slice);
    let tables = |snapshot: Option<&Snapshot>, wanted: &dyn Fn(&Value) -> bool| {
        blocks
            .iter()
            .filter(|block| wanted(block))
            .filter_map(|block| {
                block["table"]
                    .as_str()
                    .or(block["from"].as_str())
                    .or(block["cells"].as_str())
            })
            .filter_map(|name| snapshot.and_then(|s| s.rows.get_key_value(name)))
            .map(|(name, rows)| (name.clone(), json!(rows)))
            .collect::<serde_json::Map<String, Value>>()
    };
    let rows = tables(snapshot, &|_| true);
    let mut groups = BTreeMap::<&str, BTreeMap<&str, Value>>::new();
    for block in blocks {
        if let (Some(table), Some(by)) = (block["bars"].as_str(), block["by"].as_str()) {
            groups
                .entry(table)
                .or_default()
                .entry(by)
                .or_insert_with(|| {
                    let rows = snapshot
                        .and_then(|s| s.rows.get(table))
                        .map_or(&[][..], Vec::as_slice);
                    outcome_groups(rows, by)
                });
        }
    }
    let parent_body = life.parent.as_ref().zip(parent).map(|(run, parent)| {
        let phase = snapshot
            .and_then(|s| s.phases.last())
            .map(|p| p.name.as_str())
            .filter(|name| !name.is_empty() && parent.phases.iter().any(|p| p.name == *name));
        let ghosts: Vec<&str> = blocks
            .iter()
            .filter(|block| block["ghost"] == "parent" && block.get("from").is_none())
            .flat_map(|block| block["chart"].as_array().into_iter().flatten())
            .filter_map(Value::as_str)
            .collect();
        // On the step axis a line is one stage's: the parent's goes under the child's of the same
        // stage, and under any while either has none to say.
        let shown =
            |name: &str| x_name != "step" || snapshot.is_none_or(|s| same_stage(s, parent, name));
        let series: serde_json::Map<String, Value> = parent
            .series
            .iter()
            .filter(|(name, _)| ghosts.contains(&name.as_str()) && shown(name))
            .map(|(name, points)| (name.clone(), json!(points)))
            .collect();
        let rows = tables(Some(parent), &|block| {
            block.get("table").is_some() && block.get("vs").is_some()
                || block["ghost"] == "parent" && block.get("from").is_some()
        });
        json!({"runId": run.id, "phase": phase, "series": series, "rows": rows})
    });
    let best: serde_json::Map<String, Value> = view["signals"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, spec)| spec["pick"] == "best")
        .filter_map(|(name, spec)| {
            let snapshot = snapshot?;
            let values = snapshot
                .series
                .get(name)
                .into_iter()
                .flatten()
                .filter_map(|point| point.1)
                .chain(snapshot.latest.get(name).and_then(Value::as_f64))
                .filter(|value| value.is_finite());
            let best = if spec["better"] == "lower" {
                values.min_by(f64::total_cmp)
            } else {
                values.max_by(f64::total_cmp)
            };
            best.map(|best| (name.clone(), json!(best)))
        })
        .collect();
    let source = snapshot.and_then(|s| {
        s.phases
            .last()
            .map(|p| p.source.clone())
            .or_else(|| (!s.latest.is_empty()).then(|| "declared".to_owned()))
    });
    let generation = snapshot.map_or("", |s| s.generation.as_str());
    let prior_view = (view_from["isDefault"] == true && blocks.is_empty())
        .then(|| default_view(earlier.or(parent)));
    let timing_view = prior_view.as_ref().unwrap_or(&view);
    let planned_signal_blocks = blocks.iter().filter_map(|block| {
        let phase = block["phase"].as_str()?;
        let chart = block["chart"].is_array() && block.get("from").is_none();
        let stats = block["stats"].is_array();
        (chart || stats).then_some((phase, block))
    });
    let displayed: Vec<&str> = timing_view["headline"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .chain(
            timing_view["blocks"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|block| {
                    block["chart"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .chain(
                            block["series"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|series| series["y"].as_str()),
                        )
                }),
        )
        .chain(
            planned_signal_blocks
                .clone()
                .filter(|(phase, _)| plan.iter().any(|stage| stage.phase == *phase))
                .flat_map(|(_, block)| block["stats"].as_array().into_iter().flatten())
                .filter_map(Value::as_str),
        )
        .collect();
    let first_work = snapshot.and_then(|snapshot| first_work_stage(snapshot, &displayed, planned));
    let first_observed = first_work.and_then(|phase| match phase {
        None => Some(0),
        Some(phase) => stages.iter().position(|stage| stage.phase == Some(phase)),
    });
    let has_work = first_work.is_some();
    let matching_stage = |prior: &Snapshot, index: usize| {
        let name = &prior.phases[index].name;
        let occurrence = prior.phases[..index]
            .iter()
            .filter(|phase| phase.name == *name)
            .count();
        stages
            .iter()
            .enumerate()
            .filter(|(_, stage)| stage.name == *name)
            .nth(occurrence)
            .map(|(index, _)| index)
    };
    let guess = if life.live() {
        let historical = timed.iter().find_map(|(prior, _, _)| {
            let Some(first) = first_work_stage(prior, &displayed, planned)? else {
                return Some(0);
            };
            matching_stage(prior, first)
        });
        let viewed = stages.iter().position(|stage| {
            stage.planned
                && !stage.skipped
                && planned_signal_blocks
                    .clone()
                    .any(|(phase, _)| stage.name == phase)
        });
        historical.into_iter().chain(viewed).min().map(|first| {
            if planned {
                first.max(current.unwrap_or(0))
            } else {
                first
            }
        })
    } else {
        None
    };
    let setup = (started && life.live() && fold.error.is_none() && !has_work).then(|| {
        let line = snapshot
            .map(|s| s.last_line.as_str())
            .filter(|line| !line.is_empty() && find_secret(line).is_none());
        let stage = snapshot
            .and_then(|s| s.phases.last())
            .map(|phase| phase.name.as_str())
            .filter(|name| !name.is_empty());
        let current_end = || {
            let Some(current) = current else {
                return Some(life.start() as f64);
            };
            if let Some(eta) = output.as_ref().and_then(|o| o.current_phase.as_ref()) {
                if let Some(range) = eta.range {
                    return Some(laptop(eta.at) as f64 + range.mid_ms);
                }
            }
            let prior = stages[current].prior?;
            let end = if let Some(point) = input.points.last() {
                let total = input.phases[input.current].total?;
                let remaining = (1.0 - point.done / total).clamp(0.0, 1.0);
                let anchor = if input.placed { laptop(point.at) } else { at };
                anchor as f64 + prior.ms * remaining
            } else {
                laptop(snapshot?.phases.last()?.started_at? as f64) as f64 + prior.ms
            };
            (end > at as f64).then_some(end)
        };
        let starts_at = if planned {
            guess.and_then(|first| {
                if current == Some(first) {
                    let began = snapshot?.phases.last()?.started_at;
                    return Some(began.map_or(at, |ms| laptop(ms as f64)) as f64);
                }
                let from = current.map_or(0, |current| current + 1);
                let future: Option<f64> = stages[from..first]
                    .iter()
                    .map(|stage| {
                        if stage.skipped {
                            Some(0.0)
                        } else {
                            stage.prior.map(|prior| prior.ms)
                        }
                    })
                    .sum();
                Some(current_end()? + future?)
            })
        } else {
            // Without a plan, only the earlier run knows which stages lie ahead.
            timed.iter().find_map(|(prior, _, _)| {
                let first = first_work_stage(prior, &displayed, planned)?.unwrap_or(0);
                let from = if let Some(current) = current {
                    let index = (0..prior.phases.len())
                        .find(|index| matching_stage(prior, *index) == Some(current))?;
                    if index >= first {
                        return Some(at as f64);
                    }
                    index + 1
                } else {
                    0
                };
                let future: Option<f64> = prior.phases[..first]
                    .iter()
                    .enumerate()
                    .skip(from)
                    .map(|(index, phase)| {
                        matching_stage(prior, index)
                            .and_then(|index| stages[index].prior.map(|prior| prior.ms))
                            .or_else(|| Some((phase.ended_at? - phase.started_at?) as f64))
                    })
                    .sum();
                Some(current_end()? + future?)
            })
        };
        let starts_at = starts_at
            .filter(|starts_at| starts_at.is_finite())
            .map(|starts_at| starts_at.round() as i64);
        json!({ "line": line, "stage": stage, "startsAt": starts_at })
    });

    let first_data = first_observed.or(guess);
    let stage_order = json!({
        "index": current,
        "count": planned.then_some(stages.len()),
        "first": first_data,
    });

    let headline_name = headline(&view, &latest, &versus);
    let headline = headline_name.map(|name| {
        json!({
            "name": name,
            "label": view["signals"][name]["label"].as_str().unwrap_or(name),
            "value": latest.get(name),
        })
    });
    let mut referenced: serde_json::Map<String, Value> = view["signals"]
        .as_object()
        .into_iter()
        .flat_map(|signals| signals.keys())
        .filter_map(|name| latest.get(name).map(|value| (name.clone(), value.clone())))
        .collect();
    for (name, spec) in view["signals"].as_object().into_iter().flatten() {
        if spec.get("better").is_some() {
            let comparison = versus.get(name).cloned().unwrap_or(Value::Null);
            referenced.insert(format!("{name}.vs"), comparison);
        }
    }
    let counts = life.watch["counts"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    for (pattern, count) in &counts {
        referenced.insert(
            format!("count:{}", Value::from(pattern.as_str())),
            count.clone(),
        );
    }
    let open = super::flags::open(&life.flags);
    let mut watch = life.watch.clone();
    let flag_look = open.and_then(|flag| flag["lookId"].as_str());
    let sentence_look = watch["sentence"]["lookId"].as_str();
    if has_work && watch["sentence"]["setup"] == true
        || flag_look.is_some() && flag_look == sentence_look
    {
        watch["sentence"] = Value::Null;
    }
    let sentence = watch["sentence"].clone();
    let summary_estimate = estimate
        .as_ref()
        .and_then(Value::as_object)
        .map(|estimate| {
            let mut estimate = estimate.clone();
            estimate.remove("pastLimitMs");
            estimate
        });
    // A flag no one settled waits after the run ends too; it colours a run that finished.
    let flagged = open.is_some() && (life.live() || life.phase == Some(Phase::Done));
    let mut summary = json!({
        "tone": match (life.revising, flagged) {
            (true, _) => "revising",
            (false, true) => "flag",
            (false, false) => tone(life.phase),
        },
        "whole": whole,
        "stageOrder": stage_order,
        "stage": current.map(|index| {
            let stage = &stages[index];
            json!({"name": stage.name, "done": progressed.and_then(|phase| phase.done), "of": stage.of, "unit": stage.unit})
        }),
        "headline": headline,
        "logError": life.log_error,
        "sentence": sentence,
        "bindings": referenced,
        "signals": view["signals"],
        "estimate": summary_estimate,
        "limitEndsAt": life.ends_at(),
        "flag": open.map(|flag| {
            let mut summary = json!({
                "id": flag["id"], "state": flag["state"], "template": flag["template"],
                "text": super::flags::sentence(flag), "bindings": flag["bindings"],
            });
            if let Some(handback) = flag.get("handback") {
                summary["handback"] = handback.clone();
            }
            summary
        }),

        "setup": setup,
        "generation": generation,
        "asOf": as_of,
    });
    if matches!(state, "stale" | "rebuilding") {
        summary["availability"] = json!({"state": state, "reason": reason});
    } else if state == "unavailable" {
        let message = if fold.error.is_some() {
            SIGNALS_UNREADABLE
        } else {
            reason.as_deref().unwrap_or(SIGNALS_UNREADABLE)
        };
        summary["unavailable"] = json!(message);
    }
    // A plan's stages, so a line's underline is segmented without the full monitor.
    if started && (planned && stages.len() > 1 || whole.is_none()) {
        summary["segments"] = segments
            .iter()
            .map(|s| json!({"weight": s["weight"], "fill": s["fill"]}))
            .collect();
    }
    let text = status_text(
        life,
        fold,
        &watch,
        &StatusFacts {
            signals: &view["signals"],
            whole,
            estimate: estimate.as_ref(),
            estimate_reasons: &reasons,
            cost: cost.as_ref(),
            stall_ms: progress.as_ref().and_then(|p| p["stallMs"].as_i64()),
            degraded: state != "live",
            at,
            versus: &versus,
            view_problem: view_problem.as_deref().or(plan_problem.as_deref()),
            disagrees: disagrees.as_ref(),
            stages: planned.then(|| (&phases[..], current)),
            stage: &summary["stage"],
            stage_position: current
                .zip(first_observed)
                .filter(|(index, first)| planned && life.live() && index >= first)
                .map(|(index, first)| (index - first + 2, phases.len() - first + 1)),
            pace: pace.as_ref(),
            setup: setup.as_ref(),
            bindings: &summary["bindings"],
        },
    );
    let mut body = json!({
        "runId": life.run.id,
        "generation": generation,
        "asOf": as_of,
        "availability": {
            "state": state,
            "reason": reason,
            "omitted": snapshot.map_or_else(BTreeMap::new, |s| s.omitted.clone()),
        },
        "view": view,
        "viewFrom": view_from,
        "source": source,
        "phases": phases,
        "stageOrder": stage_order,
        "progress": progress,
        "x": {"name": x_name, "max": x_max},
        "latest": latest,
        "parent": parent_body,
        "headline": headline_name,
        "logError": life.log_error,
        "pace": pace,
        "versus": {"step": at_step, "final": at_end},
        "rows": rows,
        "groups": groups,
        "counts": counts,
        "estimate": estimate,
        "limit": limit,
        "cost": cost,
        "watch": watch_body(&watch),
        "sentence": sentence,
        "flagId": open.map(|flag| &flag["id"]),
        "flags": life.flags,
        "setup": setup,
        "disagrees": disagrees,
        "revising": life.revising,
        "brief": brief(life.settings.as_deref()),
        "rules": rules.iter().map(super::rules::Rule::shown).collect::<Vec<_>>(),
        "statusText": text,
    });
    if !life.live() && fold.status["state"] == "incomplete" {
        body["availability"]["collectionIncomplete"] = json!(true);
    }
    if !snapshot.is_some_and(|s| s.phases.iter().any(|p| !p.name.is_empty())) {
        if let Some(blocks) = body["view"]["blocks"].as_array_mut() {
            for block in blocks.iter_mut().filter_map(Value::as_object_mut) {
                block.remove("phase");
            }
        }
    }
    if !best.is_empty() {
        body["best"] = json!(best);
    }
    Evaluated {
        summary,
        body,
        slices: output.as_ref().map_or(0, |o| o.completed_slices),
    }
}

/// What the experiment's config tells the watcher, label by label in the brief's own order.
fn brief(settings: Option<&str>) -> Value {
    let settings: Value = settings
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or(Value::Null);
    super::view::BRIEF_LABELS
        .iter()
        .filter_map(|label| {
            let text = settings["brief"][label].as_str()?.trim();
            (!text.is_empty()).then(|| json!({"label": label, "text": text}))
        })
        .collect()
}

/// The watch as the page reads it: everything but the sentence and the counts, which it reads
/// beside it.
fn watch_body(watch: &Value) -> Value {
    let mut body = watch.clone();
    if let Some(body) = body.as_object_mut() {
        body.remove("sentence");
        body.remove("counts");
    }
    body
}

/// The revisions, lifecycle phase and summary last published, which list an ended run
/// without reading its checkpoint.
#[derive(Clone)]
struct Published {
    generation: String,
    phase: Option<Phase>,
    fold_rev: u64,
    rev: u64,
    series_rev: u64,
    hash: String,
    summary: Value,
}

impl Published {
    /// Whatever of a pointer can be read; the counters only grow from what was stored.
    fn read(value: &Value) -> Option<Self> {
        let summary = &value["summary"];
        value.is_object().then(|| Self {
            generation: summary["generation"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            phase: value["phase"].as_str().and_then(Phase::parse),
            fold_rev: value["foldRev"].as_u64().unwrap_or_default(),
            rev: summary["rev"].as_u64().unwrap_or_default(),
            series_rev: value["seriesRev"].as_u64().unwrap_or_default(),
            hash: value["hash"].as_str().unwrap_or_default().to_owned(),
            summary: summary.clone(),
        })
    }

    fn json(&self) -> Value {
        json!({
            "phase": self.phase.map(Phase::as_str),
            "foldRev": self.fold_rev,
            "seriesRev": self.series_rev,
            "hash": self.hash,
            "summary": self.summary,
        })
    }
}

/// A run's monitor at one `rev`.
pub(crate) struct Projection {
    summary: Value,
    /// Null for an ended run listed from its stored summary, until it is opened.
    body: Value,
    snapshot: Option<Arc<Snapshot>>,
    published: Published,
    slices: usize,
}

impl Projection {
    pub(crate) fn summary(&self) -> &Value {
        &self.summary
    }

    /// The checkpoint the projection read.
    pub(crate) fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_deref()
    }

    pub(crate) fn etag(&self) -> String {
        format!("\"{}:{}\"", self.published.generation, self.published.rev)
    }

    /// The whole monitor, with every series point the checkpoint keeps.
    pub(crate) fn monitor(&self) -> Value {
        let mut monitor = self.body.clone();
        monitor["series"] = series_json(self.snapshot.as_deref(), None);
        monitor
    }
}

fn series_json(snapshot: Option<&Snapshot>, from: Option<&BTreeMap<String, usize>>) -> Value {
    let Some(snapshot) = snapshot else {
        return json!({});
    };
    let series: serde_json::Map<String, Value> = snapshot
        .series
        .iter()
        .filter_map(|(name, points)| {
            let start = from.map_or(0, |lens| lens.get(name).copied().unwrap_or(0));
            let points = points.get(start..).filter(|p| !p.is_empty())?;
            Some((name.clone(), json!(points)))
        })
        .collect();
    Value::Object(series)
}

/// The rows of the view's table after `from` of them.
fn rows_after(body: &Value, snapshot: Option<&Snapshot>, from: &BTreeMap<String, usize>) -> Value {
    let Some(snapshot) = snapshot else {
        return json!({});
    };
    let rows: serde_json::Map<String, Value> = body["rows"]
        .as_object()
        .into_iter()
        .flat_map(|rows| rows.keys())
        .filter_map(|name| {
            let start = from.get(name).copied().unwrap_or(0);
            let rows = snapshot
                .rows
                .get(name)?
                .get(start..)
                .filter(|r| !r.is_empty())?;
            Some((name.clone(), json!(rows)))
        })
        .collect();
    Value::Object(rows)
}

pub(super) const MAX_OUTCOME_GROUPS: usize = 32;

fn outcome_groups(rows: &[orx_signals::Signals], by: &str) -> Value {
    let mut groups = BTreeMap::<String, (usize, usize)>::new();
    for row in rows.iter().rev() {
        let key = match row.get(by) {
            Some(Value::String(key)) if !key.trim().is_empty() => key.clone(),
            Some(Value::Number(key)) => key.to_string(),
            _ => continue,
        };
        if groups.len() == MAX_OUTCOME_GROUPS && !groups.contains_key(&key) {
            continue;
        }
        let (ok, n) = groups.entry(key).or_default();
        match row.get("outcome").map(super::view::outcome) {
            Some("correct") => {
                *ok += 1;
                *n += 1;
            }
            Some("wrong" | "error") => *n += 1,
            _ => {}
        }
    }
    Value::Array(
        groups
            .into_iter()
            .map(|(key, (ok, n))| json!({"key": key, "ok": ok, "n": n}))
            .collect(),
    )
}

/// How long each series and row table is, and how many rows the fold has dropped in all.
#[derive(Clone, PartialEq, Default)]
struct Lengths {
    series: BTreeMap<String, usize>,
    rows: BTreeMap<String, usize>,
    rows_dropped: u64,
}

impl Lengths {
    fn of(snapshot: Option<&Snapshot>) -> Self {
        let Some(snapshot) = snapshot else {
            return Self::default();
        };
        Self {
            series: snapshot
                .series
                .iter()
                .map(|(name, points)| (name.clone(), points.len()))
                .collect(),
            rows: snapshot
                .rows
                .iter()
                .map(|(name, rows)| (name.clone(), rows.len()))
                .collect(),
            rows_dropped: snapshot.omitted.get("rows").copied().unwrap_or(0),
        }
    }

    /// How many rows left the front of each table when `later` only adds to these, its tables
    /// past the fold's row cap sliding forward; `was` and `now` are the checkpoints they measure.
    fn slid(
        &self,
        later: &Self,
        was: Option<&Snapshot>,
        now: Option<&Snapshot>,
    ) -> Option<BTreeMap<String, usize>> {
        let series_grew = self
            .series
            .iter()
            .all(|(name, len)| later.series.get(name).is_some_and(|later| later >= len));
        if !series_grew {
            return None;
        }
        let dropped = later.rows_dropped.checked_sub(self.rows_dropped)?;
        let mut slid = BTreeMap::new();
        for (name, &len) in &self.rows {
            let left = if dropped == 0 {
                (later.rows.get(name).copied().unwrap_or(0) >= len).then_some(0)?
            } else {
                let old = was?.rows.get(name).map_or(&[][..], Vec::as_slice);
                let new = now?.rows.get(name).map_or(&[][..], Vec::as_slice);
                let most = len.min(usize::try_from(dropped).unwrap_or(usize::MAX));
                (0..=most).find(|&k| new.get(..len - k) == old.get(k..))?
            };
            if left > 0 {
                slid.insert(name.clone(), left);
            }
        }
        Some(slid)
    }
}

fn hash(evaluated: &Evaluated, snapshot: Option<&Snapshot>) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(evaluated.summary.to_string());
    hasher.update(evaluated.body.to_string());
    if let Some(s) = snapshot {
        hasher.update(format!("{}:{}:{}", s.generation, s.rev, s.series_rev));
    }
    format!("{:x}", hasher.finalize())[..16].to_owned()
}

#[derive(Clone)]
struct Entry {
    projection: Arc<Projection>,
    /// The checkpoint's and the status file's stamps when last read.
    stamps: [Option<FileStamp>; 2],
    /// The checkpoint is held only while the run is live.
    fold: Fold,
    live: bool,
    life_key: String,
    evaluated_at: i64,
    lengths: Lengths,
    fold_series_rev: u64,
    /// Why its last projection failed, as logged; the projection shown then says it failed.
    failed: Option<String>,
}

/// One run's place in the cache. `turn` is held by the one projecting it, across the read, the
/// store write and the event, and is never waited on while there is a projection to show.
#[derive(Default)]
struct Slot {
    held: Mutex<Option<Arc<Entry>>>,
    turn: Mutex<()>,
}

impl Slot {
    fn held(&self) -> Result<Option<Arc<Entry>>> {
        Ok(self
            .held
            .lock()
            .map_err(|_| anyhow!("A run monitor is poisoned"))?
            .clone())
    }
}

type Key = (PathBuf, String);

/// What a reader is told while a run's projection fails; why is logged.
const MONITOR_FAILED: &str = "Couldn't update this run's progress. OpenResearch is trying again.";
const SIGNALS_UNREADABLE: &str =
    "Couldn't read the progress saved for this run. Its log shows what it printed.";

const MONITOR_UNREAD: &str = "Couldn't load this run's progress. Try again in a moment.";
const SERIES_MOVING: &str =
    "This run's progress changed each time it was read. Try again in a moment.";

/// Ended runs whose projection stays in memory after they were last opened.
const ENDED_HELD: usize = 32;

#[derive(Default)]
struct Cache {
    slots: HashMap<Key, Arc<Slot>>,
    /// Ended runs held, least recently projected first.
    ended: std::collections::VecDeque<Key>,
}

fn cache() -> Result<std::sync::MutexGuard<'static, Cache>> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| anyhow!("The run monitor cache is poisoned"))
}

fn key(store: &Store, run: &str) -> Key {
    (store.data_dir().to_path_buf(), run.to_owned())
}

fn slot(store: &Store, run: &str) -> Result<Arc<Slot>> {
    Ok(cache()?.slots.entry(key(store, run)).or_default().clone())
}

/// Keeps every live run in the cache and only the most recently projected ended ones.
fn remember(store: &Store, run: &str, live: bool) -> Result<()> {
    let key = key(store, run);
    let mut cache = cache()?;
    cache.ended.retain(|held| *held != key);
    if live {
        return Ok(());
    }
    cache.ended.push_back(key);
    while cache.ended.len() > ENDED_HELD {
        let unused = cache
            .ended
            .iter()
            .position(|key| Arc::strong_count(&cache.slots[key]) == 1);
        let Some(oldest) = unused.and_then(|i| cache.ended.remove(i)) else {
            break;
        };
        cache.slots.remove(&oldest);
    }
    Ok(())
}

/// Drops a deleted run's slot once nobody is using it.
fn forget(store: &Store, run: &str) -> Result<()> {
    let key = key(store, run);
    let mut cache = cache()?;
    cache.ended.retain(|held| *held != key);
    if let Some(slot) = cache.slots.get(&key) {
        if Arc::strong_count(slot) == 1 {
            cache.slots.remove(&key);
        } else {
            cache.ended.push_front(key);
        }
    }
    Ok(())
}

type Read = HashMap<Key, (Option<FileStamp>, Arc<Snapshot>)>;

/// A run's checkpoint, read at most once while it is unchanged: the one a live run's projection
/// holds, else its file, kept for the few ended runs that others are compared with.
pub(crate) fn checkpoint(store: &Store, run: &str) -> Result<Option<Arc<Snapshot>>> {
    static READ: OnceLock<Mutex<Read>> = OnceLock::new();
    let read = || {
        READ.get_or_init(Mutex::default)
            .lock()
            .map_err(|_| anyhow!("The run checkpoint cache is poisoned"))
    };
    let key = key(store, run);
    let slot = cache()?.slots.get(&key).cloned();
    let held = slot.map(|slot| slot.held()).transpose()?.flatten();
    if let Some(snapshot) = held.and_then(|held| held.fold.snapshot.clone()) {
        return Ok(Some(snapshot));
    }
    let [path, _] = signal_paths(store.data_dir(), run);
    let stamp = stamp_of(&path);
    if stamp.is_none() {
        return Ok(None);
    }
    if let Some((at, snapshot)) = read()?.get(&key) {
        if *at == stamp {
            return Ok(Some(snapshot.clone()));
        }
    }
    #[cfg(test)]
    CHECKPOINT_READS.with(|reads| reads.set(reads.get() + 1));
    let snapshot = super::adapters::local::signals::validate_run(run)
        .and_then(|_| super::adapters::local::signals::load(&path))
        .with_context(|| format!("read run {run}'s signals from {}", path.display()))?;
    let Some(snapshot) = snapshot.map(Arc::new) else {
        return Ok(None);
    };
    let mut read = read()?;
    if read.len() >= ENDED_HELD && !read.contains_key(&key) {
        if let Some(oldest) = read.keys().next().cloned() {
            read.remove(&oldest);
        }
    }
    read.insert(key, (stamp, snapshot.clone()));
    Ok(Some(snapshot))
}

/// The checkpoints of the runs `life` is compared with and timed from, parent and earlier; one that
/// cannot be read is no comparison and no timing, and its own run says why.
fn related_checkpoints(store: &Store, life: &Lifecycle) -> [Option<Arc<Snapshot>>; 2] {
    [&life.parent, &life.earlier].map(|run| {
        let run = run.as_ref()?;
        checkpoint(store, &run.id).unwrap_or_else(|error| {
            log(format_args!(
                "run {} beside {}: {error:#}",
                life.run.id, run.id
            ));
            None
        })
    })
}

type Sink = Box<dyn Fn(String, Arc<Value>) + Send + Sync>;
static SINK: OnceLock<Sink> = OnceLock::new();

/// Where `run.monitor` events go once published: the engine's bus, in `orx up`.
pub(crate) fn install_events(sink: impl Fn(String, Arc<Value>) + Send + Sync + 'static) {
    let _ = SINK.set(Box::new(sink));
}

/// The run's monitor as last published. A live run this process has not projected is projected
/// now, and so is an ended one whose summary says less than its lifecycle, or whose last projection
/// failed; any other ended run is listed from the summary it last published, and its checkpoint is
/// read only when it is opened. `None` for an ended run that never published a monitor.
pub(crate) fn current(store: &Store, run: &StoredRun) -> Result<Option<Arc<Projection>>> {
    let now = crate::store::now_ms();
    let ended = Phase::parse(&run.status).filter(|phase| phase.is_terminal());
    let settled = |published: &Published| {
        if ended.is_none() {
            return true;
        }
        if published.phase != ended {
            return false;
        }
        // Projection reports any errors reading the state behind a flag or revision.
        match published.summary["tone"].as_str() {
            Some("revising") => store.run_revising(run).is_ok_and(|revising| revising),
            Some("flag") => super::flags::read(store, run).is_ok_and(|flags| {
                let saved = &published.summary["flag"];
                super::flags::open(&flags).is_some_and(|flag| {
                    flag["id"] == saved["id"]
                        && flag["state"] == saved["state"]
                        && flag["handback"] == saved["handback"]
                })
            }),
            _ => published.summary["tone"] == tone(ended),
        }
    };
    let slot = cache()?.slots.get(&key(store, &run.id)).cloned();
    if let Some(held) = slot.map(|slot| slot.held()).transpose()?.flatten() {
        let reusable = if held.failed.is_some() {
            now - held.evaluated_at < interval(1)
                && (ended.is_none() || held.projection.summary()["tone"] == tone(ended))
        } else {
            settled(&held.projection.published)
        };
        if reusable {
            return Ok(Some(held.projection.clone()));
        }
        return reprojected(store, run, now).map(Some);
    }
    if ended.is_none() {
        return reprojected(store, run, now).map(Some);
    }
    let published = store
        .run_monitor_state(&run.id)?
        .and_then(|state| Published::read(&state["published"]))
        .filter(|published| published.summary.is_object());
    let Some(published) = published else {
        return Ok(None);
    };
    if !settled(&published) {
        return reprojected(store, run, now).map(Some);
    }
    Ok(Some(Arc::new(Projection {
        summary: published.summary.clone(),
        body: Value::Null,
        snapshot: None,
        published,
        slices: 0,
    })))
}

/// [`advance`], or what it showed when it failed: a projection that says so.
fn reprojected(store: &Store, run: &StoredRun, now: i64) -> Result<Arc<Projection>> {
    advance(store, run, now).or_else(|error| match slot(store, &run.id)?.held()? {
        Some(held) if held.failed.is_some() => Ok(held.projection.clone()),
        _ => Err(error),
    })
}

/// Keeps every live run's monitor current: projected each second, and at once when its lifecycle
/// changes. A run stays watched after it ends through final collection and pending flag reviews.
pub(crate) async fn keep_current(
    engine: Arc<super::Engine>,
    mut stop: tokio::sync::watch::Receiver<()>,
) {
    use super::bus::RunEvent;
    use tokio::sync::broadcast::error::RecvError;
    let sink = engine.clone();
    install_events(move |run_id, event| sink.bus.publish(RunEvent::Monitor { run_id, event }));
    let mut events = engine.bus.subscribe();
    let mut tick = tokio::time::interval(crate::config::paced(Duration::from_secs(1)));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut watched = BTreeSet::new();
    let mut store = None;
    loop {
        let changed = tokio::select! {
            _ = stop.changed() => return,
            _ = tick.tick() => None,
            event = events.recv() => match event {
                Ok(RunEvent::Changed { run_id, .. } | RunEvent::LogCollection { run_id } | RunEvent::FlagReview { run_id }) => Some(run_id),
                Err(RecvError::Closed) => return,
                _ => continue,
            },
        };
        // A pass that fails keeps watching what it held.
        let held = watched.clone();
        let engine = engine.clone();
        let pass = tokio::task::spawn_blocking(move || {
            let store = match store {
                Some(store) => store,
                None => Store::open()?,
            };
            let next = watch(&store, changed, held, Some(&engine));
            Ok::<_, crate::error::Error>((store, next))
        })
        .await;
        match pass {
            Ok(Ok((held, next))) => {
                store = Some(held);
                match next {
                    Ok(next) => watched = next,
                    Err(error) => eprintln!("orx up: run monitors: {error:#}"),
                }
            }
            Ok(Err(error)) => {
                store = None;
                eprintln!("orx up: opening the run monitors' store: {error:#}");
            }
            Err(error) => {
                store = None;
                eprintln!("orx up: run monitors: {error}");
            }
        }
    }
}

/// One pass of [`keep_current`]: projects the run that `changed`, or else every run `held`, live or
/// awaiting collection, checks each live run's rules, and returns the runs to watch next. A run
/// whose projection failed stays. Without an `engine`, a rule's stop is recorded in the store
/// alone.
fn watch(
    store: &Store,
    changed: Option<String>,
    held: BTreeSet<String>,
    engine: Option<&super::Engine>,
) -> Result<BTreeSet<String>> {
    let ids = match changed {
        Some(id) => std::iter::once(id).collect(),
        None => {
            let mut ids = held.clone();
            ids.extend(store.list_active_runs()?.into_iter().map(|run| run.id));
            ids.extend(store.pending_signal_collections(false, 1024)?);
            ids.extend(store.pending_flag_review_runs()?);
            ids
        }
    };
    let mut next = held;
    for id in ids {
        let Some(run) = store.get_run(&id)? else {
            next.remove(&id);
            forget(store, &id)?;
            continue;
        };
        // A failure is logged where it happens, once.
        let now = crate::store::now_ms();
        let advanced = advance(store, &run, now);
        if let Ok(projection) = &advanced {
            let checked = check_rules(store, engine, &run, projection, crate::store::now_ms());
            let mut failing = rule_errors().lock().unwrap_or_else(PoisonError::into_inner);
            match checked {
                Ok(()) => {
                    failing.remove(&run.id);
                }
                Err(error) => {
                    let cause = format!("{error:#}");
                    if failing.get(&run.id) != Some(&cause) {
                        log(format_args!("run {} rules: {cause}", run.id));
                        failing.insert(run.id.clone(), cause);
                    }
                }
            }
        }
        let failed = advanced.is_err();
        let ended = Phase::parse(&run.status).is_some_and(Phase::is_terminal);
        // A finished run can still change when the agent's turn ends.
        let agent_work = advanced.as_ref().is_ok_and(|projection| {
            projection.summary["tone"] == "revising"
                || matches!(
                    projection.summary["flag"]["state"].as_str(),
                    Some("raised" | "asking" | "waking" | "agent_looking")
                )
        });
        if ended && !failed && !agent_work && store.signal_collection_retry_at(&id)?.is_none() {
            next.remove(&id);
        } else {
            next.insert(id);
        }
    }
    Ok(next)
}

/// Why each run's rules last failed to be checked, so a lasting failure is logged once.
fn rule_errors() -> &'static Mutex<HashMap<String, String>> {
    static ERRORS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    ERRORS.get_or_init(Mutex::default)
}

/// Checks a live run's rules against what it reported since they last looked, and acts on any
/// that tripped: an `ask` wakes the run's chats, a `stop` cancels the run through the engine and
/// keeps who stopped it. Nothing is checked on data that is not live, or while a stop is pending.
fn check_rules(
    store: &Store,
    engine: Option<&super::Engine>,
    run: &StoredRun,
    projection: &Arc<Projection>,
    now: i64,
) -> Result<()> {
    use super::rules::{Does, Facts};
    let body = &projection.body;
    let Some(snapshot) = projection.snapshot.as_deref() else {
        return Ok(());
    };
    if Phase::parse(&run.status) != Some(Phase::Running)
        || run.cancel_requested
        || body["availability"]["state"] != "live"
    {
        return Ok(());
    }
    let rules = super::rules::frozen(&run.id, store.run_monitor_config(&run.id)?.as_deref())?;
    if rules.is_empty() {
        return Ok(());
    }
    let Some(held) = slot(store, &run.id)?
        .held()?
        .filter(|held| Arc::ptr_eq(&held.projection, projection))
    else {
        return Ok(());
    };
    let sample = &held.fold.status["sample"];
    let Some(sample_at) = sample["at"].as_i64() else {
        return Ok(());
    };
    if held.fold.status["state"] != "cached"
        || sample["checkpoint"][0].as_str() != Some(snapshot.generation.as_str())
        || sample["checkpoint"][1].as_u64() != Some(snapshot.rev)
        || snapshot
            .coverage
            .reasons
            .iter()
            .any(|reason| reason != "truncated")
    {
        return Ok(());
    }
    let paths = signal_paths(store.data_dir(), &run.id);
    let checked_at = crate::store::now_ms();
    let current = || {
        let at = now.saturating_add(crate::store::now_ms().saturating_sub(checked_at));
        (0..=STALE_AFTER_MS).contains(&at.saturating_sub(sample_at))
            && paths
                .iter()
                .zip(held.stamps)
                .all(|(path, stamp)| stamp_of(path) == stamp)
    };
    if !current() {
        return Ok(());
    }
    let Some(state) = store
        .engine_run_state(&run.id)?
        .filter(|state| state.contact == Contact::Live && state.cancel_accepted_at.is_none())
    else {
        return Ok(());
    };
    let parent = match body["parent"]["runId"].as_str() {
        Some(parent) => checkpoint(store, parent)?,
        None => None,
    };
    let facts = Facts {
        snapshot,
        parent: parent.as_deref(),
        body,
        slices: projection.slices,
        started_at: state.started_at.unwrap_or(run.created_at),
        now: sample_at,
    };
    let counts = store
        .run_monitor_state(&run.id)?
        .and_then(|state| state.get("ruleState").cloned());
    let Some(checked) = super::rules::check(&rules, counts.as_ref(), &facts) else {
        return Ok(());
    };
    let stop = checked
        .trips
        .iter()
        .find(|trip| trip.rule.does == Does::Stop);
    let mut flags = Vec::new();
    if stop.is_none() {
        for trip in &checked.trips {
            let flag = super::rules::flag(trip, projection.summary["whole"].clone(), now);
            flags.push(super::flags::new_flag(store, run, flag, false)?);
        }
    }
    let stop = stop.map(|trip| {
        let stopped = super::rules::stopped(trip);
        let context = crate::store::StopContext {
            reason: Some(trip.evidence.clone()),
            rule: Some(trip.rule.json()),
            ..crate::store::StopContext::by(crate::store::StopBy::Rule)
        };
        (stopped, context)
    });
    let woken: Vec<String> = flags
        .iter()
        .flat_map(|flag| flag.wakes.iter().map(|(chat, _)| chat.clone()))
        .collect();
    let record = |store: &Store| {
        store.record_rules(
            run,
            &checked.counts,
            &flags,
            stop.as_ref()
                .map(|(reason, context)| (reason.as_str(), context)),
            |settings| {
                let patterns = super::view::stored_patterns(&run.experiment_id, settings)?;
                Ok(patterns.hash() == snapshot.config_hash && current())
            },
        )
    };
    match engine {
        Some(engine) if stop.is_some() => {
            engine.accept_stop(&run.id, record)?;
        }
        _ => {
            record(store)?;
        }
    }
    crate::commands::up::api_v2::notices_queued(woken);
    Ok(())
}

/// Re-projects the run when anything it reads changed, at most once a second, and publishes a
/// new `rev` with its event when the result differs. Time alone re-evaluates a live run every
/// ten seconds, so a stall, the limit and the cost advance without new reports.
pub(crate) fn advance(store: &Store, run: &StoredRun, now: i64) -> Result<Arc<Projection>> {
    publish(store, run, now, SINK.get()).map(|(projection, _)| projection)
}

/// [`advance`], sending to `sink`: the projection, and its event when it published a new `rev`.
fn publish(
    store: &Store,
    run: &StoredRun,
    now: i64,
    sink: Option<&Sink>,
) -> Result<(Arc<Projection>, Option<Value>)> {
    let slot = slot(store, &run.id)?;
    // Another projection of this run is under way: what it last showed stands until it lands.
    let _turn = match slot.turn.try_lock() {
        Ok(turn) => turn,
        Err(std::sync::TryLockError::WouldBlock) => match slot.held()? {
            Some(held) => return Ok((held.projection.clone(), None)),
            None => slot.turn.lock().unwrap_or_else(PoisonError::into_inner),
        },
        Err(std::sync::TryLockError::Poisoned(turn)) => turn.into_inner(),
    };
    let held = slot.held()?;
    let projected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        project(store, run, now, sink, &slot)
    }));
    let was = held.as_ref().and_then(|held| held.failed.clone());
    let cause = match projected {
        Ok(Ok(Some(projected))) => {
            if was.is_some() {
                log(format_args!("run {} monitor recovered", run.id));
            }
            return Ok(projected);
        }
        Ok(Ok(None)) => return Err(anyhow!(MONITOR_UNREAD)),
        Ok(Err(error)) => format!("{error:#}"),
        Err(panic) => panic
            .downcast_ref::<&str>()
            .map(|why| (*why).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic".into()),
    };
    if was.as_ref() != Some(&cause) {
        log(format_args!(
            "run {} ({}) monitor failed: {cause}",
            run.id, run.status
        ));
    } // Until a projection succeeds, every reader is told it failed, never shown the last one as if
      // it were live.
    let published = match held.as_ref() {
        Some(held) => Some(held.projection.published.clone()),
        None => store
            .run_monitor_state(&run.id)
            .ok()
            .flatten()
            .and_then(|state| Published::read(&state["published"])),
    };
    let mut summary = published
        .as_ref()
        .map(|p| p.summary.clone())
        .filter(Value::is_object)
        .unwrap_or_else(|| {
            json!({
                "whole": null, "stage": null, "headline": null, "sentence": null,
                "stageOrder": {"index": null, "count": null, "first": null},
                "bindings": {}, "signals": {}, "limitEndsAt": null, "flag": null,
                "etaFloor": false, "setup": null, "logError": null,
                "generation": "", "asOf": run.updated_at, "rev": 0,
            })
        });
    summary["tone"] = json!(tone(Phase::parse(&run.status)));
    summary["estimate"] = Value::Null;
    summary["unavailable"] = json!(MONITOR_FAILED);
    let mut body = held
        .as_ref()
        .map_or(Value::Null, |held| held.projection.body.clone());
    if body.is_object() {
        body["availability"]["state"] = json!("unavailable");
        body["availability"]["reason"] = json!(MONITOR_FAILED);
    }
    let published = published.unwrap_or_else(|| Published {
        generation: String::new(),
        phase: None,
        fold_rev: 0,
        rev: 0,
        series_rev: 0,
        hash: String::new(),
        summary: Value::Null,
    });
    let failed = Projection {
        summary,
        body,
        snapshot: held
            .as_ref()
            .and_then(|held| held.projection.snapshot.clone()),
        published: published.clone(),
        slices: 0,
    };
    let entry = match held.as_deref() {
        Some(held) => Entry {
            projection: Arc::new(failed),
            evaluated_at: now,
            failed: Some(cause),
            ..held.clone()
        },
        None => Entry {
            projection: Arc::new(failed),
            stamps: [None, None],
            fold: Fold {
                snapshot: None,
                error: None,
                status: Value::Null,
            },
            live: false,
            life_key: String::new(),
            evaluated_at: now,
            lengths: Lengths::default(),
            fold_series_rev: 0,
            failed: Some(cause),
        },
    };
    *slot.held.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(entry));
    if let Some(sink) = sink.filter(|_| was.is_none()) {
        let reread = json!({
            "runId": run.id,
            "rev": published.rev,
            "generation": published.generation,
            "reset": true,
        });
        sink(run.id.clone(), Arc::new(reread));
    }
    Err(anyhow!(MONITOR_FAILED))
}

/// One projection under the run's turn, or `None` if another publisher advanced its revision.
fn project(
    store: &Store,
    run: &StoredRun,
    now: i64,
    sink: Option<&Sink>,
    slot: &Slot,
) -> Result<Option<(Arc<Projection>, Option<Value>)>> {
    let poisoned = || anyhow!("Run {}'s monitor is poisoned", run.id);
    let held = slot.held()?;
    #[cfg(test)]
    if BREAK_NEXT.with(|next| next.replace(false)) {
        panic!("the projection broke");
    }
    let life = Lifecycle::read(store, run)?;
    let life_key = life.key();
    let [checkpoint, status] = signal_paths(store.data_dir(), &run.id);
    let stamps = [stamp_of(&checkpoint), stamp_of(&status)];
    // What a failed projection showed is never kept as if nothing changed.
    let same_life = held.as_ref().filter(|held| {
        held.failed.is_none()
            && held.life_key == life_key
            && life.final_log_warning_due(held.evaluated_at) == life.final_log_warning_due(now)
    });
    let media = same_life
        .map(|held| super::media::project(store, run, &life.kind, &held.projection.body["view"]));
    let same_media = same_life.filter(|held| media.as_ref() == held.projection.body.get("media"));
    if let Some(held) = same_media {
        let recent = now - held.evaluated_at < interval(1);
        let idle = !life.live() || now - held.evaluated_at < interval(10);
        // New points wait for the next second; losing or recovering a checkpoint does not.
        let throttle =
            life.live() && stamps[0].is_some() && held.fold.status["state"] != "unavailable";
        if (recent || idle) && held.stamps == stamps || recent && throttle {
            return Ok(Some((held.projection.clone(), None)));
        }
    }
    // The status file was rewritten with nothing the projection reads changed: checked without
    // reading the checkpoint, which an ended run no longer holds.
    let rewritten = same_media.filter(|held| {
        // Only a checkpoint gives a projection its generation; without one there is none to read.
        !held.projection.published.generation.is_empty()
            && held.stamps[0] == stamps[0]
            && (!life.live() || now - held.evaluated_at < interval(10))
            && held.fold.status
                == fold_status(store.data_dir(), &run.id, true, held.fold.error.clone())
    });
    if let Some(held) = rewritten {
        *slot.held.lock().map_err(|_| poisoned())? = Some(Arc::new(Entry {
            stamps,
            ..Entry::clone(held)
        }));
        return Ok(Some((held.projection.clone(), None)));
    }
    let fold = fold_at(store.data_dir(), &run.id, stamps[0], held.as_deref());
    let [parent, earlier] = related_checkpoints(store, &life);
    let mut evaluated = evaluate(&fold, &life, parent.as_deref(), earlier.as_deref(), now);
    evaluated.body["media"] = media
        .unwrap_or_else(|| super::media::project(store, run, &life.kind, &evaluated.body["view"]));
    let snapshot = fold.snapshot.as_deref();
    let hash = hash(&evaluated, snapshot);
    let previous = match held.as_ref() {
        Some(held) => Some(held.projection.published.clone()),
        None => store
            .run_monitor_state(&run.id)?
            .and_then(|state| Published::read(&state["published"])),
    };
    let generation = snapshot.map_or("", |s| s.generation.as_str()).to_owned();
    let fold_rev = snapshot.map_or(0, |s| s.rev);
    let lengths = Lengths::of(snapshot);
    let fold_series_rev = snapshot.map_or(0, |s| s.series_rev);

    let slices = evaluated.slices;
    let mut summary = evaluated.summary;
    let mut body = evaluated.body;
    // A recovery is published, so readers told of the failure hear it ended.
    let unchanged = previous.as_ref().filter(|p| {
        p.phase == life.phase
            && p.hash == hash
            && held.as_ref().is_none_or(|held| held.failed.is_none())
    });
    let (published, event) = match unchanged {
        Some(previous) => (previous.clone(), None),
        None => {
            let base = previous.clone().unwrap_or(Published {
                generation: String::new(),
                phase: None,
                fold_rev: 0,
                rev: 0,
                series_rev: 0,
                hash: String::new(),
                summary: Value::Null,
            });
            // Points and rows extend what this process published only on the same basis.
            let basis = held.as_ref().and_then(|held| {
                let same = held.projection.published.generation == generation
                    && held.fold_series_rev == fold_series_rev
                    && held.projection.body["view"] == body["view"]
                    && held.projection.body["parent"] == body["parent"];
                let slid = held
                    .lengths
                    .slid(&lengths, held.fold.snapshot.as_deref(), snapshot);
                slid.filter(|_| same).map(|slid| (held, slid))
            });
            let append = basis
                .as_ref()
                .map(|(held, _)| series_json(snapshot, Some(&held.lengths.series)));
            let append_rows = basis.as_ref().map(|(held, slid)| {
                let kept = held
                    .lengths
                    .rows
                    .iter()
                    .map(|(name, len)| (name.clone(), len - slid.get(name).unwrap_or(&0)))
                    .collect();
                rows_after(&body, snapshot, &kept)
            });
            let drop_rows: serde_json::Map<String, Value> = basis
                .iter()
                .flat_map(|(_, slid)| slid)
                .filter(|(name, _)| body["rows"].get(name.as_str()).is_some())
                .map(|(name, left)| (name.clone(), json!(left)))
                .collect();
            let added = |value: &Option<Value>| {
                value
                    .as_ref()
                    .is_some_and(|v| v.as_object().is_some_and(|v| !v.is_empty()))
            };
            let grew = added(&append) || added(&append_rows) || !drop_rows.is_empty();
            let series_rev = if basis.is_some() && !grew {
                base.series_rev
            } else {
                base.series_rev + 1
            };
            let rev = base.rev + 1;
            summary["rev"] = json!(rev);
            let published = Published {
                generation: generation.clone(),
                phase: life.phase,
                fold_rev,
                rev,
                series_rev,
                hash: hash.clone(),
                summary: summary.clone(),
            };
            if !store.set_run_monitor_published(&run.id, &published.json())? {
                *slot.held.lock().map_err(|_| poisoned())? = None;
                log(format_args!(
                    "run {} monitor revision {} changed before publication",
                    run.id, base.rev
                ));
                return Ok(None);
            }
            let fields = body.as_object().expect("monitor body is an object");
            let mut event = Value::Object(
                fields
                    .iter()
                    .filter(|(name, _)| !matches!(name.as_str(), "view" | "parent" | "rows"))
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
            );
            event["rev"] = json!(rev);
            event["seriesRev"] = json!(series_rev);
            event["appendFrom"] = json!(base.series_rev);
            event["append"] = append.unwrap_or_else(|| json!({}));
            event["appendRows"] = append_rows.unwrap_or_else(|| json!({}));
            event["dropRows"] = json!(drop_rows);
            if basis.is_none() {
                event["reset"] = json!(true);
            }
            (published, Some(event))
        }
    };

    summary["rev"] = json!(published.rev);
    body["rev"] = json!(published.rev);
    body["seriesRev"] = json!(published.series_rev);
    let live = life.live();
    let projection = Arc::new(Projection {
        summary,
        body,
        // An ended run's series are read again when asked for, never held for every past run.
        snapshot: fold.snapshot.clone().filter(|_| live),
        published,
        slices,
    });
    *slot.held.lock().map_err(|_| poisoned())? = Some(Arc::new(Entry {
        projection: projection.clone(),
        stamps,
        fold: Fold {
            snapshot: fold.snapshot.filter(|_| live),
            ..fold
        },
        live,
        life_key,
        evaluated_at: now,
        lengths,
        fold_series_rev,
        failed: None,
    }));
    remember(store, &run.id, live)?;
    // Sent before the turn ends, so each run's events leave in the order of their revs.
    if let (Some(event), Some(sink)) = (event.as_ref(), sink) {
        sink(run.id.clone(), Arc::new(event.clone()));
    }
    Ok(Some((projection, event)))
}

/// The run's whole monitor at its current `rev`, with series read again for an ended run.
pub(crate) fn monitor(store: &Store, run: &StoredRun) -> Result<(Arc<Projection>, Value)> {
    for _ in 0..3 {
        let projection = advance(store, run, crate::store::now_ms()).context(MONITOR_UNREAD)?;
        if projection.snapshot.is_some() || projection.published.generation.is_empty() {
            let monitor = projection.monitor();
            return Ok((projection, monitor));
        }
        let path = &signal_paths(store.data_dir(), &run.id)[0];
        let snapshot = super::adapters::local::signals::load(path)
            .with_context(|| format!("read run {}'s series from {}", run.id, path.display()))
            .context(SIGNALS_UNREADABLE)?;
        if let Some(snapshot) = snapshot.filter(|s| {
            s.generation == projection.published.generation
                && s.rev == projection.published.fold_rev
        }) {
            let mut monitor = projection.body.clone();
            monitor["series"] = series_json(Some(&snapshot), None);
            return Ok((projection, monitor));
        }
    }
    Err(anyhow!(SERIES_MOVING))
}

/// What a `--json` status or a stopped server reads: the same projection, published by nothing.
pub(crate) fn offline(store: &Store, run: &StoredRun, now: i64) -> Result<Value> {
    let life = Lifecycle::read(store, run)?;
    let [checkpoint, _] = signal_paths(store.data_dir(), &run.id);
    let fold = fold_at(store.data_dir(), &run.id, stamp_of(&checkpoint), None);
    let [parent, earlier] = related_checkpoints(store, &life);
    let evaluated = evaluate(&fold, &life, parent.as_deref(), earlier.as_deref(), now);
    let published = store
        .run_monitor_state(&run.id)?
        .and_then(|state| Published::read(&state["published"]));
    let mut monitor = evaluated.body;
    monitor["media"] = super::media::project(store, run, &life.kind, &monitor["view"]);
    monitor["rev"] = json!(published.as_ref().map_or(0, |p| p.rev));
    monitor["seriesRev"] = json!(published.as_ref().map_or(0, |p| p.series_rev));
    monitor["series"] = series_json(fold.snapshot.as_deref(), None);
    Ok(monitor)
}

/// Shares `MAX_BODY` fairly among the series, keeping the newest points.
pub(crate) fn bounded(mut monitor: Value) -> Value {
    let size = monitor.to_string().len();
    if size <= MAX_BODY {
        return monitor;
    }
    let series = monitor["series"].take();
    let Value::Object(mut series) = series else {
        return monitor;
    };
    let fixed = monitor.to_string().len();
    let points: usize = series
        .values()
        .map(|p| p.as_array().map_or(0, Vec::len))
        .sum();
    let per_point = (size - fixed).max(1) / points.max(1) + 1;
    let share = (MAX_BODY.saturating_sub(fixed) / per_point / series.len().max(1)).max(1);
    for points in series.values_mut().filter_map(Value::as_array_mut) {
        points.drain(..points.len().saturating_sub(share));
    }
    monitor["series"] = Value::Object(series);
    monitor
}

/// Up to `limit` points of one series after `after`, from the run's current checkpoint.
pub(crate) fn series_page(
    monitor: &Value,
    name: &str,
    after: Option<f64>,
    limit: usize,
) -> Option<Value> {
    let points = monitor["series"][name].as_array()?;
    let start = after.map_or(0, |after| {
        points.partition_point(|p| p[0].as_f64().is_some_and(|x| x <= after))
    });
    let page: Vec<&Value> = points[start..].iter().take(limit.min(2048)).collect();
    Some(json!({
        "runId": monitor["runId"],
        "rev": monitor["rev"],
        "generation": monitor["generation"],
        "seriesRev": monitor["seriesRev"],
        "name": name,
        "points": page,
    }))
}

// --- `orx exp status` ------------------------------------------------------

struct StatusFacts<'a> {
    /// The view's signals, whose declared sign and unit the values are printed with.
    signals: &'a Value,
    whole: Option<f64>,
    estimate: Option<&'a Value>,
    estimate_reasons: &'a [String],
    cost: Option<&'a Value>,
    stall_ms: Option<i64>,
    degraded: bool,
    at: i64,
    versus: &'a serde_json::Map<String, Value>,
    /// Why the experiment's saved view could not be read.
    view_problem: Option<&'a str>,
    /// Where the plan counts work the run reports only as rows.
    disagrees: Option<&'a Value>,
    setup: Option<&'a Value>,
    /// A plan's stages as the monitor lists them, and the one the run is in.
    stages: Option<(&'a [Value], Option<usize>)>,
    stage: &'a Value,
    stage_position: Option<(usize, usize)>,
    /// How much slower than the run that timed it the stage is going, when it is.
    pace: Option<&'a Value>,
    /// What the run's sentence reads now.
    bindings: &'a Value,
}

fn status_text(life: &Lifecycle, fold: &Fold, watch: &Value, facts: &StatusFacts) -> String {
    let snapshot = fold.snapshot.as_deref();
    let mut lines = Vec::new();
    let mut first = format!("{}  {}", life.slug, life.state);
    if let Some(whole) = facts.whole.filter(|_| life.started()) {
        first.push_str(&format!("  {:.0}%", (whole * 100.0).clamp(0.0, 100.0)));
    }
    if let Some(name) = facts.stage["name"].as_str() {
        let unit = facts.stage["unit"].as_str().unwrap_or_default();
        let count = facts.stage["done"]
            .as_f64()
            .map(|done| count_words(done, facts.stage["of"].as_f64(), unit));
        let stage = match facts.stage_position {
            Some((position, total)) if !name.is_empty() => {
                format!("{name} ({position}/{total})")
            }
            _ if count.is_some() && counted(name) != counted(unit) => name.to_owned(),
            _ if count.is_none() && life.live() && facts.setup.is_none() => name.to_owned(),
            _ => String::new(),
        };
        match (stage.is_empty(), count) {
            (false, Some(count)) => first.push_str(&format!("  {stage} · {count}")),
            (false, None) => first.push_str(&format!("  {stage}")),
            (true, Some(count)) => first.push_str(&format!("  {count}")),
            (true, None) => {}
        }
    }
    first.push_str(&format!("  [run {}]", life.run.id));
    lines.push(first);
    if let Some(stop) = stop_text(life) {
        lines.push(format!("stop     {stop}"));
    }
    if life.revising {
        lines.push("revising The agent is repairing this experiment and will run it again.".into());
    }
    lines.extend(super::watch::status_lines(
        watch,
        facts.bindings,
        facts.signals,
    ));
    if let Some(flag) = super::flags::open(&life.flags).or(life.flags.last()) {
        lines.push(super::flags::status_line(flag));
    }
    if let Some(eta) = eta_text(life, snapshot, facts) {
        lines.push(format!("eta      {eta}"));
    }
    if let Some((stages, current)) = facts.stages {
        lines.push(format!("stages   {}", stages_text(life, stages, current)));
    }
    match &life.rules {
        Ok(rules) if rules.is_empty() => {}
        Ok(rules) => lines.push(format!(
            "rules    {}",
            rules
                .iter()
                .map(|rule| {
                    let does = match rule.does {
                        super::rules::Does::Stop => "stops the run",
                        super::rules::Does::Ask => "wakes the agent",
                    };
                    match rule.when {
                        super::rules::When::CostAbove { usd } => {
                            format!("{} {does} past ${usd:.2}", rule.name)
                        }
                        _ => format!("{} {does}", rule.name),
                    }
                })
                .collect::<Vec<_>>()
                .join(" \u{b7} ")
        )),
        Err(error) => lines.push(format!("rules    {error}")),
    }
    if let Some(snapshot) = snapshot.filter(|s| !s.latest.is_empty()) {
        let values = snapshot
            .signal_order
            .iter()
            .filter_map(|name| {
                let spec = &facts.signals[name];
                let value = snapshot.latest.get(name)?;
                let label = spec["label"].as_str().unwrap_or(name);
                let mut text = format!("{label} {}", value_text(value, spec));
                if let Some(unit) = spec["unit"].as_str().filter(|unit| *unit != "%") {
                    text.push_str(&format!(" {unit}"));
                }
                if let Some(vs) = facts.versus.get(name) {
                    let parent = match vs["step"].as_f64() {
                        Some(step) => format!("parent at step {}", number(step)),
                        None => "parent's final value".to_owned(),
                    };
                    text.push_str(&if vs["flat"] == true {
                        format!(" (level with {parent})")
                    } else {
                        let mag = vs["mag"].as_f64().unwrap_or_default();
                        let mag = match spec["digits"].as_u64() {
                            Some(digits) => fixed(mag, digits as usize, &json!({})),
                            None => measure(mag, &json!({})),
                        };
                        let word = if vs["good"] == true {
                            "better"
                        } else {
                            "worse"
                        };
                        format!(" ({mag} {word} than {parent})")
                    });
                }
                Some(text)
            })
            .collect::<Vec<_>>()
            .join(" · ");
        lines.push(format!("signals  {values}"));
    }
    if let Some(problem) = facts.view_problem {
        lines.push(format!("view     {problem}"));
    }
    if let Some(disagrees) = facts.disagrees {
        lines.push(format!("plan     {}", disagreement(disagrees)));
    }
    let reading = life.retry_at.is_some() || life.live();
    let mut status = fold.status.clone();
    if let Some(at) = life.retry_at {
        status["retryAt"] = json!(at.max(facts.at));
    }
    if life.started() {
        for note in signal_notes(&life.run.id, reading, &status, snapshot) {
            lines.push(format!("signals  {note}"));
        }
    }
    if let Some(count) = snapshot
        .map(|s| s.diagnostics.counts.values().sum::<u64>())
        .filter(|count| *count > 0)
    {
        lines.push(format!(
            "signals  {count} log diagnostics; inspect `orx logs {}`.",
            life.run.id
        ));
    }
    lines.join("\n")
}

/// Who stopped the run and why, in the lead the page gives it, once a stop was accepted.
fn stop_text(life: &Lifecycle) -> Option<String> {
    use crate::store::StopBy;
    let stop = match &life.stop {
        Ok(stop) => stop.as_ref()?,
        Err(error) => return Some(error.clone()),
    };
    let ended = life.phase == Some(Phase::Cancelled);
    let because = |words: &str| match &stop.reason {
        Some(reason) if reason.ends_with(['.', '!', '?']) => format!("{words}: {reason}"),
        Some(reason) => format!("{words}: {reason}."),
        None => format!("{words}."),
    };
    Some(match (stop.by, ended) {
        (StopBy::User, true) => because("Stopped by the user"),
        (StopBy::User, false) => because("The user asked to stop it"),
        (StopBy::Agent, true) => because("Auto-stopped by the agent"),
        (StopBy::Agent, false) => because("The agent asked to stop it"),
        (StopBy::Rule, _) => {
            let name = stop.rule.as_ref().and_then(|rule| rule["name"].as_str());
            let lead = if ended {
                "Auto-stopped by rule"
            } else {
                "Stopping by rule"
            };
            format!(
                "{} No model was involved.",
                because(&format!("{lead} {}", name.unwrap_or("?")))
            )
        }
    })
}

/// Where the plan and the run disagree, and how to make them agree, as `orx` says it.
pub(crate) fn disagreement(disagrees: &Value) -> String {
    let unit = disagrees["unit"].as_str().unwrap_or("steps");
    format!(
        "the plan counts {} {unit} in {}, but the run reported {} rows in {} and no step, so its bar cannot move: report each finished one as a step (`\"step\": n`, or `p.step(n)` after `p.row(…)`), or declare the stage the run has",
        disagrees["of"],
        disagrees["stage"].as_str().unwrap_or_default(),
        disagrees["rows"],
        disagrees["table"].as_str().unwrap_or_default(),
    )
}

/// A plan's stages on one line: `✓ download 2m · ● train · ○ eval about 11m · – export skipped`.
fn stages_text(life: &Lifecycle, stages: &[Value], current: Option<usize>) -> String {
    stages
        .iter()
        .enumerate()
        .map(|(index, stage)| {
            let name = stage["name"].as_str().unwrap_or_default();
            let unplanned = if stage["planned"] == false {
                " (unplanned)"
            } else {
                ""
            };
            if stage["skipped"] == true {
                return format!("\u{2013} {name} skipped");
            }
            let reached = current.is_some_and(|current| index <= current);
            let here = current == Some(index);
            if reached && !(here && life.live()) && (!here || life.phase == Some(Phase::Done)) {
                let took = stage["startedAt"]
                    .as_i64()
                    .zip(stage["endedAt"].as_i64())
                    .map(|(from, to)| format!(" {}", short(to - from)))
                    .unwrap_or_default();
                return format!("\u{2713} {name}{took}{unplanned}");
            }
            if here || current.is_none() && index == 0 && life.live() {
                let mark = if life.live() { '\u{25cf}' } else { '\u{2717}' };
                return format!("{mark} {name}{unplanned}");
            }
            match stage["priorMs"].as_i64().filter(|_| life.live()) {
                Some(prior) => format!("\u{25cb} {name} about {}", short(prior)),
                None => format!("\u{25cb} {name}"),
            }
        })
        .collect::<Vec<_>>()
        .join(" \u{b7} ")
}

fn eta_text(life: &Lifecycle, snapshot: Option<&Snapshot>, facts: &StatusFacts) -> Option<String> {
    let elapsed = |end: i64| short((end - life.start()).max(0));
    match life.phase? {
        Phase::Submitting | Phase::Starting => return Some("Not started".into()),
        Phase::Done => return Some(format!("Done in {}", elapsed(life.end()))),
        Phase::Failed => return Some(format!("Failed at {}", elapsed(life.end()))),
        Phase::Cancelled => return Some(format!("Stopped at {}", elapsed(life.end()))),
        Phase::Lost => return None,
        Phase::Running => {}
    }
    if life.contact_lost() || facts.degraded {
        let phase = snapshot.and_then(Snapshot::progress)?;
        let mut seen = format!("Last seen at step {}", number(phase.done?));
        if let Some(total) = phase.of {
            seen.push_str(&format!(" of {}", number(total)));
        }
        seen.push('.');
        return Some(with_cost(seen, facts.cost, true));
    }
    let now = facts.at;
    let text = match facts.estimate {
        Some(estimate) if estimate["pending"] == true => {
            // Only bad timing keeps an estimate pending; a cost input degrades the cost alone.
            let timing = [
                "now",
                "as_of",
                "current",
                "started_at",
                "phase_started_at",
                "points",
            ];
            let why = facts
                .estimate_reasons
                .iter()
                .map(String::as_str)
                .find(|reason| timing.iter().any(|field| reason.starts_with(field)));
            format!("Estimating… ({})", why.unwrap_or("needs timed progress"))
        }
        Some(estimate) => {
            let gone = now - estimate["at"].as_i64().unwrap_or(now);
            let left = |key: &str| short((estimate[key].as_i64().unwrap_or(0) - gone).max(0));
            let (lo, hi) = (left("loMs"), left("hiMs"));
            let floor = estimate["floor"] == true;
            let untimed: Vec<&str> = estimate["untimed"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            // A run that reports in chunks can pass its midpoint between reports.
            let mid = estimate["midMs"].as_i64().unwrap_or(0);
            let mid_left = mid - gone;
            if !floor && mid > 0 && mid <= gone {
                let hi = estimate["hiMs"].as_i64().unwrap_or(mid);
                if facts.stall_ms.is_none_or(|ms| ms <= 0) && hi > gone {
                    return Some(with_cost("Almost done".into(), facts.cost, false));
                }
                let due = now - gone + mid;
                return Some(with_cost(
                    format!(
                        "No progress for {} (was due about {})",
                        short(gone),
                        clock_text(due)
                    ),
                    facts.cost,
                    false,
                ));
            }
            // A floor counted down to nothing promises nothing: what is left is unknown.
            if floor && mid_left < 1_000 {
                return Some(with_cost(
                    if untimed.is_empty() {
                        "No estimate (timed from progress in the log; more work may follow it)"
                            .to_owned()
                    } else {
                        format!("No estimate (no earlier run timed {})", untimed.join(", "))
                    },
                    facts.cost,
                    false,
                ));
            }
            // A range whose ends read alike says nothing the time left does not.
            let mut detail: Vec<String> = if !untimed.is_empty() {
                vec![format!("no earlier run timed {}", untimed.join(", "))]
            } else if floor {
                vec!["timed from progress in the log; more work may follow it".to_owned()]
            } else {
                (lo != hi)
                    .then(|| format!("{lo} to {hi}"))
                    .into_iter()
                    .collect()
            };
            if let Some(pace) = facts.pace {
                let than = if pace["than"] == "earlier" {
                    "this experiment's last run"
                } else {
                    "the parent"
                };
                detail.push(format!(
                    "{}\u{d7} slower than {than} at this stage",
                    number(pace["times"].as_f64().unwrap_or_default())
                ));
            }
            if let Some(past) = estimate["pastLimitMs"].as_i64().filter(|ms| *ms > 0) {
                detail.push(format!("up to {} past the limit", short(past)));
            }
            if let Some(stall) = facts.stall_ms.filter(|ms| *ms > 0) {
                detail.push(format!(
                    "no progress for at least {}; assumes it resumes",
                    short(stall)
                ));
            }
            // A floor that every later stage is missing from counts only the stage the run is in.
            let stage = facts.stages.and_then(|(stages, current)| {
                let current = current?;
                let name = stages.get(current)?["name"]
                    .as_str()
                    .filter(|name| !name.is_empty())?;
                let later = stages.len() - current - 1;
                (later > 0 && untimed.len() == later && !untimed.contains(&name)).then_some(name)
            });
            let duration = short(mid_left.max(1_000));
            let left = match stage {
                Some(stage) if floor => format!("{duration} of {stage} left"),
                _ if floor => format!("At least {duration} left"),
                _ => format!("{duration} left"),
            };
            if detail.is_empty() {
                left
            } else {
                format!("{left} ({})", detail.join("; "))
            }
        }
        None => match life.ends_at() {
            Some(end) => format!(
                "{} to the limit (ends by {})",
                short((end - now).max(0)),
                clock_text(end)
            ),
            None => match snapshot
                .and_then(Snapshot::progress)
                .filter(|p| p.done.is_some())
            {
                None if let Some(setup) = facts.setup => {
                    let state = setup["stage"].as_str().map_or_else(
                        || "Setting up".to_owned(),
                        |stage| format!("{stage} started"),
                    );
                    match setup["line"].as_str() {
                        Some(line) => format!("{state} · {line}"),
                        None => state,
                    }
                }
                // Most runs never report progress, so nothing is promised that may never come.
                None => "No estimate (no progress found in the log)".to_owned(),
                // A bar with a total says how much of it is left, never how much of the run.
                Some(phase) if phase.of.is_some() => {
                    "No estimate (how much is left is unknown)".to_owned()
                }
                Some(_) => "No estimate (no total or time limit)".to_owned(),
            },
        },
    };
    Some(with_cost(text, facts.cost, false))
}

fn with_cost(text: String, cost: Option<&Value>, last_seen: bool) -> String {
    let Some(cost) = cost else {
        return text;
    };
    let usd = |key: &str| cost[key].as_f64().map(|usd| format!("${usd:.2}"));
    let detail = if last_seen {
        usd("soFar").map(|so_far| format!("{so_far} when last seen"))
    } else {
        usd("soFar").map(|so_far| match usd("projected") {
            Some(total) => format!("{so_far} so far, about {total} in all"),
            None => format!("{so_far} so far"),
        })
    };
    match detail {
        Some(detail) => format!("{text}  ·  {detail}"),
        None => text,
    }
}

/// A duration as the page writes it (`fmtDuration`), rounded down: "45s", "12m", "1h 0m", "2d 3h".
pub(crate) fn short(ms: i64) -> String {
    let seconds = ms.max(0) / 1000;
    let (minutes, hours) = (seconds / 60, seconds / 3600);
    match (hours, minutes) {
        (_, 0) => format!("{seconds}s"),
        (0, m) => format!("{m}m"),
        (h, m) if h < 24 => format!("{h}h {}m", m % 60),
        (h, _) => format!("{}d {}h", h / 24, h % 24),
    }
}

/// `HH:MM UTC`: status prints for agents and logs, which carry no time zone.
pub(crate) fn clock_text(ms: i64) -> String {
    let iso = crate::telemetry::iso8601_utc(ms);
    format!("{} UTC", &iso[11..16])
}

/// One measurement the way its view declared it, as the page's `fmtSignal` writes it: the word
/// `labels` gives its raw value, else as a duration, compactly, or scaled and rounded to `digits`.
pub(crate) fn value_text(value: &Value, spec: &Value) -> String {
    let raw = match value {
        Value::Number(n) => n.as_f64().map(|n| n.to_string()),
        Value::String(text) => Some(text.clone()),
        _ => None,
    };
    if let Some(word) = raw.and_then(|raw| spec["labels"][raw].as_str().map(str::to_owned)) {
        return word;
    }
    match value {
        Value::String(text) => text.clone(),
        Value::Number(n) => n.as_f64().map_or_else(
            || number(value),
            |value| match spec["as"].as_str() {
                Some("duration") => short((value * 1000.0) as i64),
                Some("compact") => compact(value),
                _ => {
                    let scaled = value * spec["scale"].as_f64().unwrap_or(1.0);
                    match spec["digits"].as_u64() {
                        Some(_) if scaled.abs() >= 1000.0 => fixed(scaled, 0, spec),
                        Some(digits) => fixed(scaled, digits as usize, spec),
                        None => measure(scaled, spec),
                    }
                }
            },
        ),
        Value::Null => "—".into(),
        other => other.to_string(),
    }
}

/// `value` to exactly `digits` places, rounded half away from zero on its shortest decimal, with
/// grouping and the sign and `%` its view declares, as `Intl.NumberFormat` writes it.
pub(crate) fn fixed(value: f64, digits: usize, spec: &Value) -> String {
    let shortest = value.abs().to_string();
    let (whole, fraction) = shortest.split_once('.').unwrap_or((&shortest, ""));
    let mut kept: Vec<u8> = whole
        .bytes()
        .chain(fraction.bytes().chain(std::iter::repeat(b'0')).take(digits))
        .collect();
    if fraction.as_bytes().get(digits).is_some_and(|d| *d >= b'5') {
        match kept.iter().rposition(|d| *d != b'9') {
            Some(at) => {
                kept[at] += 1;
                kept[at + 1..].fill(b'0');
            }
            None => {
                kept.fill(b'0');
                kept.insert(0, b'1');
            }
        }
    }
    let point = kept.len() - digits;
    let text = String::from_utf8(kept).expect("ASCII digits");
    let (whole, fraction) = text.split_at(point);
    let whole = whole.trim_start_matches('0');
    let mut text = number(if whole.is_empty() { "0" } else { whole });
    if digits > 0 {
        text.push('.');
        text.push_str(fraction);
    }
    let zero = text.bytes().all(|b| matches!(b, b'0' | b'.' | b','));
    let sign = if spec["sign"] == true {
        match () {
            _ if zero => "",
            _ if value < 0.0 => "-",
            _ => "+",
        }
    } else if value.is_sign_negative() && value != 0.0 {
        "-"
    } else {
        ""
    };
    let unit = if spec["unit"] == "%" { "%" } else { "" };
    format!("{sign}{text}{unit}")
}

/// `value` in the page's compact notation: 1.2K, 12K, 123M, to two significant figures and never
/// fewer than its whole part.
fn compact(value: f64) -> String {
    let size = value.abs();
    let mut group = 0;
    while group < 4 && size >= 1000f64.powi(group + 1) {
        group += 1;
    }
    let text = loop {
        let scaled = size / 1000f64.powi(group);
        let (digits, power) = if scaled == 0.0 {
            ("0".to_owned(), 0)
        } else {
            significant(scaled, |power| (power + 1).max(2) as usize)
        };
        if power >= 3 && group < 4 {
            group += 1;
            continue;
        }
        let plain = match usize::try_from(power).map(|power| power + 1) {
            Ok(whole) if digits.len() <= whole => format!("{digits:0<whole$}"),
            Ok(whole) => format!("{}.{}", &digits[..whole], &digits[whole..]),
            Err(_) => format!("0.{}{digits}", "0".repeat((-power - 1) as usize)),
        };
        break plain;
    };
    let suffix = ["", "K", "M", "B", "T"][group as usize];
    let sign = if value < 0.0 { "-" } else { "" };
    format!("{sign}{text}{suffix}")
}

fn count_words(done: f64, of: Option<f64>, unit: &str) -> String {
    let done = number(done);
    let word = match unit {
        "" | "step" | "steps" => Some("step"),
        "round" | "rounds" => Some("round"),
        _ => None,
    };
    match (of.map(number), word) {
        (Some(of), Some(word)) => format!("{word} {done} of {of}"),
        (Some(of), None) => format!("{done} of {of} {unit}"),
        (None, _) if unit.is_empty() => format!("step {done}"),
        (None, _) => format!("{done} {unit}"),
    }
}

/// What a stage counts in, singular and lower case: "rounds" and "Round" are both "round", and no
/// unit is "step".
fn counted(text: &str) -> String {
    let text = if text.is_empty() { "step" } else { text };
    let text = text.to_lowercase();
    text.strip_suffix('s')
        .map_or_else(|| text.clone(), str::to_owned)
}

/// How a run's signals fall short of current and whole, in the words of the state they are in.
/// `reading` is whether anything will still read this run's log.
fn signal_notes(
    run: &str,
    reading: bool,
    availability: &Value,
    snapshot: Option<&Snapshot>,
) -> Vec<String> {
    let mut notes = Vec::new();
    let reported = snapshot.is_some_and(Snapshot::has_signals);
    if let Some(reason) = availability["reason"].as_str() {
        if availability["state"] == REFOLD_FAILED {
            notes.push(reason.to_owned());
            if let Some(at) = availability["retryAt"].as_i64() {
                notes.push(format!(
                    "Reading the saved log again at {}.",
                    crate::telemetry::iso8601_utc(at)
                ));
            }
        } else {
            notes.push(match availability["state"].as_str() {
                Some("incomplete") => format!(
                "Signal collection could not finish. Read any saved output with `orx logs {run}`."
            ),
                Some("unavailable") => format!(
                    "Signal data could not be read. Read any saved output with `orx logs {run}`."
                ),
                _ => match availability["retryAt"].as_i64() {
                    Some(at) => format!(
                        "Final signal collection is retrying; next attempt at {}.",
                        crate::telemetry::iso8601_utc(at)
                    ),
                    None => "Signal collection is delayed; orx keeps retrying.".to_owned(),
                },
            });
            notes.push(format!("Error details: {reason}"));
        }
    } else if availability["retryAt"].is_i64() {
        notes.push(READING_FINAL_LOG.to_owned());
    } else if !reported && reading {
        notes.push("No metrics found in the log yet.".to_owned());
    }
    if !reported {
        return notes;
    }
    if let Some(collected) = availability["lastCollectedAt"].as_i64() {
        notes.push(format!(
            "Last collected at {}.",
            crate::telemetry::iso8601_utc(collected)
        ));
    }
    let Some(snapshot) = snapshot else {
        return notes;
    };
    notes.push(format!(
        "Data as of {}.",
        crate::telemetry::iso8601_utc(snapshot.updated_at)
    ));
    // A settled collection marks the checkpoint too, unless a collection held it then.
    let settled = (availability["state"] == "incomplete")
        .then_some("collection_incomplete")
        .filter(|settled| !snapshot.coverage.reasons.iter().any(|r| r == settled));
    let reasons = snapshot.coverage.reasons.iter().map(String::as_str);
    for reason in reasons.chain(settled) {
        let text = match reason {
            "catching_up" | "rebuilding" | "log_replaced" if !reading => continue,
            "catching_up" | "rebuilding" | "log_replaced" => "Signal history is still being read",
            "truncated" => "Some signal history exceeded the retention limits",
            "missing_prefix" => "The beginning of the log is unavailable",
            "collection_incomplete" => "The run ended before all signals could be kept",
            "log_missing" => "The raw log is unavailable",
            "missing_middle" => "The saved log is missing bytes between its retained ranges",
            "missing_suffix" => "The end of the saved log is unavailable",
            _ => "Signal coverage is partial",
        };
        notes.push(format!("{text}. Inspect `orx logs {run}`."));
    }
    notes
}

/// A number no view gave digits, as the page writes it (`fmtMeasure`): to three significant
/// figures and never fewer than its whole part, and one too small or too large for that as
/// `3e-4`, with the sign and the `%` its view declares. A nonzero value never reads as zero.
pub(crate) fn measure(value: f64, spec: &Value) -> String {
    let size = value.abs();
    let text = if size == 0.0 {
        "0".to_owned()
    } else if (1e-3..1e15).contains(&size) {
        let (digits, power) = significant(size, |power| (power + 1).max(3) as usize);
        let plain = match usize::try_from(power).map(|power| power + 1) {
            Ok(whole) if digits.len() <= whole => format!("{digits:0<whole$}"),
            Ok(whole) => format!("{}.{}", &digits[..whole], &digits[whole..]),
            Err(_) => format!("0.{}{digits}", "0".repeat((-power - 1) as usize)),
        };
        number(plain)
    } else {
        let (digits, power) = significant(size, |_| 3);
        let (first, rest) = digits.split_at(1);
        let point = if rest.is_empty() { "" } else { "." };
        format!("{first}{point}{rest}e{power}")
    };
    let sign = if value < 0.0 {
        "-"
    } else if value > 0.0 && spec["sign"] == true {
        "+"
    } else {
        ""
    };
    let unit = if spec["unit"] == "%" { "%" } else { "" };
    format!("{sign}{text}{unit}")
}

/// A positive `size`'s shortest decimal, rounded half away from zero to the count of significant
/// digits `count` gives for its power of ten, as `Intl.NumberFormat` rounds: the digits without
/// trailing zeros, and the power of ten of the first.
fn significant(size: f64, count: impl Fn(i32) -> usize) -> (String, i32) {
    let shortest = format!("{size:e}");
    let (mantissa, power) = shortest.split_once('e').expect("exponent form");
    let mut power: i32 = power.parse().expect("an exponent");
    let all: Vec<u8> = mantissa.bytes().filter(u8::is_ascii_digit).collect();
    let count = count(power);
    let mut kept = all[..count.min(all.len())].to_vec();
    if all.get(count).is_some_and(|digit| *digit >= b'5') {
        match kept.iter().rposition(|digit| *digit != b'9') {
            Some(at) => {
                kept[at] += 1;
                kept.truncate(at + 1);
            }
            None => {
                kept = vec![b'1'];
                power += 1;
            }
        }
    }
    let kept = String::from_utf8(kept).expect("ASCII digits");
    (kept.trim_end_matches('0').to_owned(), power)
}

pub(super) fn number(value: impl std::fmt::Display) -> String {
    let raw = value.to_string();
    if raw.contains(['e', 'E']) {
        return raw;
    }
    let (integer, fraction) = raw
        .split_once('.')
        .map_or((raw.as_str(), None), |(a, b)| (a, Some(b)));
    let sign = if integer.starts_with('-') { "-" } else { "" };
    let digits = integer.trim_start_matches('-');
    let mut out = String::from(sign);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if let Some(fraction) = fraction {
        out.push('.');
        out.push_str(fraction);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::git::TemporaryDirectory;

    const T0: i64 = 1_794_444_300_000;

    struct Fixture {
        dir: TemporaryDirectory,
        store: Store,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let dir = TemporaryDirectory::new(name).unwrap();
            let store = Store::open_at(dir.path().to_path_buf()).unwrap();
            store
                .create_local_experiment(&crate::local::model::LocalExperiment {
                    id: "exp".into(),
                    project_id: "project".into(),
                    parent_experiment_id: None,
                    slug: "baseline".into(),
                    branch_name: "orx/baseline".into(),
                    title: None,
                    description: None,
                    run_command: "train".into(),
                    agent_status: "idle".into(),
                    created_at: 1,
                    updated_at: 1,
                    chat_session_id: None,
                    archived: false,
                })
                .unwrap();
            Self { dir, store }
        }

        fn run(&self, status: &str, backend: Value) -> StoredRun {
            self.run_named("r1", status, backend)
        }

        /// An experiment under `parent`, which starts with a copy of its view.
        fn child(&self, id: &str, parent: &str) {
            self.store
                .create_local_experiment(&crate::local::model::LocalExperiment {
                    id: id.into(),
                    project_id: "project".into(),
                    parent_experiment_id: Some(parent.into()),
                    slug: id.into(),
                    branch_name: format!("orx/{id}"),
                    title: None,
                    description: None,
                    run_command: "train".into(),
                    agent_status: "idle".into(),
                    created_at: 2,
                    updated_at: 2,
                    chat_session_id: None,
                    archived: false,
                })
                .unwrap();
        }

        /// Saves a view as `orx exp view set` does.
        fn view(&self, experiment: &str, markup: &str) -> Value {
            let raw = self.store.run_monitor_json(experiment).unwrap();
            let source = crate::engine::view::Source {
                file: Some("view.xml"),
                patterns: None,
                config: None,
            };
            crate::engine::view::set(
                &self.store,
                experiment,
                &crate::engine::view::etag(raw.as_deref()),
                Some(markup),
                None,
                None,
                &source,
            )
            .unwrap()
        }

        fn run_named(&self, id: &str, status: &str, backend: Value) -> StoredRun {
            self.run_in(id, "exp", status, backend)
        }

        fn run_in(&self, id: &str, experiment: &str, status: &str, backend: Value) -> StoredRun {
            let terminal = Phase::parse(status).is_some_and(Phase::is_terminal);
            let run = StoredRun {
                id: id.into(),
                experiment_id: experiment.into(),
                project_id: "project".into(),
                status: status.into(),
                backend_json: backend.to_string(),
                command: "train".into(),
                created_at: T0,
                updated_at: T0,
                ended_at: terminal.then_some(T0 + 600_000),
                exit_code: None,
                commit_sha: None,
                result_markdown: None,
                cancel_requested: false,
                cancel_reason: None,
                chat_session_id: None,
            };
            self.store.upsert_run(&run).unwrap();
            run
        }

        /// A checkpoint some other writer left, such as an earlier `orx up`.
        fn signals(&self, snapshot: &Snapshot) {
            let [path, _] = signal_paths(self.dir.path(), &snapshot.run_id);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            super::super::adapters::local::signals::publish(&path, snapshot).unwrap();
        }

        /// A checkpoint the laptop's collection publishes.
        fn collect(&self, snapshot: &Snapshot) {
            let [path, _] = signal_paths(self.dir.path(), &snapshot.run_id);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            super::super::adapters::signals::publish(&path, snapshot).unwrap();
        }

        fn status(&self, status: Value) {
            let [_, path] = signal_paths(self.dir.path(), "r1");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, status.to_string()).unwrap();
        }

        fn collected(&self, at: i64) {
            self.status(json!({"state": "cached", "reason": null, "lastCollectedAt": at}));
        }

        fn confirmed(&self, id: &str, at: i64) {
            let [checkpoint, status] = signal_paths(self.dir.path(), id);
            let mut value = json!({"state": "cached", "reason": null, "lastCollectedAt": at});
            if let Some(snapshot) =
                super::super::adapters::local::signals::load(&checkpoint).unwrap()
            {
                value["sample"] =
                    json!({"checkpoint": [snapshot.generation, snapshot.rev], "at": at});
            }
            std::fs::create_dir_all(status.parent().unwrap()).unwrap();
            std::fs::write(status, value.to_string()).unwrap();
        }

        /// The store refuses to save `run`'s monitor, as a full disk would.
        fn failing(&self, run: &str, failing: bool) {
            let db = rusqlite::Connection::open(self.dir.path().join("orx.db")).unwrap();
            let sql = if failing {
                format!(
                    "CREATE TRIGGER fail_insert BEFORE INSERT ON run_monitor_state WHEN NEW.run_id = '{run}' BEGIN SELECT RAISE(ABORT, 'database or disk is full'); END;
                     CREATE TRIGGER fail_update BEFORE UPDATE ON run_monitor_state WHEN NEW.run_id = '{run}' BEGIN SELECT RAISE(ABORT, 'database or disk is full'); END;"
                )
            } else {
                "DROP TRIGGER fail_insert; DROP TRIGGER fail_update;".to_owned()
            };
            db.execute_batch(&sql).unwrap();
        }

        /// What a restart of `orx up` forgets.
        fn restart(&self) {
            forget(&self.store, "r1").unwrap();
        }
    }

    fn local() -> Value {
        json!({"kind": "local_job"})
    }

    #[test]
    fn media_files_advance_a_run_without_a_lifecycle_or_log_change() {
        use crate::local::{files, model::LocalProject};
        use base64::Engine as _;
        use std::time::UNIX_EPOCH;

        let f = Fixture::new("media");
        let project = LocalProject {
            id: "project".into(),
            name: "Media".into(),
            slug: format!("media-{}", uuid::Uuid::new_v4()),
            repo_path: f.dir.path().to_string_lossy().into_owned(),
            github_owner: String::new(),
            github_repo: String::new(),
            github_sync_enabled: false,
            baseline_branch: "main".into(),
            run_command: None,
            paper_id: None,
            icon: None,
            proposal: None,
            created_at: 1,
            updated_at: 1,
        };
        f.store.create_local_project(&project).unwrap();
        let run = f.run("done", local());
        let saved = r#"<view><media glob="samples/*.png" last="1" /></view>"#;
        f.view("exp", saved);
        let base = files::files_dir(&project);
        let samples = base.join("runs/r1/samples");
        let glob = "samples/*.png";
        let first = publish(&f.store, &run, T0, None).unwrap().0;
        assert_eq!(first.body["media"][glob]["items"], json!([]));
        assert_eq!(first.body["media"][glob]["error"], Value::Null);

        std::fs::create_dir_all(&samples).unwrap();
        let file = samples.join("sample.png");
        let pic = base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAIAAAADCAIAAAA2iEnWAAAAC0lEQVR4nGNgwAIAABUAAapll8QAAAAASUVORK5CYII=").unwrap();
        std::fs::write(&file, &pic).unwrap();
        let stamp = std::fs::File::options().write(true).open(&file).unwrap();
        stamp
            .set_modified(UNIX_EPOCH + Duration::from_secs(123))
            .unwrap();
        let (ready, event) = publish(&f.store, &run, T0, None).unwrap();
        let media = &ready.body["media"];
        let contract: Value =
            serde_json::from_str(include_str!("../../tests/contracts/run-media.json")).unwrap();
        assert_eq!(media, &contract);
        assert_eq!(event.unwrap()["media"], *media);
        assert_eq!(first.published.series_rev, ready.published.series_rev);
        assert!(publish(&f.store, &run, T0, None).unwrap().1.is_none());

        let version = media[glob]["items"][0]["version"].as_str().unwrap();
        let path = "samples/sample.png";
        let opened = super::super::media::open_preview(&f.store, &run, path, version).unwrap();
        assert_eq!(
            opened
                .as_ref()
                .map(|(mime, bytes)| (*mime, bytes.as_slice())),
            Some(("image/png", pic.as_slice()))
        );
        let artifact_stamp = files::fingerprint(&project);
        stamp
            .set_modified(UNIX_EPOCH + Duration::from_micros(123_000_500))
            .unwrap();
        assert_ne!(files::fingerprint(&project), artifact_stamp);
        assert!(
            super::super::media::open_preview(&f.store, &run, path, version)
                .unwrap()
                .is_none()
        );

        let changed = publish(&f.store, &run, T0, None).unwrap().0;
        assert!(changed.published.rev > ready.published.rev);
        assert_ne!(changed.body["media"], *media);
        drop(stamp);
        std::fs::remove_file(file).unwrap();
        let deleted = publish(&f.store, &run, T0, None).unwrap().0;
        assert_eq!(deleted.body["media"], first.body["media"]);

        for kind in ["ssh_job", "slurm_job", "modal_job"] {
            let remote = f.run_named(kind, "done", json!({"kind": kind}));
            let remote = publish(&f.store, &remote, T0, None).unwrap().0;
            assert_eq!(
                remote.body["media"][glob]["error"],
                json!({"kind": "unsupported", "backend": kind})
            );
        }
        std::fs::write(samples.join("not-a-directory.png"), b"example").unwrap();
        f.view(
            "exp",
            r#"<view><media glob="samples/not-a-directory.png/*.png" /></view>"#,
        );
        let unreadable = publish(&f.store, &run, T0, None).unwrap().0;
        assert_eq!(
            unreadable.body["media"]["samples/not-a-directory.png/*.png"]["error"]["kind"],
            "unreadable"
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    /// A payload declaring `of` steps, one report a second from `T0`, loss falling, a NaN at 50.
    fn declared(steps: std::ops::RangeInclusive<i64>, of: i64) -> Vec<(i64, String)> {
        steps
            .map(|i| {
                let loss = if i == 5 {
                    "NaN".to_owned()
                } else {
                    format!("{}", 3.0 / (i + 1) as f64)
                };
                (
                    T0 + i * 1000,
                    format!(
                        "::orx:: {{\"step\":{},\"of\":{of},\"t\":{i},\"m\":{{\"loss\":{loss}}}}}",
                        i * 10
                    ),
                )
            })
            .collect()
    }

    fn fold(snapshot: &mut Snapshot, lines: &[(i64, String)]) {
        for (at, line) in lines {
            snapshot
                .feed(format!("{line}\n").as_bytes(), false, *at, true)
                .unwrap();
            snapshot.reached_end(*at);
        }
    }

    fn folded(lines: &[(i64, String)]) -> Snapshot {
        let mut snapshot = Snapshot::new("r1", "1-1", "g1");
        fold(&mut snapshot, lines);
        snapshot
    }

    fn mentions(text: &str, parts: &[&str]) {
        for part in parts {
            assert!(text.contains(*part), "missing {part:?} in {text:?}");
        }
    }

    fn status_line<'a>(body: &'a Value, label: &str) -> &'a str {
        body["statusText"]
            .as_str()
            .unwrap()
            .lines()
            .find_map(|line| {
                let (name, text) = line.split_once(' ')?;
                (name == label).then_some(text.trim_start())
            })
            .unwrap_or_default()
    }

    fn monitor_at(f: &Fixture, run: &StoredRun, now: i64) -> (Value, Option<Value>) {
        let (projection, event) = publish(&f.store, run, now, None).unwrap();
        let mut monitor = projection.body.clone();
        monitor["series"] = series_json(projection.snapshot.as_deref(), None);
        (monitor, event)
    }

    #[test]
    fn a_declared_run_shows_progress_time_left_and_its_curve() {
        let f = Fixture::new("orx-monitor-declared");
        let run = f.run("running", local());
        f.signals(&folded(&declared(0..=20, 400)));
        f.collected(T0 + 20_000);
        let (monitor, _) = monitor_at(&f, &run, T0 + 20_500);

        assert_eq!(monitor["availability"]["state"], "live");
        assert_eq!(monitor["source"], "declared");
        assert_eq!(monitor["progress"]["done"], 200.0);
        assert_eq!(monitor["progress"]["segments"][0]["fill"], 0.5);
        assert_eq!(monitor["x"], json!({"name": "step", "max": 400.0}));
        assert_eq!(monitor["view"]["headline"], json!(["loss"]));
        assert_eq!(
            monitor["view"]["blocks"][0],
            json!({"chart": ["loss"], "ghost": "parent"})
        );
        assert_eq!(monitor["viewFrom"]["isDefault"], true);
        let estimate = &monitor["estimate"];
        assert_eq!(estimate["pending"], false, "{estimate}");
        let (lo, hi) = (
            estimate["loMs"].as_i64().unwrap(),
            estimate["hiMs"].as_i64().unwrap(),
        );
        assert!(lo < 20_000 && 20_000 < hi, "{estimate}");
        let loss = monitor["series"]["loss"].as_array().unwrap();
        assert_eq!(loss.len(), 21);
        assert_eq!(loss[5], json!([50.0, null]), "a NaN is a gap");
        assert_eq!(
            monitor["cost"],
            Value::Null,
            "a local run costs nothing to show"
        );
        let text = monitor["statusText"].as_str().unwrap();
        let header = text.lines().next().unwrap();
        mentions(
            header,
            &["baseline", "running", "50%", "step 200 of 400", "[run r1]"],
        );
        mentions(status_line(&monitor, "eta"), &["left", " to "]);
        assert_eq!(status_line(&monitor, "signals"), "loss 0.143");
        let stamp = "2026-11-12T00:45:20.000Z";
        assert!(
            text.lines()
                .any(|line| line.contains("collected") && line.contains(stamp)),
            "{text}"
        );
        assert!(
            text.lines()
                .any(|line| line.contains("Data as of") && line.contains(stamp)),
            "{text}"
        );
        mentions(text, &["1 log diagnostics", "orx logs r1"]);

        let summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
        assert_eq!(summary["tone"], "running");
        assert_eq!(summary["whole"], 0.5);
        assert_eq!(summary["headline"]["name"], "loss");
        assert_eq!(summary["estimate"]["midMs"], estimate["midMs"]);
        assert_eq!(summary["rev"], monitor["rev"]);
    }

    #[test]
    fn a_nan_reads_nan_and_strings_never_head_a_chart() {
        let mut snapshot = folded(&[(
            T0,
            "::orx:: {\"m\":{\"note\":\"warmup\",\"loss\":NaN}}".into(),
        )]);
        let view = default_view(Some(&snapshot));
        assert_eq!(view["headline"], json!(["loss"]));
        assert_eq!(
            view["blocks"],
            json!([{"chart": ["loss"], "ghost": "parent"}, {"stats": ["note"]}])
        );
        assert_eq!(snapshot.latest["loss"], "NaN");

        snapshot = folded(&[(T0, "::orx:: {\"m\":{\"note\":\"warmup\"}}".into())]);
        let view = default_view(Some(&snapshot));
        assert_eq!(view["headline"], json!([]));
        assert_eq!(view["blocks"], json!([{"stats": ["note"]}]));

        let view = default_view(Some(&Snapshot::new("r1", "1-1", "g1")));
        assert_eq!(view["blocks"], json!([]), "stages alone before any metric");

        let snapshot = folded(&[(
            T0,
            "::orx:: {\"step\":3,\"m\":{\"Steps\":3,\"loss\":2}}".into(),
        )]);
        let view = default_view(Some(&snapshot));
        assert_eq!(
            view["headline"],
            json!(["loss"]),
            "the stage line says the counter"
        );
    }

    #[test]
    fn the_default_view_heads_with_a_measure_of_its_own_then_its_tables() {
        let view = |lines: &[(i64, &str)]| {
            let lines: Vec<(i64, String)> = lines
                .iter()
                .map(|(at, line)| (*at, format!("::orx:: {line}")))
                .collect();
            let snapshot = folded(&lines);
            let view = default_view(Some(&snapshot));
            let restored: Snapshot =
                serde_json::from_value(serde_json::to_value(&snapshot).unwrap()).unwrap();
            assert_eq!(
                default_view(Some(&restored)),
                view,
                "a restart draws the same"
            );
            (view["headline"].clone(), view["blocks"].clone())
        };

        // A loss every step and an eval row every five: the loss, then its table, never row stats.
        let mut training: Vec<(i64, String)> = Vec::new();
        for step in 1..=10 {
            training.push((
                T0 + step * 1000,
                format!(
                    r#"{{"step":{step},"of":40,"m":{{"loss":{}}}}}"#,
                    2.0 / step as f64
                ),
            ));
            if step % 5 == 0 {
                training.push((
                    T0 + step * 1000,
                    format!(
                        r#"{{"row":"evals","m":{{"epoch":{},"val_loss":0.5,"val_acc":0.8}}}}"#,
                        step / 5
                    ),
                ));
            }
        }
        let training: Vec<(i64, &str)> = training.iter().map(|(at, l)| (*at, l.as_str())).collect();
        assert_eq!(
            view(&training),
            (
                json!(["loss"]),
                json!([
                    {"chart": ["loss"], "ghost": "parent"},
                    {"table": "evals", "columns": ["epoch", "val_loss", "val_acc"]},
                ])
            )
        );

        // A row printed first still leaves the headline to a measure of the run's own, and its
        // other measures are stats; a row's column that jumps never heads it.
        assert_eq!(
            view(&[
                (T0, r#"{"row":"tasks","m":{"task":"gsm8k","score":0.4}}"#),
                (T0 + 1000, r#"{"m":{"mean_score":0.4,"done_tasks":1}}"#),
                (T0 + 2000, r#"{"row":"eval","m":{"split":"dev","acc":0.7}}"#),
            ]),
            (
                json!(["mean_score"]),
                json!([
                    {"chart": ["mean_score"], "ghost": "parent"},
                    {"table": "tasks", "columns": ["task", "score"]},
                    {"table": "eval", "columns": ["split", "acc"]},
                    {"stats": ["done_tasks"]},
                ])
            )
        );

        // Rows only: the table first and a column as the headline, as before.
        assert_eq!(
            view(&[(T0, r#"{"row":"eval","m":{"task":"gsm8k","score":0.4}}"#)]),
            (
                json!(["score"]),
                json!([
                    {"table": "eval", "columns": ["task", "score"]},
                    {"stats": ["task"]},
                ])
            )
        );

        // A counter and one metric: the metric, charted over the counter.
        assert_eq!(
            view(&[(
                T0,
                r#"{"phase":"round","unit":"rounds","step":3,"of":10,"m":{"round":3,"loss":2}}"#
            )]),
            (
                json!(["loss"]),
                json!([{"chart": ["loss"], "ghost": "parent"}, {"stats": ["round"]}])
            )
        );

        // Strings only: no headline, every value a stat.
        assert_eq!(
            view(&[(T0, r#"{"m":{"model":"llama","split":"dev"}}"#)]),
            (json!([]), json!([{"stats": ["model", "split"]}]))
        );
    }

    #[test]
    fn rev_only_grows_and_survives_a_restart() {
        let f = Fixture::new("orx-monitor-rev");
        let run = f.run("running", local());
        f.signals(&folded(&declared(0..=10, 400)));
        f.collected(T0 + 10_000);
        let (first, event) = monitor_at(&f, &run, T0 + 10_000);
        assert_eq!(first["rev"], 1);
        assert_eq!(
            event.unwrap()["reset"],
            true,
            "nothing cached can be extended yet"
        );

        let (same, event) = monitor_at(&f, &run, T0 + 10_500);
        assert_eq!(
            (same["rev"].clone(), event),
            (json!(1), None),
            "at most one a second"
        );
        let (idle, event) = monitor_at(&f, &run, T0 + 15_000);
        assert_eq!(
            (idle["rev"].clone(), event),
            (json!(1), None),
            "nothing changed"
        );

        // Time alone moves a live run's cost and stall.
        let (ticked, event) = monitor_at(&f, &run, T0 + 25_000);
        assert_eq!(ticked["rev"], 2);
        let event = event.unwrap();
        assert_eq!(
            (event["seriesRev"].clone(), event["append"].clone()),
            (first["seriesRev"].clone(), json!({}))
        );

        f.restart();
        let (again, event) = monitor_at(&f, &run, T0 + 25_000);
        assert_eq!(
            (again["rev"].clone(), event),
            (json!(2), None),
            "the stored pointer stands"
        );

        // The fold published while orx up was down: one reset repairs the pointer.
        let mut snapshot = folded(&declared(0..=10, 400));
        fold(&mut snapshot, &declared(11..=12, 400));
        f.signals(&snapshot);
        f.restart();
        let (repaired, event) = monitor_at(&f, &run, T0 + 25_000);
        assert_eq!(repaired["rev"], 3);
        assert_eq!(event.unwrap()["reset"], true);
        assert!(repaired["seriesRev"].as_u64() > ticked["seriesRev"].as_u64());
        let state = f.store.run_monitor_state("r1").unwrap().unwrap();
        assert_eq!(state["published"]["summary"]["rev"], 3);
        assert_eq!(state["published"]["foldRev"], snapshot.rev);
    }

    #[test]
    fn new_points_append_on_the_same_basis_and_reset_otherwise() {
        let f = Fixture::new("orx-monitor-append");
        let run = f.run("running", local());
        let mut snapshot = folded(&declared(0..=10, 400));
        f.signals(&snapshot);
        f.collected(T0 + 10_000);
        let (first, _) = monitor_at(&f, &run, T0 + 10_000);

        fold(&mut snapshot, &declared(11..=12, 400));
        f.signals(&snapshot);
        let (next, event) = monitor_at(&f, &run, T0 + 12_000);
        let event = event.unwrap();
        assert_eq!(event.get("reset"), None);
        assert_eq!(event["appendFrom"], first["seriesRev"]);
        assert_eq!(event["seriesRev"], next["seriesRev"]);
        assert_eq!(
            event["append"]["loss"],
            json!(snapshot.series["loss"][11..])
        );
        assert_eq!(event["latest"]["loss"], snapshot.latest["loss"]);

        // A relaunch from an older step drops the redone points: the page reads the run again.
        fold(
            &mut snapshot,
            &[(
                T0 + 13_000,
                "::orx:: {\"step\":60,\"of\":400,\"t\":0,\"m\":{\"loss\":1.5}}".into(),
            )],
        );
        f.signals(&snapshot);
        let (relaunched, event) = monitor_at(&f, &run, T0 + 14_000);
        assert_eq!(event.unwrap()["reset"], true);
        let steps: Vec<f64> = relaunched["series"]["loss"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p[0].as_f64().unwrap())
            .collect();
        assert_eq!(
            steps.iter().filter(|&&x| x == 60.0).count(),
            1,
            "each step once: {steps:?}"
        );

        f.signals(&Snapshot {
            generation: "g2".into(),
            ..snapshot.clone()
        });
        let (_, event) = monitor_at(&f, &run, T0 + 16_000);
        assert_eq!(
            event.unwrap()["reset"],
            true,
            "another generation is another basis"
        );
    }

    #[test]
    fn a_same_view_stage_change_reaches_the_open_monitor() {
        let f = Fixture::new("orx-monitor-stage-event");
        let run = f.run("running", local());
        f.view("exp", VIEW);
        let mut snapshot = folded(&[(
            T0,
            "::orx:: {\"phase\":\"train\",\"step\":1,\"m\":{\"loss\":2}}".into(),
        )]);
        f.signals(&snapshot);
        f.collected(T0);
        let (before, _) = monitor_at(&f, &run, T0);

        fold(
            &mut snapshot,
            &[(T0 + 1000, "::orx:: {\"phase\":\"eval\",\"step\":0}".into())],
        );
        f.signals(&snapshot);
        f.collected(T0 + 1000);
        let (after, event) = monitor_at(&f, &run, T0 + 1000);
        let event = event.unwrap();
        assert_eq!(before["view"], after["view"]);
        assert_ne!(before["stageOrder"], after["stageOrder"]);
        assert_eq!(event.get("reset"), None);
        assert_eq!(event["stageOrder"], after["stageOrder"]);
    }

    #[test]
    fn time_the_laptop_did_not_watch_is_unknown() {
        let f = Fixture::new("orx-monitor-stale");
        let run = f.run("running", local());
        f.signals(&folded(&declared(0..=10, 400)));
        f.collected(T0 + 10_000);
        let (live, _) = monitor_at(&f, &run, T0 + 10_000);
        let (asleep, _) = monitor_at(&f, &run, T0 + 3_600_000);
        assert_eq!(asleep["availability"]["state"], "stale");
        let reason = asleep["availability"]["reason"].as_str().unwrap();
        mentions(reason, &["No update", "00:45 UTC"]);
        assert_eq!(asleep["asOf"], T0 + 10_000);
        let projection = current(&f.store, &run).unwrap().unwrap();
        let summary = projection.summary();
        assert_eq!(summary["availability"]["state"], "stale");
        assert_eq!(summary["availability"]["reason"], reason);
        assert_eq!(summary["asOf"], asleep["asOf"]);
        assert_eq!(
            asleep["estimate"], live["estimate"],
            "nothing advances across the gap"
        );
        assert_eq!(asleep["progress"]["stallMs"], live["progress"]["stallMs"]);
        mentions(
            status_line(&asleep, "eta"),
            &["Last seen", "step 100 of 400"],
        );

        f.collected(T0 + 3_601_000);
        let (awake, _) = monitor_at(&f, &run, T0 + 3_601_000);
        assert_eq!(awake["availability"]["state"], "live");
        let projection = current(&f.store, &run).unwrap().unwrap();
        assert!(projection.summary().get("availability").is_none());
        assert!(
            awake["progress"]["stallMs"].as_i64().unwrap() > 3_000_000,
            "a collection shows the silence"
        );
        let eta = status_line(&awake, "eta");
        mentions(eta, &["No progress", "59m", "due", "00:45 UTC"]);
        assert!(!eta.contains("0s left"), "{eta}");
    }

    #[test]
    fn each_degraded_source_says_so() {
        let f = Fixture::new("orx-monitor-degraded");
        let run = f.run("running", local());
        let mut snapshot = folded(&declared(0..=10, 400));
        f.signals(&snapshot);
        f.status(json!({"state": "stale", "reason": "The host did not answer", "lastCollectedAt": T0 + 9_000}));
        let (stale, _) = monitor_at(&f, &run, T0 + 10_000);
        assert_eq!(
            stale["availability"],
            json!({"state": "stale", "reason": "The host did not answer", "omitted": {}})
        );
        mentions(
            stale["statusText"].as_str().unwrap(),
            &["collection", "delayed", "retry"],
        );

        snapshot.coverage.add_reason("rebuilding");
        f.signals(&snapshot);
        f.collected(T0 + 11_000);
        let (rebuilding, _) = monitor_at(&f, &run, T0 + 12_000);
        assert_eq!(
            rebuilding["availability"]["state"], "rebuilding",
            "never lost contact"
        );
        let projection = current(&f.store, &run).unwrap().unwrap();
        let summary = projection.summary();
        assert_eq!(summary["availability"]["state"], "rebuilding");
        assert_eq!(
            summary["availability"]["reason"],
            rebuilding["availability"]["reason"]
        );

        let [path, _] = signal_paths(f.dir.path(), "r1");
        std::fs::write(path, "{not json").unwrap();
        let (unreadable, _) = monitor_at(&f, &run, T0 + 14_000);
        assert_eq!(unreadable["availability"]["state"], "unavailable");
        assert!(unreadable["availability"]["reason"]
            .as_str()
            .unwrap()
            .contains("r1.json"));
        assert_eq!(unreadable["progress"], Value::Null, "unknown is not zero");
        let summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
        assert_eq!(summary["unavailable"], SIGNALS_UNREADABLE, "{summary}");
        let text = unreadable["statusText"].as_str().unwrap();
        assert!(!text.contains("no progress found"), "{text}");
    }

    #[test]
    fn an_open_ended_run_fills_against_its_time_limit() {
        let f = Fixture::new("orx-monitor-limit");
        let run = f.run("running", json!({"kind": "slurm_job", "timeoutSecs": 3600}));
        f.signals(&folded(&[(
            T0 + 1000,
            "::orx:: {\"step\":5,\"m\":{\"loss\":2}}".into(),
        )]));
        f.collected(T0 + 900_000);
        let (monitor, _) = monitor_at(&f, &run, T0 + 900_000);
        assert_eq!(monitor["limit"], json!({"endsAt": T0 + 3_600_000}));
        assert_eq!(monitor["estimate"], Value::Null, "no total, no time left");
        assert_eq!(monitor["progress"]["whole"], 0.25);
        assert_eq!(monitor["x"]["max"], Value::Null);
        let text = monitor["statusText"].as_str().unwrap();
        mentions(text.lines().next().unwrap(), &["25%", "step 5", "[run r1]"]);
        mentions(status_line(&monitor, "eta"), &["45m", "limit", "01:45 UTC"]);
        let summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
        assert_eq!(summary["limitEndsAt"], T0 + 3_600_000);
        assert_eq!(summary["estimate"], Value::Null);
    }

    #[test]
    fn status_reports_the_lifecycle_and_elapsed_time() {
        let cases = [
            ("starting", "starting", "starting", &["Not started"][..]),
            ("done", "done", "done", &["Done", "10m"]),
            ("failed", "failed", "failed", &["Failed", "10m"]),
            ("lost", "failed", "lost", &[]),
            ("cancelled", "stopped", "Cancelled", &["Stopped", "10m"]),
        ];
        for (status, tone, label, timing) in cases {
            let f = Fixture::new("orx-monitor-words");
            let run = f.run(status, local());
            if Phase::parse(status).is_some_and(Phase::is_terminal) {
                f.signals(&folded(&[(T0, "::orx:: {\"step\":100,\"of\":100}".into())]));
                f.collected(T0);
            }
            let (monitor, _) = monitor_at(&f, &run, T0 + 700_000);
            let text = monitor["statusText"].as_str().unwrap();
            let header = text.lines().next().unwrap();
            mentions(header, &["baseline", label, "[run r1]"]);
            if matches!(status, "lost" | "cancelled") {
                mentions(header, &["100%", "step 100 of 100"]);
            }
            mentions(status_line(&monitor, "eta"), timing);
            assert_eq!(monitor["estimate"], Value::Null, "{status}");
            let summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
            assert_eq!(summary["tone"], tone, "{status}");
            assert_eq!(
                monitor["availability"]["state"], "live",
                "{status}: an ended run has nothing more to wait for"
            );
        }

        let f = Fixture::new("orx-monitor-open");
        let run = f.run("running", local());
        let (monitor, _) = monitor_at(&f, &run, T0 + 5_000);
        let text = monitor["statusText"].as_str().unwrap();
        mentions(
            text.lines().next().unwrap(),
            &["baseline", "running", "[run r1]"],
        );
        mentions(status_line(&monitor, "eta"), &["Setting up"]);
        mentions(status_line(&monitor, "signals"), &["No metrics"]);
        assert_eq!(monitor["progress"], Value::Null);
        let summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
        assert_eq!(summary["whole"], Value::Null, "unknown is not done");

        let f = Fixture::new("orx-monitor-pending");
        let run = f.run("running", local());
        f.signals(&folded(&[(
            T0 + 1000,
            "::orx:: {\"step\":0,\"of\":400,\"t\":0}".into(),
        )]));
        f.collected(T0 + 1000);
        let (monitor, _) = monitor_at(&f, &run, T0 + 2000);
        assert_eq!(monitor["estimate"]["pending"], true);
        mentions(
            status_line(&monitor, "eta"),
            &["Estimating", "timed progress"],
        );
    }

    #[test]
    fn a_run_that_has_said_nothing_orx_recognises_shows_its_newest_line() {
        let key = "hf_abcdefghijklmnopqrstuvwxyz0123456";
        let setting_up = |name: &str, lines: &[&str], status: &str| {
            let f = Fixture::new(&format!("orx-monitor-setup-{name}"));
            let run = f.run(status, local());
            if !lines.is_empty() {
                let lines: Vec<_> = lines.iter().map(|l| (T0 + 1000, (*l).to_owned())).collect();
                f.collect(&folded(&lines));
                f.collected(T0 + 1000);
            }
            let (monitor, _) = monitor_at(&f, &run, T0 + 5000);
            let summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
            assert_eq!(summary["setup"], monitor["setup"], "{name}");
            monitor["setup"].clone()
        };

        let guess = |line: Value| json!({"line": line, "stage": null, "startsAt": null});
        assert_eq!(setting_up("silent", &[], "running"), guess(Value::Null));
        assert_eq!(
            setting_up(
                "printing",
                &["uv sync", "Computing initial pair counts"],
                "running"
            ),
            guess(json!("Computing initial pair counts"))
        );
        assert_eq!(
            setting_up(
                "secret",
                &["uv sync", &format!("export HF_TOKEN={key}")],
                "running"
            ),
            guess(Value::Null),
            "a credential is never shown"
        );
        assert_eq!(
            setting_up(
                "stage",
                &[
                    "Using CPython 3.12",
                    "::orx:: {\"phase\":\"install\"}",
                    "Resolved 120 packages"
                ],
                "running"
            ),
            json!({"line": "Resolved 120 packages", "stage": "install", "startsAt": null}),
            "a declared stage with no step is that stage for as long as it lasts"
        );
        assert_eq!(
            setting_up(
                "begun",
                &["Using CPython 3.12", "::orx:: {\"phase\":\"install\"}"],
                "running"
            ),
            json!({"line": null, "stage": "install", "startsAt": null}),
            "a line from before the stage is not what it is doing"
        );
        assert_eq!(
            setting_up(
                "stepped",
                &["::orx:: {\"phase\":\"download\",\"step\":0,\"of\":9}"],
                "running"
            ),
            Value::Null
        );
        assert_eq!(
            setting_up("progress", &["Epoch 1/10", "Epoch 2/10"], "running"),
            Value::Null
        );
        assert_eq!(
            setting_up(
                "declared",
                &["uv sync", "::orx:: {\"m\":{\"loss\":2.5}}"],
                "running"
            ),
            Value::Null
        );
        assert_eq!(
            setting_up(
                "rows",
                &[
                    "::orx:: {\"phase\":\"bench\"}",
                    "::orx:: {\"row\":\"cases\",\"m\":{\"n\":1000}}"
                ],
                "running"
            ),
            Value::Null,
            "a table's headline point starts the run"
        );
        assert_eq!(setting_up("ended", &["uv sync"], "done"), Value::Null);
    }

    #[test]
    fn an_open_page_follows_the_newest_line_of_a_run_setting_up() {
        let f = Fixture::new("orx-monitor-setup-events");
        let run = f.run("running", local());
        let mut snapshot = folded(&[(T0 + 1000, "uv sync".into())]);
        f.collect(&snapshot);
        f.collected(T0 + 1000);
        monitor_at(&f, &run, T0 + 2000);

        fold(
            &mut snapshot,
            &[(T0 + 3000, "Computing initial pair counts".into())],
        );
        f.collect(&snapshot);
        f.collected(T0 + 3000);
        let (_, event) = monitor_at(&f, &run, T0 + 4000);

        assert_eq!(
            event.unwrap()["setup"],
            json!({"line": "Computing initial pair counts", "stage": null, "startsAt": null})
        );
    }

    #[test]
    fn a_run_that_reports_no_progress_is_promised_no_estimate() {
        let slurm = json!({"kind": "slurm_job", "timeoutSecs": 3600});
        let metrics = folded(&[(T0 + 1000, "::orx:: {\"m\":{\"loss\":2.5}}".into())]);
        for (name, backend, reported) in [
            ("silent", local(), None),
            ("silent-limit", slurm.clone(), None),
            ("metrics", local(), Some(&metrics)),
            ("metrics-limit", slurm, Some(&metrics)),
        ] {
            let f = Fixture::new(&format!("orx-monitor-no-progress-{name}"));
            let limited = backend["timeoutSecs"].is_number();
            let run = f.run("running", backend);
            if let Some(snapshot) = reported {
                f.collect(snapshot);
                f.collected(T0 + 1000);
            }
            for after in [10_000, 1_000_000] {
                // Collection keeps up; only progress is missing.
                if reported.is_some() {
                    f.collected(T0 + after);
                }
                let (monitor, _) = monitor_at(&f, &run, T0 + after);
                assert_eq!(monitor["estimate"], Value::Null, "{name} {after}");
                let summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
                assert_eq!(summary["estimate"], Value::Null, "{name} {after}");
                let eta = status_line(&monitor, "eta");
                if limited {
                    let left = short(3_600_000 - after);
                    mentions(eta, &[&left, "limit", "01:45 UTC"]);
                } else if reported.is_none() {
                    mentions(eta, &["Setting up"]);
                } else {
                    mentions(eta, &["No estimate", "no progress"]);
                }
            }
        }

        // A declared total is reported: the estimate waits on its pace.
        let f = Fixture::new("orx-monitor-no-progress-declared");
        let run = f.run("running", local());
        f.collect(&folded(&declared(0..=0, 400)));
        f.collected(T0);
        let (monitor, _) = monitor_at(&f, &run, T0 + 5_000);
        assert_eq!(monitor["estimate"]["pending"], true);
        mentions(
            status_line(&monitor, "eta"),
            &["Estimating", "timed progress"],
        );
        assert_eq!(
            current(&f.store, &run).unwrap().unwrap().summary()["estimate"]["pending"],
            true
        );
    }

    #[test]
    fn a_failing_run_stays_watched_until_a_projection_succeeds() {
        let f = Fixture::new("orx-monitor-watched");
        f.run("running", local());
        f.collect(&folded(&declared(0..=20, 400)));
        f.collected(T0 + 20_000);
        let watched = watch(&f.store, None, BTreeSet::new(), None).unwrap();
        assert!(watched.contains("r1"), "a live run is watched");

        f.failing("r1", true);
        f.collect(&folded(&declared(0..=30, 400)));
        let watched = watch(&f.store, None, watched, None).unwrap();
        assert!(watched.contains("r1"));
        let run = f.run("done", local());
        let watched = watch(&f.store, Some(run.id.clone()), watched, None).unwrap();
        assert!(
            watched.contains("r1"),
            "an ended run whose settle failed stays watched"
        );
        let watched = watch(&f.store, None, watched, None).unwrap();
        assert!(
            watched.contains("r1"),
            "and is projected again on the next tick"
        );
        assert_eq!(
            current(&f.store, &run).unwrap().unwrap().summary()["tone"],
            "done"
        );

        f.failing("r1", false);
        let watched = watch(&f.store, None, watched, None).unwrap();
        assert!(
            !watched.contains("r1"),
            "a settle that succeeds ends the watch"
        );
        let settled = current(&f.store, &run).unwrap().unwrap();
        assert!(settled.summary().get("unavailable").is_none());
        f.restart();
        assert_eq!(
            current(&f.store, &run).unwrap().unwrap().summary()["tone"],
            "done"
        );
    }

    #[test]
    fn host_times_are_shown_on_the_laptops_clock() {
        const HOUR: i64 = 3_600_000;
        let f = Fixture::new("orx-monitor-clock");
        let run = f.run("running", local());
        let host: Vec<(i64, String)> = declared(0..=20, 400)
            .into_iter()
            .map(|(at, line)| (at + HOUR, line))
            .collect();
        f.signals(&folded(&host));
        f.status(json!({"state": "cached", "reason": null, "lastCollectedAt": T0 + 20_000, "clock": [T0 + HOUR + 20_000, T0 + 20_000]}));
        let (skewed, _) = monitor_at(&f, &run, T0 + 20_500);
        let f2 = Fixture::new("orx-monitor-clock-same");
        let run2 = f2.run("running", local());
        f2.signals(&folded(&declared(0..=20, 400)));
        f2.collected(T0 + 20_000);
        let (same, _) = monitor_at(&f2, &run2, T0 + 20_500);
        assert_eq!(skewed["estimate"], same["estimate"]);
        assert_eq!(
            skewed["phases"][0]["startedAt"],
            same["phases"][0]["startedAt"]
        );
        assert_eq!(skewed["progress"]["lastAt"], same["progress"]["lastAt"]);
    }

    /// The tick and every `Run` read the cache; the checkpoint is parsed only when it changes.
    #[test]
    fn an_unchanged_checkpoint_is_not_read_again() {
        let f = Fixture::new("orx-monitor-bound");
        let run = f.run("running", local());
        let mut snapshot = folded(&declared(0..=10, 400));
        f.signals(&snapshot);
        f.collected(T0 + 10_000);
        let held = |at: i64| {
            publish(&f.store, &run, at, None)
                .unwrap()
                .0
                .snapshot
                .clone()
                .unwrap()
        };
        let first = held(T0 + 10_000);
        for second in 1..120 {
            let later = held(T0 + 10_000 + second * 500);
            assert!(Arc::ptr_eq(&first, &later), "read again at {second}");
            assert!(Arc::ptr_eq(
                &first,
                current(&f.store, &run)
                    .unwrap()
                    .unwrap()
                    .snapshot
                    .as_ref()
                    .unwrap()
            ));
        }
        fold(&mut snapshot, &declared(11..=11, 400));
        f.signals(&snapshot);
        assert!(
            !Arc::ptr_eq(&first, &held(T0 + 80_000)),
            "a new checkpoint is read"
        );
    }

    /// A finished bar fills the whole underline: the end is evaluated where the run ended.
    #[test]
    fn an_ended_run_fills_its_bar() {
        let f = Fixture::new("orx-monitor-ended-bar");
        let run = f.run("done", local());
        let mut snapshot = Snapshot::new("r1", "1-1", "g1");
        for i in 0..=8 {
            let frame = format!(
                "\rtrain: {:3}%|{:8}| {i}/8 [00:0{i}<00:0{}, 1.00it/s]",
                i * 100 / 8,
                "#".repeat(i as usize),
                8 - i
            );
            snapshot
                .feed(frame.as_bytes(), false, T0 + i * 1000, true)
                .unwrap();
            snapshot.reached_end(T0 + i * 1000);
        }
        snapshot.feed(b"\n", true, T0 + 9000, true).unwrap();
        f.signals(&snapshot);
        f.collected(T0 + 9000);
        let (monitor, _) = monitor_at(&f, &run, T0 + 700_000);
        assert_eq!(monitor["progress"]["whole"], 1.0, "{}", monitor["progress"]);
        assert_eq!(monitor["progress"]["segments"][0]["fill"], 1.0);
        let summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
        assert_eq!(summary["whole"], 1.0);

        // Its last read landed after it ended, before the final fold.
        let mut late = snapshot.clone();
        late.extra.remove("terminal");
        late.updated_at = T0 + 900_000;
        f.signals(&late);
        let (monitor, _) = monitor_at(&f, &run, T0 + 901_000);
        assert_eq!(monitor["progress"]["whole"], 1.0, "{}", monitor["progress"]);
    }

    #[test]
    fn an_ended_run_reports_pending_final_collection_until_its_signals_land() {
        let f = Fixture::new("orx-monitor-final-log");
        let run = f.run("done", local());
        let now = T0 + 700_000;
        f.store.queue_signal_collection(&run.id).unwrap();

        let ended = run.ended_at.unwrap();
        let (early, _) = monitor_at(&f, &run, ended + 14_999);
        assert_eq!(early["availability"]["state"], "stale");
        assert!(early["availability"]["reason"].is_null());
        assert!(!early["statusText"]
            .as_str()
            .unwrap()
            .contains("No metrics found"));
        let (late, event) = monitor_at(&f, &run, ended + 15_000);
        assert_eq!(late["availability"]["reason"], READING_FINAL_LOG);
        assert!(
            event.is_some(),
            "the view hears about the delay even while collection is idle"
        );

        let (_, published) = monitor(&f.store, &run).unwrap();
        let offline = offline(&f.store, &run, now).unwrap();
        for body in [&published, &offline] {
            assert_eq!(body["availability"]["state"], "stale");
            assert!(body["availability"]["reason"]
                .as_str()
                .unwrap()
                .contains("final log"));
            let status = body["statusText"].as_str().unwrap();
            assert!(status.contains("final log"), "{status}");
            assert!(!status.contains("No metrics found"), "{status}");
        }

        f.signals(&folded(&declared(0..=2, 4)));
        let (_, body) = monitor(&f.store, &run).unwrap();
        assert!(body["latest"]["loss"].is_number());
        assert!(body["statusText"].as_str().unwrap().contains("final log"));

        f.store.finish_signal_collection(&run.id).unwrap();
        let (_, body) = monitor(&f.store, &run).unwrap();
        assert_eq!(body["availability"]["state"], "live");
        assert!(body["latest"]["loss"].is_number());
        assert!(!body["statusText"].as_str().unwrap().contains("final log"));
    }

    #[test]
    fn incomplete_collection_is_exposed_only_for_an_ended_run() {
        for phase in ["running", "lost"] {
            let f = Fixture::new("orx-monitor-incomplete-collection");
            let run = f.run(phase, local());
            f.signals(&folded(&declared(0..=2, 4)));
            super::super::adapters::signals::settle_incomplete(
                f.store.data_dir(),
                &run.id,
                "The rental was deleted",
            )
            .unwrap();

            let (_, page) = monitor(&f.store, &run).unwrap();
            assert_eq!(page["availability"]["state"], "live");
            assert_eq!(
                page["availability"].get("collectionIncomplete"),
                (phase == "lost").then_some(&Value::Bool(true))
            );
            assert_eq!(page["progress"]["done"], 20.0);
        }
    }

    #[test]
    fn each_signal_state_has_its_own_words() {
        let mut folding = Snapshot::new("r1", "1-1", "g1");
        folding.latest.insert("loss".into(), json!(2));
        folding.coverage.add_reason("catching_up");
        let mut settled = folding.clone();
        settled.remove_reason("catching_up");
        settled.coverage.add_reason("collection_incomplete");
        let retrying = json!({"state":"stale","reason":"connection refused","retryAt":0});
        let live = json!({"state":"stale","reason":"budget exceeded"});
        let incomplete = json!({"state":"incomplete","reason":"The rental was released"});
        let cached = json!({"state":"cached","reason":null});
        let table = [
            (true, &retrying, None, &["Final", "retry", "1970"][..]),
            (true, &live, None, &["collection", "delayed", "retry"]),
            (
                false,
                &incomplete,
                Some(&settled),
                &["collection", "could not finish"],
            ),
            (
                false,
                &incomplete,
                Some(&settled),
                &["run ended", "signals", "kept"],
            ),
            (true, &cached, Some(&folding), &["history", "being read"]),
        ];
        for (reading, availability, snapshot, facts) in table {
            let notes = signal_notes("r1", reading, availability, snapshot);
            assert!(
                notes
                    .iter()
                    .any(|note| facts.iter().all(|part| note.contains(*part))),
                "{facts:?} in {notes:?}"
            );
        }
        let notes = signal_notes("r1", false, &cached, Some(&folding));
        assert!(!notes.iter().any(|note| note.contains("still being read")));
    }

    /// A run that has reported nothing has nothing to report missing.
    #[test]
    fn a_run_without_signals_prints_no_signal_error() {
        let empty = json!({"state":"empty","reason":null});
        let silent = Snapshot::new("r1", "1-1", "g1");
        let cached = json!({"state":"cached","reason":null,"lastCollectedAt":5});
        assert_eq!(
            signal_notes("r1", true, &empty, None),
            ["No metrics found in the log yet."]
        );
        assert_eq!(
            signal_notes("r1", true, &cached, Some(&silent)),
            ["No metrics found in the log yet."]
        );
        assert!(signal_notes("r1", false, &empty, None).is_empty());
        assert!(signal_notes("r1", false, &cached, Some(&silent)).is_empty());
    }

    /// `bars` finished tqdm bars of `of` items, then one at `next`, a second an item.
    fn epochs(bars: i64, of: i64, next: i64) -> Snapshot {
        let mut snapshot = Snapshot::new("r1", "1-1", "g1");
        let mut at = T0;
        let frames = (0..bars).flat_map(|_| 0..=of).chain(0..=next);
        for (index, i) in frames.enumerate() {
            let frame = format!(
                "\rtrain: {:3}%| {i}/{of} [00:0{i}<00:00, 1.00it/s]",
                i * 100 / of
            );
            if index > 0 && i == 0 {
                snapshot.feed(b"\n", false, at, true).unwrap();
            }
            snapshot.feed(frame.as_bytes(), false, at, true).unwrap();
            snapshot.reached_end(at);
            at += 1000;
        }
        // A frame is folded once the next one arrives.
        snapshot.feed(b"\r", false, at, true).unwrap();
        snapshot.reached_end(at);
        snapshot
    }

    /// The number of bars to come is unknown, so finished ones are never drawn as full segments.
    #[test]
    fn several_bars_draw_the_current_bar_without_a_whole_run_share() {
        let f = Fixture::new("orx-monitor-epochs");
        let run = f.run("running", local());
        f.signals(&epochs(3, 4, 1));
        f.collected(T0 + 20_000);
        let (monitor, _) = monitor_at(&f, &run, T0 + 20_000);
        let progress = &monitor["progress"];
        assert_eq!(progress["done"], 1.0, "{progress}");
        assert_eq!(
            progress["segments"].as_array().unwrap().len(),
            1,
            "{progress}"
        );
        assert_eq!(progress["segments"][0]["fill"], 0.25, "{progress}");
        assert_eq!(progress["whole"], Value::Null);
        let summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
        assert_eq!(summary["whole"], Value::Null);
        assert_eq!(summary["segments"], json!([{"weight": 1.0, "fill": 0.25}]));
        let first = monitor["statusText"]
            .as_str()
            .unwrap()
            .lines()
            .next()
            .unwrap();
        mentions(first, &["baseline", "running", "step 1 of 4", "[run r1]"]);
        assert!(!first.contains('%'), "{first}");

        let f = Fixture::new("orx-monitor-epochs-done");
        let run = f.run("done", local());
        f.signals(&epochs(5, 4, 4));
        f.collected(T0 + 30_000);
        let (monitor, _) = monitor_at(&f, &run, T0 + 700_000);
        assert_eq!(
            monitor["progress"]["segments"],
            json!([{"weight": 1.0, "fill": 1.0}]),
            "a finished run did all it had to"
        );
        assert_eq!(monitor["progress"]["whole"], 1.0);
        let summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
        assert_eq!(summary["whole"], 1.0);

        let f = Fixture::new("orx-monitor-one-bar");
        let run = f.run("running", local());
        f.signals(&epochs(0, 4, 3));
        f.collected(T0 + 5_000);
        let (monitor, _) = monitor_at(&f, &run, T0 + 5_000);
        let first = monitor["statusText"]
            .as_str()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .to_owned();
        mentions(
            &first,
            &["baseline", "running", "75%", "step 3 of 4", "[run r1]"],
        );

        let f = Fixture::new("orx-monitor-epochs-limit");
        let run = f.run(
            "running",
            json!({"kind": "slurm_job", "timeoutSecs": 36_000}),
        );
        f.signals(&epochs(3, 4, 1));
        f.collected(T0 + 20_000);
        let (monitor, _) = monitor_at(&f, &run, T0 + 20_000);
        let progress = &monitor["progress"];
        assert_eq!(
            progress["segments"].as_array().unwrap().len(),
            1,
            "{progress}"
        );
        let whole = progress["whole"].as_f64().unwrap();
        assert!(whole < 0.01, "{progress}");
        assert_eq!(progress["segments"][0]["fill"], whole, "{progress}");
    }

    /// Listing never reads a checkpoint: an ended run shows the summary it last published.
    #[test]
    fn an_ended_run_is_listed_from_its_stored_summary() {
        let f = Fixture::new("orx-monitor-old");
        let run = f.run("done", local());
        f.signals(&folded(&[(T0, "::orx:: {\"m\":{\"loss\":1}}".into())]));
        let reads = CHECKPOINT_READS.with(std::cell::Cell::get);
        assert!(
            current(&f.store, &run).unwrap().is_none(),
            "a run that ended before monitors shows its lifecycle until opened"
        );
        assert_eq!(CHECKPOINT_READS.with(std::cell::Cell::get), reads);
        assert!(
            f.store.run_monitor_state("r1").unwrap().is_none(),
            "reading writes nothing"
        );

        let (opened, _) = super::monitor(&f.store, &run).unwrap();
        f.restart();
        let [path, _] = signal_paths(f.dir.path(), "r1");
        std::fs::write(&path, "{not json").unwrap();
        let listed = current(&f.store, &run).unwrap().unwrap();
        assert_eq!(listed.summary(), opened.summary());
        assert_eq!(listed.summary()["tone"], "done");
    }

    #[test]
    fn listing_thousands_of_ended_runs_reads_no_checkpoint_and_holds_few() {
        let f = Fixture::new("orx-monitor-history");
        let template = f.run_named("r0", "done", local());
        f.signals(&Snapshot {
            run_id: "r0".into(),
            ..folded(&declared(0..=10, 100))
        });
        super::monitor(&f.store, &template).unwrap();
        let stored = f.store.run_monitor_state("r0").unwrap().unwrap();
        let checkpoint = folded(&declared(0..=10, 100));
        let history = f.store.begin().unwrap();
        let runs: Vec<StoredRun> = (1..=2000)
            .map(|i| {
                let id = format!("r{i}");
                let run = f.run_named(&id, "done", local());
                let [path, _] = signal_paths(f.dir.path(), &id);
                let written = Snapshot {
                    run_id: id.clone(),
                    ..checkpoint.clone()
                };
                std::fs::write(path, serde_json::to_vec(&written).unwrap()).unwrap();
                f.store.put_run_monitor_state(&id, &stored);
                run
            })
            .collect();
        history.commit().unwrap();

        let reads = CHECKPOINT_READS.with(std::cell::Cell::get);
        for run in &runs {
            let listed = current(&f.store, run).unwrap().unwrap();
            assert_eq!(listed.summary()["tone"], "done");
        }
        assert_eq!(CHECKPOINT_READS.with(std::cell::Cell::get), reads);
        assert_eq!(
            f.store.run_monitor_state("r2000").unwrap().unwrap(),
            stored,
            "listing rewrites no ended run"
        );

        for run in &runs[..100] {
            super::monitor(&f.store, run).unwrap();
        }
        let held = cache()
            .unwrap()
            .slots
            .keys()
            .filter(|(data, _)| data == f.dir.path())
            .count();
        assert!(held <= ENDED_HELD, "{held} ended runs held");
    }

    #[test]
    fn a_cold_list_of_flagged_finished_runs_reads_no_checkpoints() {
        use crate::store::NewFlag;

        let f = Fixture::new("orx-monitor-flagged-history");
        let flag = |id: &str| NewFlag {
            run_id: id.into(),
            body: json!({"id": "f1", "source": "look", "text": "Loss rose."}),
            wakes: vec![],
            noted: vec![],
            only_if_clear: false,
        };
        let template = f.run_named("r0", "done", local());
        f.store.raise_flags(&[flag("r0")], T0).unwrap();
        let checkpoint = folded(&declared(0..=1000, 1000));
        f.signals(&Snapshot {
            run_id: "r0".into(),
            ..checkpoint.clone()
        });
        let projection = advance(&f.store, &template, T0).unwrap();
        assert_eq!(projection.summary()["tone"], "flag");
        let stored = f.store.run_monitor_state("r0").unwrap().unwrap();
        let history = f.store.begin().unwrap();
        let runs: Vec<_> = (1..=250)
            .map(|i| {
                let id = format!("flagged{i}");
                let run = f.run_named(&id, "done", local());
                f.store.append_flags(&[flag(&id)], T0).unwrap();
                f.signals(&Snapshot {
                    run_id: id.clone(),
                    ..checkpoint.clone()
                });
                f.store.put_run_monitor_state(&id, &stored);
                run
            })
            .collect();
        history.commit().unwrap();

        let reads = CHECKPOINT_READS.with(std::cell::Cell::get);
        let started = std::time::Instant::now();
        for run in &runs {
            let listed = current(&f.store, run).unwrap().unwrap();
            assert_eq!(listed.summary()["tone"], "flag");
        }
        let opened = CHECKPOINT_READS.with(std::cell::Cell::get) - reads;
        eprintln!(
            "cold list of {} finished runs: {:?}, {opened} checkpoint reads",
            runs.len(),
            started.elapsed()
        );
        assert_eq!(opened, 0);
    }

    #[test]
    fn an_open_watch_flag_uses_its_look_in_the_summary_monitor_and_status() {
        let f = Fixture::new("orx-monitor-frozen-flag");
        let run = f.run("running", local());
        f.store.put_chat_session("chat", "claude-code");
        let tx = f.store.begin_immediate().unwrap();
        let look = Store::prepare_watch_look(
            &tx,
            "chat",
            "live",
            "test",
            &[(&run.id, "r1", "subject")],
            T0,
        )
        .unwrap();
        tx.commit().unwrap();
        let template = "Train rising to **{loss}**.";
        let text = "Train rising to **6.040**.";
        let body = json!({"id": "f1", "source": "look", "lookId": look.to_string(), "template": template, "text": text});
        let flag = super::super::flags::new_flag(&f.store, &run, body, true).unwrap();
        let result = crate::store::LookResult {
            run_id: run.id.clone(),
            result: "flag".into(),
            template: Some(template.into()),
            text: Some(text.into()),
            basis: json!({"bindings": {"loss": 6.04}}),
            whole: None,
            cursor: Value::Null,
        };
        f.store
            .finish_watch_look(
                look,
                "done",
                T0 + 1_000,
                None,
                None,
                None,
                &[result],
                &[flag],
            )
            .unwrap();
        let snapshot = folded(&[(
            T0 + 2_000,
            r#"::orx:: {"step":2,"m":{"loss":5.438}}"#.into(),
        )]);
        f.collect(&snapshot);
        let (monitor, _) = monitor_at(&f, &run, T0 + 2_000);
        let summary = current(&f.store, &run).unwrap().unwrap();
        assert_eq!(monitor["latest"]["loss"], 5.438);
        assert_eq!(monitor["flags"][0]["bindings"]["loss"], 6.04);
        assert_eq!(summary.summary()["flag"]["bindings"]["loss"], 6.04);
        assert_eq!(summary.summary()["flag"]["text"], text);
        assert!(status_line(&monitor, "flag").contains("Train rising to 6.040."));
        assert!(monitor["sentence"].is_null());
    }

    #[test]
    fn a_finished_runs_flag_is_followed_up_when_its_agent_turn_ends() {
        let f = Fixture::new("orx-monitor-finished-flag");
        let run = f.run("running", local());
        let chat = "chat";
        f.store.put_chat_session(chat, "claude-code");
        f.store.subscribe_run(&run.id, chat, T0).unwrap();
        let body = json!({"id": "f1", "source": "look", "text": "Loss rose."});
        let flag = super::super::flags::new_flag(&f.store, &run, body, true).unwrap();
        f.store.raise_flags(&[flag], T0).unwrap();
        let notices = f.store.run_notice_batch_at(chat, T0 + 1).unwrap();
        let turn = &notices[0].client_turn_id;
        f.store.put_chat_turn(chat, turn, "running", T0 + 1);
        f.store
            .mark_run_notice_batch_delivered(chat, turn, T0 + 1)
            .unwrap();

        let run = f.run("done", local());
        f.signals(&folded(&declared(0..=10, 100)));
        let published = advance(&f.store, &run, T0 + 600_000).unwrap();
        assert_eq!(published.summary()["flag"]["state"], "agent_looking");
        f.restart();
        let reads = CHECKPOINT_READS.with(std::cell::Cell::get);
        let listed = current(&f.store, &run).unwrap().unwrap();
        assert_eq!(listed.summary()["flag"]["state"], "agent_looking");
        assert_eq!(CHECKPOINT_READS.with(std::cell::Cell::get), reads);

        f.store
            .conn_execute_for_tests("UPDATE chat_turns SET state = 'completed'");
        let listed = current(&f.store, &run).unwrap().unwrap();
        assert_eq!(listed.summary()["flag"]["state"], "waking");
    }

    /// The laptop hands each checkpoint it publishes to the projection, so many live runs
    /// reporting every second cost no reads.
    #[test]
    fn the_hot_path_never_reads_a_checkpoint_back() {
        /// Fifty live runs projected each second for twelve seconds: the checkpoints read, and
        /// how long it took. With a parent, each run is compared with its parent's ended run.
        fn hot(parented: bool) -> (usize, std::time::Duration) {
            let f = Fixture::new("orx-monitor-hot");
            if parented {
                f.child("base", "exp");
                f.store.set_experiment_parent("base", None).unwrap();
                f.store.set_experiment_parent("exp", Some("base")).unwrap();
                f.view("exp", VIEW);
                let parent = loss_run(&f, "p1", "base", "done", 20, 1.0);
                monitor_at(&f, &parent, T0 + 700_000);
            }
            let runs: Vec<(StoredRun, Snapshot)> = (0..50)
                .map(|i| {
                    let id = format!("live{i}");
                    let run = f.run_named(&id, "running", local());
                    let snapshot = Snapshot {
                        run_id: id,
                        ..folded(&declared(0..=1, 400))
                    };
                    (run, snapshot)
                })
                .collect();
            let reads = CHECKPOINT_READS.with(std::cell::Cell::get);
            let started = std::time::Instant::now();
            let mut runs = runs;
            for second in 2..=13 {
                let now = T0 + second * 1000;
                for (run, snapshot) in &mut runs {
                    fold(snapshot, &declared(second..=second, 400));
                    f.collect(snapshot);
                    let [_, status] = signal_paths(f.dir.path(), &run.id);
                    // Rewritten every second; only `lastCollectedAt` moves, once a second.
                    std::fs::write(
                        status,
                        json!({"state": "cached", "reason": null, "lastCollectedAt": now, "lastGoodAt": now}).to_string(),
                    )
                    .unwrap();
                    let (projection, _) = publish(&f.store, run, now, None).unwrap();
                    assert_eq!(
                        projection.body["progress"]["done"],
                        json!(second as f64 * 10.0)
                    );
                    assert_eq!(
                        projection.body["parent"]["runId"].as_str(),
                        parented.then_some("p1")
                    );
                }
            }
            let elapsed = started.elapsed();
            (CHECKPOINT_READS.with(std::cell::Cell::get) - reads, elapsed)
        }
        let (alone, before) = hot(false);
        let (parented, after) = hot(true);
        eprintln!("hot path: {before:?} alone, {after:?} compared with a parent");
        assert_eq!(alone, 0);
        assert_eq!(parented, 1, "an ended parent's checkpoint is read once");
    }

    #[test]
    fn a_status_rewrite_that_changes_nothing_read_projects_nothing() {
        let f = Fixture::new("orx-monitor-status");
        let run = f.run("running", local());
        f.collect(&folded(&declared(0..=10, 400)));
        f.collected(T0 + 10_000);
        let (first, _) = monitor_at(&f, &run, T0 + 10_000);
        f.status(json!({"state": "cached", "reason": null, "lastCollectedAt": T0 + 10_000, "lastGoodAt": 1}));
        let (projection, event) = publish(&f.store, &run, T0 + 12_000, None).unwrap();
        assert_eq!(
            (projection.body["rev"].clone(), event),
            (first["rev"].clone(), None)
        );
        assert_eq!(
            slot(&f.store, "r1")
                .unwrap()
                .held()
                .unwrap()
                .unwrap()
                .evaluated_at,
            T0 + 10_000,
            "not evaluated again"
        );
    }

    /// Whoever is projecting a run, a reader gets what was last published without waiting.
    #[test]
    fn a_reader_never_waits_for_a_projection() {
        let f = Fixture::new("orx-monitor-turn");
        let run = f.run("running", local());
        f.collect(&folded(&declared(0..=10, 400)));
        f.collected(T0 + 10_000);
        let (first, _) = monitor_at(&f, &run, T0 + 10_000);
        f.collect(&folded(&declared(0..=12, 400)));
        let slot = slot(&f.store, "r1").unwrap();
        let _projecting = slot.turn.lock().unwrap();
        let (sent, answer) = std::sync::mpsc::channel();
        let data = f.dir.path().to_path_buf();
        std::thread::spawn(move || {
            let store = Store::open_at(data).unwrap();
            let listed = current(&store, &run).unwrap().unwrap().summary()["rev"].clone();
            let (projection, event) = publish(&store, &run, T0 + 20_000, None).unwrap();
            sent.send((listed, projection.body["rev"].clone(), event))
                .unwrap();
        });
        let answer = answer.recv_timeout(Duration::from_secs(10));
        assert_eq!(
            answer.expect("a reader waited on the projection under way"),
            (first["rev"].clone(), first["rev"].clone(), None)
        );
    }

    #[test]
    fn eviction_keeps_an_ended_runs_publisher_until_its_event_leaves() {
        let f = Fixture::new("orx-monitor-eviction");
        let run = f.run("done", local());

        let (rev, changed) = std::thread::scope(|scope| {
            let (entered, publishing) = std::sync::mpsc::sync_channel(0);
            let (release, resume) = std::sync::mpsc::channel();
            let resume = Mutex::new(resume);
            let data = f.dir.path().to_path_buf();
            let initial = run.clone();
            scope.spawn(move || {
                let store = Store::open_at(data).unwrap();
                let sink: Sink = Box::new(move |_, event| {
                    entered.send(event["rev"].clone()).unwrap();
                    let _ = resume.lock().unwrap().recv();
                });
                publish(&store, &initial, T0 + 700_000, Some(&sink)).unwrap();
            });
            let first = publishing.recv_timeout(Duration::from_secs(10)).unwrap();
            for n in 0..ENDED_HELD {
                let other = f.run_named(&format!("other-{n}"), "done", local());
                publish(&f.store, &other, T0 + 700_000, None).unwrap();
            }
            f.store.set_run_status(&run.id, "failed");
            let changed = f.store.get_run(&run.id).unwrap().unwrap();
            let (projection, event) = publish(&f.store, &changed, T0 + 701_000, None).unwrap();
            assert_eq!(projection.body["rev"], first);
            assert!(event.is_none());
            release.send(()).unwrap();
            (first, changed)
        });

        let (_, next) = publish(&f.store, &changed, T0 + 701_000, None).unwrap();
        assert_eq!(next.unwrap()["rev"], rev.as_u64().unwrap() + 1);
    }

    #[test]
    fn authored_outcomes_follow_retained_rows_and_live_events() {
        let f = Fixture::new("orx-monitor-outcomes");
        let run = f.run("running", local());
        f.view("exp", r#"<view><cells from="items" /><bars from="items" by="category" /><bars from="items" by="split" /></view>"#);
        let row = |category: &str, outcome: &str| {
            (
                T0,
                format!(
                    r#"::orx:: {{"row":"items","m":{{"category":"{category}","split":"test","outcome":"{outcome}"}}}}"#
                ),
            )
        };
        let mut rows = vec![
            row("proofs", "correct"),
            row("proofs", "wrong"),
            row("geometry", "unknown"),
        ];
        rows.resize(2048, row("proofs", "error"));
        let mut snapshot = folded(&rows);
        f.collect(&snapshot);
        f.collected(T0 + 1_000);
        let (first, _) = monitor_at(&f, &run, T0 + 1_000);
        assert!(first.get("cells").is_none());
        assert_eq!(first["rows"]["items"].as_array().unwrap().len(), 2048);
        assert_eq!(
            first["groups"]["items"]["category"],
            json!([
                {"key":"geometry", "ok":0, "n":0}, {"key":"proofs", "ok":1, "n":2047}
            ])
        );
        assert_eq!(
            first["groups"]["items"]["split"],
            json!([{"key":"test", "ok":1, "n":2047}])
        );

        fold(&mut snapshot, &[row("proofs", "invalid")]);
        f.collect(&snapshot);
        f.collected(T0 + 2_000);
        let (next, event) = monitor_at(&f, &run, T0 + 2_000);
        let event = event.unwrap();
        assert_eq!(event.get("reset"), None);
        assert_eq!(event["dropRows"]["items"], 1);
        assert_eq!(
            event["appendRows"]["items"],
            json!(snapshot.rows["items"][2047..])
        );
        assert_eq!(event["groups"], next["groups"]);
        assert_eq!(
            next["groups"]["items"]["category"][1],
            json!({"key":"proofs", "ok":0, "n":2046})
        );

        let shown = super::super::view::shown(&run, &next, &Value::Null, &[], None, None);
        assert!(shown.contains("256 retained rows"), "{shown}");
        assert!(
            shown.contains("0 correct, 0 wrong, 255 errors, 1 unknown"),
            "{shown}"
        );
        assert!(shown.contains("geometry: — (0 of 0)"), "{shown}");
        assert!(shown.contains("proofs: 0% (0 of 2046)"), "{shown}");

        let added: Vec<_> = (2049..2081)
            .map(|i| row(&format!("case-{i}"), "correct"))
            .collect();
        fold(&mut snapshot, &added);
        f.collect(&snapshot);
        f.collected(T0 + 3_000);
        let (latest, _) = monitor_at(&f, &run, T0 + 3_000);
        let categories = latest["groups"]["items"]["category"].as_array().unwrap();
        assert_eq!(categories.len(), MAX_OUTCOME_GROUPS);
        assert_eq!(categories.first().unwrap()["key"], "case-2049");
        assert_eq!(categories.last().unwrap()["key"], "case-2080");
    }

    /// An event carries the rows a table gained, never the table.
    #[test]
    fn rows_travel_like_series() {
        let f = Fixture::new("orx-monitor-rows");
        let run = f.run("running", local());
        let row = |i: i64| {
            (
                T0 + i * 1000,
                format!("::orx:: {{\"row\":\"eval\",\"m\":{{\"task\":\"t{i}\",\"score\":{i}}}}}"),
            )
        };
        let mut snapshot = folded(&(0..3).map(row).collect::<Vec<_>>());
        f.collect(&snapshot);
        f.collected(T0 + 3000);
        let (first, _) = monitor_at(&f, &run, T0 + 3000);
        assert_eq!(
            first["rows"]["eval"].as_array().unwrap().len(),
            3,
            "a read has the table"
        );

        fold(&mut snapshot, &[row(3), row(4)]);
        f.collect(&snapshot);
        let (_, event) = monitor_at(&f, &run, T0 + 5000);
        let event = event.unwrap();
        assert_eq!(event.get("reset"), None, "{event}");
        assert_eq!(
            event["appendRows"]["eval"],
            json!(snapshot.rows["eval"][3..])
        );
        assert_eq!(event.get("rows"), None);
        assert!(event["seriesRev"].as_u64() > first["seriesRev"].as_u64());

        // A row that plots nothing still moves `seriesRev`, which the page's next append follows.
        let line = b"::orx:: {\"row\":\"eval\",\"m\":{\"task\":\"t5\"}}\n";
        snapshot.feed(line, false, T0 + 6000, false).unwrap();
        snapshot.reached_end(T0 + 6000);
        f.collect(&snapshot);
        let alone = monitor_at(&f, &run, T0 + 7000).1.unwrap();
        assert_eq!(alone["append"], json!({}), "{alone}");
        assert_eq!(
            alone["appendRows"]["eval"],
            json!(snapshot.rows["eval"][5..])
        );
        assert!(alone["seriesRev"].as_u64() > event["seriesRev"].as_u64());
    }

    /// Past the fold's row cap the oldest rows leave as new ones come: each event says how many
    /// left, so an open page slides its table and never reads the whole monitor again.
    #[test]
    fn rows_slide_past_the_cap_without_a_reset() {
        let f = Fixture::new("orx-monitor-rows-slide");
        let run = f.run("running", local());
        let row = |i: i64| {
            (
                T0 + i,
                format!("::orx:: {{\"row\":\"eval\",\"m\":{{\"task\":\"t{i}\"}}}}"),
            )
        };
        let mut snapshot = folded(&(0..2000).map(row).collect::<Vec<_>>());
        f.collect(&snapshot);
        f.collected(T0 + 2000);
        let (first, _) = monitor_at(&f, &run, T0 + 2000);
        let mut page = first["rows"]["eval"].as_array().unwrap().clone();
        let mut series_rev = first["seriesRev"].as_u64().unwrap();
        let mut resets = 0;
        for batch in 0..10 {
            let from = 2000 + batch * 100;
            let compacted = snapshot.series_rev;
            fold(
                &mut snapshot,
                &(from..from + 100).map(row).collect::<Vec<_>>(),
            );
            f.collect(&snapshot);
            let now = T0 + 3000 + batch * 1000;
            f.collected(now);
            let (monitor, event) = monitor_at(&f, &run, now);
            let event = event.unwrap();
            let moved = event["seriesRev"].as_u64().unwrap();
            assert!(moved > series_rev, "batch {batch}: new rows move seriesRev");
            series_rev = moved;
            // The cells are signals too, and their series halve now and then: a new basis.
            if snapshot.series_rev != compacted {
                assert_eq!(event["reset"], true, "batch {batch}");
                resets += 1;
                page = monitor["rows"]["eval"].as_array().unwrap().clone();
                continue;
            }
            assert_eq!(event.get("reset"), None, "batch {batch}: {event}");
            let dropped = event["dropRows"]["eval"].as_u64().unwrap_or(0) as usize;
            page.drain(..dropped);
            page.extend(
                event["appendRows"]["eval"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
        }
        assert!(resets <= 1, "{resets} resets");
        assert_eq!(page.len(), 2048);
        assert_eq!(json!(page), json!(snapshot.rows["eval"]));

        // A table that shrank with no row dropped from its front is not the one the page holds.
        snapshot.rows.get_mut("eval").unwrap().pop();
        f.collect(&snapshot);
        f.collected(T0 + 20_000);
        let (_, event) = monitor_at(&f, &run, T0 + 20_000);
        assert_eq!(event.unwrap()["reset"], true);
    }

    /// A full bar on a run still going says nothing of what remains: validation, saving or a last
    /// evaluation may follow it.
    #[test]
    fn tables_sharing_the_cap_slide_by_their_own_amounts() {
        let f = Fixture::new("orx-monitor-rows-two-tables");
        let run = f.run("running", local());
        // Runs of identical rows, two tables interleaved unevenly, so a shift by the total
        // dropped, or by the first match from either end, shows the wrong rows.
        let row = |i: i64| {
            let table = if i % 7 < 2 { "train" } else { "eval" };
            (
                T0 + i,
                format!(
                    "::orx:: {{\"row\":\"{table}\",\"m\":{{\"task\":\"t{}\"}}}}",
                    i / 5
                ),
            )
        };
        let mut snapshot = folded(&(0..2048).map(row).collect::<Vec<_>>());
        f.collect(&snapshot);
        f.collected(T0 + 2048);
        let (first, _) = monitor_at(&f, &run, T0 + 2048);
        let mut page = first["rows"]["eval"].as_array().unwrap().clone();
        let mut from = 2048;
        for (batch, size) in [1_i64, 3, 79, 40, 2, 17, 64, 5].into_iter().enumerate() {
            let compacted = snapshot.series_rev;
            let lines: Vec<_> = (from..from + size).map(row).collect();
            from += size;
            fold(&mut snapshot, &lines);
            f.collect(&snapshot);
            let now = T0 + 5000 + batch as i64 * 1000;
            f.collected(now);
            let (monitor, event) = monitor_at(&f, &run, now);
            let event = event.unwrap();
            let names: Vec<&String> = event["dropRows"].as_object().unwrap().keys().collect();
            assert!(
                names
                    .iter()
                    .all(|name| monitor["rows"].get(name.as_str()).is_some()),
                "batch {batch}: rows dropped only from the table the page holds: {names:?}"
            );
            if snapshot.series_rev != compacted {
                page = monitor["rows"]["eval"].as_array().unwrap().clone();
                continue;
            }
            assert_eq!(event.get("reset"), None, "batch {batch}");
            let dropped = event["dropRows"]["eval"].as_u64().unwrap_or(0) as usize;
            page.drain(..dropped);
            page.extend(
                event["appendRows"]["eval"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
            assert_eq!(json!(page), json!(snapshot.rows["eval"]), "batch {batch}");
        }
        assert!(!snapshot.rows["train"].is_empty() && snapshot.omitted["rows"] > 200);
    }

    #[test]
    fn a_full_bar_on_a_live_run_leaves_the_rest_unknown() {
        let f = Fixture::new("orx-monitor-full-bar");
        let run = f.run("running", local());
        f.signals(&epochs(0, 4, 4));
        for after in [14_000, 120_000, 300_000] {
            let now = T0 + 5_000 + after;
            f.collected(now);
            let (monitor, _) = monitor_at(&f, &run, now);
            let progress = &monitor["progress"];
            assert_eq!(progress["whole"], Value::Null, "{after}: {progress}");
            assert_eq!(progress["segments"][0]["fill"], 1.0, "{after}: {progress}");
            assert_eq!(monitor["estimate"], Value::Null, "{after}");
            let text = monitor["statusText"].as_str().unwrap();
            let header = text.lines().next().unwrap();
            mentions(header, &["baseline", "running", "step 4 of 4", "[run r1]"]);
            assert!(!header.contains('%'), "{header}");
            mentions(status_line(&monitor, "eta"), &["No estimate", "unknown"]);
            let summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
            assert_eq!(summary["whole"], Value::Null, "{after}");
            assert_eq!(summary["estimate"], Value::Null, "{after}");
        }

        // A run that failed on its last bar got that far, and no further bar is to come.
        let f = Fixture::new("orx-monitor-full-bar-failed");
        let run = f.run("failed", local());
        let mut snapshot = epochs(0, 4, 4);
        snapshot.extra.insert("terminal".into(), true.into());
        f.signals(&snapshot);
        f.collected(T0 + 10_000);
        let (monitor, _) = monitor_at(&f, &run, T0 + 700_000);
        assert_eq!(monitor["progress"]["whole"], 1.0, "{}", monitor["progress"]);
        let summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
        assert_eq!(summary["whole"], 1.0);
    }

    /// `orx up` stopped between a run's end and its projection: the next read projects it.
    #[test]
    fn a_missed_settle_is_repaired_where_it_is_read() {
        let f = Fixture::new("orx-monitor-missed-settle");
        let run = f.run("running", local());
        f.collect(&folded(&declared(0..=20, 400)));
        f.collected(T0 + 20_000);
        monitor_at(&f, &run, T0 + 20_000);
        f.restart();
        let run = f.run("done", local());
        let listed = current(&f.store, &run).unwrap().unwrap();
        assert_eq!(listed.summary()["tone"], "done");
        assert_eq!(listed.summary()["estimate"], Value::Null);
    }

    /// A projection that panics poisons nothing: until one succeeds, readers are told the
    /// monitor is unavailable and why, and the next attempt starts clean.
    fn sink() -> (Sink, Arc<Mutex<Vec<Arc<Value>>>>) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let sink: Sink = {
            let sent = sent.clone();
            Box::new(move |_, event| sent.lock().unwrap().push(event))
        };
        (sink, sent)
    }

    fn assert_failing(summary: &Value) {
        assert_eq!(summary["unavailable"], MONITOR_FAILED, "{summary}");
        assert_eq!(summary["estimate"], Value::Null, "{summary}");
    }

    #[test]
    fn a_failed_projection_is_said_and_the_next_one_starts_clean() {
        let f = Fixture::new("orx-monitor-failed");
        let run = f.run("running", local());
        let mut snapshot = folded(&declared(0..=10, 400));
        f.collect(&snapshot);
        f.collected(T0 + 10_000);
        monitor_at(&f, &run, T0 + 10_000);
        fold(&mut snapshot, &declared(11..=12, 400));
        f.collect(&snapshot);
        f.collected(T0 + 12_000);
        let (sink, sent) = sink();
        BREAK_NEXT.with(|next| next.set(true));
        let error = publish(&f.store, &run, T0 + 12_000, Some(&sink))
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            MONITOR_FAILED,
            "a panic's text is only logged"
        );
        let shown = slot(&f.store, "r1")
            .unwrap()
            .held()
            .unwrap()
            .unwrap()
            .projection
            .clone();
        assert_failing(shown.summary());
        let availability = &shown.body["availability"];
        assert_eq!(availability["state"], "unavailable", "{availability}");
        assert_eq!(availability["reason"], MONITOR_FAILED, "{availability}");
        let sent = std::mem::take(&mut *sent.lock().unwrap());
        assert_eq!(sent.len(), 1, "an open page is told to read it again");
        assert_eq!(sent[0]["reset"], true);

        let (projection, _) = publish(&f.store, &run, T0 + 12_500, None).unwrap();
        assert_eq!(projection.body["availability"]["state"], "live");
        assert_eq!(projection.body["progress"]["done"], 120.0);

        // An ended run projects the same between attempts; its recovery is published all the same.
        let run = f.run("done", local());
        publish(&f.store, &run, T0 + 700_000, None).unwrap();
        BREAK_NEXT.with(|next| next.set(true));
        assert!(publish(&f.store, &run, T0 + 701_000, None).is_err());
        let (back, event) = publish(&f.store, &run, T0 + 702_000, None).unwrap();
        assert!(event.is_some(), "readers told of the failure hear it ended");
        assert!(back.summary().get("unavailable").is_none());
    }

    #[test]
    fn a_failing_store_is_said_in_every_summary_until_it_recovers() {
        let f = Fixture::new("orx-monitor-failing-store");
        let run = f.run("running", local());
        let mut snapshot = folded(&declared(0..=10, 400));
        f.collect(&snapshot);
        f.collected(T0 + 10_000);
        let (good, _) = publish(&f.store, &run, T0 + 10_000, None).unwrap();
        assert!(good.summary()["estimate"].is_object(), "{}", good.summary());
        let rev = good.published.rev;

        f.failing("r1", true);
        fold(&mut snapshot, &declared(11..=12, 400));
        f.collect(&snapshot);
        f.collected(T0 + 12_000);
        let (sink, sent) = sink();
        for at in [T0 + 12_000, T0 + 13_000, T0 + 72_000, T0 + 612_000] {
            assert!(publish(&f.store, &run, at, Some(&sink)).is_err());
            let listed = current(&f.store, &run).unwrap().unwrap();
            assert_failing(listed.summary());
            assert_eq!(listed.summary()["tone"], "running");
        }
        let failed = std::mem::take(&mut *sent.lock().unwrap());
        assert_eq!(failed.len(), 1, "said when it starts, not on each attempt");
        assert_eq!(failed[0]["reset"], true);
        assert_eq!(failed[0]["rev"], rev);

        // Nothing on disk changes: the next attempt is a projection all the same.
        f.failing("r1", false);
        let (back, event) = publish(&f.store, &run, T0 + 613_000, Some(&sink)).unwrap();
        assert!(
            back.summary().get("unavailable").is_none(),
            "{}",
            back.summary()
        );
        assert_eq!(back.body["progress"]["done"], 120.0);
        assert!(event.is_some(), "the recovery is published");
        assert!(back.published.rev > rev);
        assert_eq!(sent.lock().unwrap().len(), 1);
        let listed = current(&f.store, &run).unwrap().unwrap();
        assert!(listed.summary().get("unavailable").is_none());
    }

    #[test]
    fn a_done_run_whose_settle_failed_never_lists_as_running() {
        let f = Fixture::new("orx-monitor-failed-settle");
        let run = f.run("running", local());
        f.collect(&folded(&declared(0..=20, 400)));
        f.collected(T0 + 20_000);
        publish(&f.store, &run, T0 + 20_000, None).unwrap();
        f.failing("r1", true);
        let run = f.run("done", local());
        assert!(publish(&f.store, &run, T0 + 700_000, None).is_err());
        let listed = current(&f.store, &run).unwrap().unwrap();
        assert_eq!(listed.summary()["tone"], "done");
        assert_failing(listed.summary());
        // The read tried again at the clock's time; this attempt leaves the entry a second old.
        assert!(publish(&f.store, &run, T0 + 701_000, None).is_err());

        f.failing("r1", false);
        let listed = current(&f.store, &run).unwrap().unwrap();
        assert_eq!(listed.summary()["tone"], "done");
        assert!(listed.summary().get("unavailable").is_none());
        f.restart();
        let stored = current(&f.store, &run).unwrap().unwrap();
        assert_eq!(stored.summary()["tone"], "done", "the settle was saved");
    }

    /// A read retries a failing projection at most once a second, and the log speaks once per
    /// change: when the cause first appears and when it clears.
    #[test]
    fn a_failing_projection_is_retried_on_read_once_a_second_and_logged_once_per_change() {
        let f = Fixture::new("orx-monitor-failing-pace");
        let run = f.run("running", local());
        let mut snapshot = folded(&declared(0..=10, 400));
        f.collect(&snapshot);
        f.collected(T0 + 10_000);
        publish(&f.store, &run, T0 + 10_000, None).unwrap();
        let logged = LOGGED.with(std::cell::Cell::get);

        f.failing("r1", true);
        let now = crate::store::now_ms();
        for at in [now + 60_000, now + 61_000, now + 62_000] {
            fold(&mut snapshot, &declared(11..=11, 400));
            f.collect(&snapshot);
            assert!(publish(&f.store, &run, at, None).is_err());
        }
        assert_eq!(
            LOGGED.with(std::cell::Cell::get),
            logged + 1,
            "one cause, one line"
        );

        f.failing("r1", false);
        let read = current(&f.store, &run).unwrap().unwrap();
        assert_failing(read.summary());
        let held = slot(&f.store, "r1").unwrap().held().unwrap().unwrap();
        assert_eq!(
            held.evaluated_at,
            now + 62_000,
            "not retried within its second"
        );

        f.failing("r1", true);
        assert!(publish(&f.store, &run, crate::store::now_ms() - interval(1), None).is_err());
        f.failing("r1", false);
        let read = current(&f.store, &run).unwrap().unwrap();
        assert!(
            read.summary().get("unavailable").is_none(),
            "{}",
            read.summary()
        );
        assert_eq!(
            LOGGED.with(std::cell::Cell::get),
            logged + 2,
            "and one for its recovery"
        );
        current(&f.store, &run).unwrap().unwrap();
        assert_eq!(LOGGED.with(std::cell::Cell::get), logged + 2);
    }

    #[test]
    fn an_ended_run_reads_its_checkpoint_once_however_often_its_status_is_rewritten() {
        let f = Fixture::new("orx-monitor-ended-status");
        let run = f.run("done", local());
        f.signals(&folded(&declared(0..=10, 100)));
        f.collected(T0 + 10_000);
        publish(&f.store, &run, T0 + 700_000, None).unwrap();
        let reads = CHECKPOINT_READS.with(std::cell::Cell::get);
        for i in 1..=5 {
            f.status(json!({"state": "cached", "reason": null, "lastCollectedAt": T0 + 10_000, "lastGoodAt": i}));
            publish(&f.store, &run, T0 + 700_000 + i * 1000, None).unwrap();
        }
        assert_eq!(CHECKPOINT_READS.with(std::cell::Cell::get), reads);
    }

    #[test]
    fn an_ended_run_reads_its_series_again_and_holds_none() {
        let f = Fixture::new("orx-monitor-ended");
        let run = f.run("done", local());
        f.signals(&folded(&declared(0..=10, 100)));
        f.collected(T0 + 10_000);
        let (projection, whole) = super::monitor(&f.store, &run).unwrap();
        assert!(projection.snapshot.is_none());
        assert_eq!(whole["series"]["loss"].as_array().unwrap().len(), 11);
        mentions(status_line(&whole, "eta"), &["Done", "10m"]);
        f.restart();
        let (again, _) = super::monitor(&f.store, &run).unwrap();
        assert_eq!(
            again.published.rev, projection.published.rev,
            "an ended run's monitor is settled"
        );
    }

    /// The series of an ended run are read from its checkpoint when opened; a checkpoint that
    /// can't be read, or that keeps changing under the read, is said in words.
    #[test]
    fn an_ended_run_whose_series_cannot_be_read_says_why_in_words() {
        let f = Fixture::new("orx-monitor-series-unreadable");
        let run = f.run("done", local());
        let snapshot = folded(&declared(0..=10, 100));
        f.signals(&snapshot);
        f.collected(T0 + 10_000);
        super::monitor(&f.store, &run).unwrap();
        let [path, _] = signal_paths(f.dir.path(), "r1");
        // Rewritten in place with its length and time kept, so the projection takes it as unchanged.
        let rewrite = |bytes: &[u8]| {
            let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
            assert_eq!(bytes.len() as u64, std::fs::metadata(&path).unwrap().len());
            std::fs::write(&path, bytes).unwrap();
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(modified)
                .unwrap();
        };
        let good = std::fs::read(&path).unwrap();

        rewrite(&vec![b' '; good.len()]);
        let Err(error) = super::monitor(&f.store, &run) else {
            panic!("the read should fail");
        };
        assert_eq!(error.to_string(), SIGNALS_UNREADABLE);
        assert!(format!("{error:#}").contains("r1.json"), "{error:#}");

        let mut other = snapshot.clone();
        other.generation = "g2".into();
        let elsewhere = f.dir.path().join("other.json");
        super::super::adapters::local::signals::publish(&elsewhere, &other).unwrap();
        rewrite(&std::fs::read(&elsewhere).unwrap());
        let Err(error) = super::monitor(&f.store, &run) else {
            panic!("the read should fail");
        };
        assert_eq!(error.to_string(), SERIES_MOVING);
    }

    #[test]
    fn the_published_pointer_keeps_keys_a_newer_orx_stored() {
        let f = Fixture::new("orx-monitor-state");
        let run = f.run("running", local());
        f.store.put_run_monitor_state(
            "r1",
            &json!({"published": {"summary": {"rev": 1}}, "schedule": {"nextAt": 5}}),
        );
        publish(&f.store, &run, T0, None).unwrap();
        let stored = f.store.run_monitor_state("r1").unwrap().unwrap();
        assert_eq!(
            stored["published"]["summary"]["rev"], 2,
            "an unreadable pointer still only grows"
        );
        assert_eq!(stored["schedule"], json!({"nextAt": 5}));
    }

    /// One server state with the display values shared by the CLI and dashboard tests.
    fn state(
        name: &str,
        status: &str,
        backend: Value,
        snapshot: &Snapshot,
        collected: i64,
        now: i64,
        mut expect: Value,
    ) -> (String, Value) {
        let f = Fixture::new(&format!("orx-monitor-contract-{name}"));
        let run = f.run(status, backend.clone());
        f.collect(snapshot);
        f.collected(collected);
        let (monitor, _) = monitor_at(&f, &run, now);
        let summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
        let eta = status_line(&monitor, "eta");
        assert!(!eta.is_empty(), "{name}: missing estimate line");
        let cli = expect.as_object_mut().unwrap().remove("cli").unwrap();
        for part in cli.as_array().unwrap() {
            mentions(eta, &[part.as_str().unwrap()]);
        }
        if let Some(duration) = expect["duration"].as_str() {
            mentions(eta, &[duration]);
        }
        if let Some(range) = expect["range"].as_array() {
            let evidence = format!(
                "{} to {}",
                range[0].as_str().unwrap(),
                range[1].as_str().unwrap()
            );
            mentions(eta, &[&evidence]);
        }
        let text = monitor["statusText"].as_str().unwrap();
        for (signal, words) in expect["measures"].as_object().into_iter().flatten() {
            let said = format!("{signal} {}", words.as_str().unwrap());
            assert!(text.contains(&said), "{name}: {said} in {text}");
        }
        for case in expect["declared"].as_array().into_iter().flatten() {
            assert_eq!(
                value_text(&case["value"], &case["spec"]),
                case["words"],
                "{name}: {case}"
            );
        }
        if let Some(stage) = expect["stage"].as_str() {
            let first = text.lines().next().unwrap();
            assert!(first.contains(&format!("  {stage}  ")), "{name}: {first}");
        }
        let run = json!({
            "status": status,
            "createdAt": T0,
            "startedAt": T0,
            "updatedAt": T0,
            "endedAt": run.ended_at,
            "backend": backend,
        });
        (
            name.to_owned(),
            json!({"run": run, "now": now, "monitor": monitor, "summary": summary, "expect": expect}),
        )
    }

    const VIEW: &str = r#"<signal name="loss" label="Loss" digits="3" better="lower" />
<signal name="val_loss" label="Validation loss" better="lower" />
<view headline="val_loss loss">
  <chart y="loss" ghost="parent" />
  <stats of="loss" />
</view>
"#;

    /// A run of `experiment` whose loss is `scale` times the parent's, reported to step `last`.
    fn loss_run(
        f: &Fixture,
        id: &str,
        experiment: &str,
        status: &str,
        last: i64,
        scale: f64,
    ) -> StoredRun {
        let run = f.run_in(id, experiment, status, local());
        let lines: Vec<(i64, String)> = (0..=last)
            .map(|i| {
                let loss = scale * 3.0 / (i + 1) as f64;
                (
                    T0 + i * 1000,
                    format!(
                        "::orx:: {{\"step\":{},\"of\":400,\"t\":{i},\"m\":{{\"loss\":{loss}}}}}",
                        i * 10
                    ),
                )
            })
            .collect();
        f.collect(&Snapshot {
            run_id: id.into(),
            ..folded(&lines)
        });
        run
    }

    #[test]
    fn a_value_that_went_wrong_leads_the_run_whatever_the_views_order() {
        let f = Fixture::new("orx-monitor-wrong-headline");
        f.view(
            "exp",
            "<signal name=\"val_loss\" label=\"Val loss\" better=\"lower\" />\n<signal name=\"eval_score\" label=\"Eval score\" better=\"higher\" />\n<view headline=\"val_loss eval_score\"><stats of=\"val_loss eval_score\" /></view>",
        );
        let ended = |id: &str, experiment: &str, val_loss: &str, score: &str| {
            let run = f.run_in(id, experiment, "done", local());
            f.collect(&Snapshot {
                run_id: id.into(),
                ..folded(&[
                    (T0, format!("::orx:: {{\"phase\":\"train\",\"step\":1,\"of\":1,\"m\":{{\"val_loss\":{val_loss}}}}}")),
                    (T0 + 1000, format!("::orx:: {{\"phase\":\"eval\",\"step\":1,\"of\":1,\"m\":{{\"eval_score\":{score}}}}}")),
                ])
            });
            run
        };
        let parent = ended("p1", "exp", "1.0", "0.68");
        monitor_at(&f, &parent, T0 + 700_000);
        f.child("child", "exp");
        for (id, val_loss, score, leads) in [
            ("healthy", "1.0", "0.6", "val_loss"),
            ("nan", "NaN", "0.25", "val_loss"),
            ("zeros", "1.0", "0", "eval_score"),
        ] {
            let run = ended(id, "child", val_loss, score);
            let (monitor, _) = monitor_at(&f, &run, T0 + 700_000);
            assert_eq!(monitor["headline"], leads, "{id}");
            let summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
            assert_eq!(summary["headline"]["name"], leads, "{id}");
        }
    }

    #[test]
    fn a_view_names_rounds_and_heads_the_run_its_way() {
        let f = Fixture::new("orx-monitor-view");
        let run = loss_run(&f, "r1", "exp", "running", 20, 1.0);
        f.collected(T0 + 20_000);
        let (before, _) = monitor_at(&f, &run, T0 + 20_500);
        assert_eq!(before["viewFrom"]["isDefault"], true);
        assert_eq!(
            before["viewFrom"]["markup"],
            "<signal name=\"loss\" />\n<view headline=\"loss\">\n  <chart y=\"loss\" ghost=\"parent\" />\n</view>\n",
            "a default view is shown as the markup that draws it"
        );

        let saved = f.view("exp", VIEW);
        assert_eq!(
            saved["pending"],
            json!(["val_loss"]),
            "declared, not reported yet"
        );
        let (monitor, event) = monitor_at(&f, &run, T0 + 21_000);
        assert_eq!(
            monitor["viewFrom"],
            json!({"experimentId": "exp", "markup": VIEW, "isDefault": false})
        );
        assert_eq!(
            monitor["view"]["blocks"][0],
            json!({"chart": ["loss"], "ghost": "parent"})
        );
        assert_eq!(
            event.unwrap()["reset"],
            true,
            "an open page draws the new view from scratch"
        );
        let summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
        assert_eq!(
            summary["headline"],
            json!({"name": "loss", "label": "Loss", "value": 3.0 / 21.0}),
            "the first headline with a value"
        );
        assert_eq!(
            summary["bindings"]["loss.vs"],
            Value::Null,
            "no parent, no comparison"
        );
        let text = monitor["statusText"].as_str().unwrap();
        assert!(text.contains("signals  Loss 0.143\n"), "{text}");
        assert_eq!(monitor["parent"], Value::Null);
    }

    #[test]
    fn a_labelled_value_reads_as_its_word_in_the_status_and_the_headline() {
        let f = Fixture::new("orx-monitor-labels");
        let run = f.run_in("r1", "exp", "running", local());
        f.collect(&Snapshot {
            run_id: "r1".into(),
            ..folded(&[(
                T0,
                "::orx:: {\"m\":{\"valid\":1,\"front_gain\":0.412}}".into(),
            )])
        });
        f.collected(T0 + 1_000);
        f.view(
            "exp",
            "<signal name=\"valid\" label=\"Bit-exact\" labels='{\"1\":\"valid\",\"0\":\"invalid\"}' />\n<view headline=\"valid\"><stats of=\"valid front_gain\" /></view>",
        );
        let (monitor, _) = monitor_at(&f, &run, T0 + 2_000);

        let text = monitor["statusText"].as_str().unwrap();
        assert!(
            text.contains("signals  Bit-exact valid · front_gain 0.412"),
            "{text}"
        );
        let summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
        assert_eq!(
            summary["headline"],
            json!({"name": "valid", "label": "Bit-exact", "value": 1}),
            "the page words the raw value with the same labels"
        );
        assert_eq!(summary["signals"]["valid"]["labels"]["1"], "valid");
    }

    /// Saves a stage plan as `orx exp view set --config` does.
    fn plan(
        f: &Fixture,
        experiment: &str,
        plan: Value,
    ) -> Result<Value, crate::engine::view::Refusal> {
        let raw = f.store.run_monitor_json(experiment).unwrap();
        let source = crate::engine::view::Source {
            file: None,
            patterns: None,
            config: Some("config.json"),
        };
        crate::engine::view::set(
            &f.store,
            experiment,
            &crate::engine::view::etag(raw.as_deref()),
            None,
            None,
            Some(&json!({ "plan": plan })),
            &source,
        )
    }

    #[test]
    fn periodic_pause_has_a_bounded_allowance_at_matching_steps() {
        let f = Fixture::new("orx-monitor-periodic-pause");
        let parent = f.run_named("p1", "done", local());
        let mut history = Snapshot::new("p1", "1-1", "g1");
        for step in 1..=5000 {
            let t = step + ((step - 1) / 100) * 150;
            fold(
                &mut history,
                &[(
                    T0 + t * 1000,
                    format!(
                        "::orx:: {{\"phase\":\"train\",\"step\":{step},\"of\":5000,\"m\":{{\"loss\":2}},\"t\":{t}}}"
                    ),
                )],
            );
        }
        f.signals(&history);
        monitor_at(&f, &parent, T0 + 13_000_000);
        f.child("child", "exp");
        let mut run = f.run_in("r1", "child", "running", local());
        let mut snapshot = Snapshot::new("r1", "1-1", "g1");
        for step in 1..=100 {
            fold(
                &mut snapshot,
                &[(
                    T0 + step * 1000,
                    format!(
                        "::orx:: {{\"phase\":\"train\",\"step\":{step},\"of\":5000,\"m\":{{\"loss\":2}},\"t\":{step}}}"
                    ),
                )],
            );
        }
        f.signals(&snapshot);
        let (monitor, _) = monitor_at(&f, &run, T0 + 280_000);
        assert_eq!(monitor["progress"]["stallMs"], 0, "{}", monitor["progress"]);
        let (monitor, _) = monitor_at(&f, &run, T0 + 500_000);
        assert!(monitor["progress"]["stallMs"].as_i64().unwrap() > 0);

        for step in 101..=150 {
            let t = step + 150;
            fold(
                &mut snapshot,
                &[(
                    T0 + t * 1000,
                    format!(
                        "::orx:: {{\"phase\":\"train\",\"step\":{step},\"of\":5000,\"m\":{{\"loss\":2}},\"t\":{t}}}"
                    ),
                )],
            );
        }
        f.signals(&snapshot);
        let (monitor, _) = monitor_at(&f, &run, T0 + 480_000);
        assert!(
            monitor["progress"]["stallMs"].as_i64().unwrap() > 0,
            "an off-interval pause is still a stall"
        );
        run.experiment_id = "exp".into();
        f.store.upsert_run(&run).unwrap();
        for step in 151..=200 {
            let t = step + 150;
            fold(&mut snapshot, &[(T0 + t * 1000, format!("::orx:: {{\"phase\":\"train\",\"step\":{step},\"of\":5000,\"m\":{{\"loss\":2}},\"t\":{t}}}"))]);
        }
        f.signals(&snapshot);
        let (monitor, _) = monitor_at(&f, &run, T0 + 530_000);
        assert_eq!(
            monitor["progress"]["stallMs"], 0,
            "a run also learns its own pauses"
        );
    }

    const PIPELINE: [&str; 5] = ["install", "download", "train", "eval", "sft"];

    /// A run going through `stages` of (name, seconds, total), one report a second, from `T0`,
    /// up to `until` seconds; train reports a loss.
    fn staged(
        f: &Fixture,
        id: &str,
        experiment: &str,
        status: &str,
        stages: &[(&str, i64, Option<i64>)],
        until: i64,
    ) -> StoredRun {
        let run = f.run_in(id, experiment, status, local());
        let mut lines = Vec::new();
        let mut second = 0;
        for (name, seconds, of) in stages {
            for step in 0..*seconds {
                if second > until {
                    break;
                }
                let at = T0 + second * 1000;
                let line = match (step, of) {
                    (0, Some(of)) => {
                        format!("::orx:: {{\"phase\":\"{name}\",\"step\":0,\"of\":{of}}}")
                    }
                    (0, None) => format!("::orx:: {{\"phase\":\"{name}\"}}"),
                    (step, Some(_)) if *name == "train" => format!(
                        "::orx:: {{\"step\":{step},\"m\":{{\"loss\":{}}}}}",
                        3.0 / step as f64
                    ),
                    (step, Some(_)) => format!("::orx:: {{\"step\":{step}}}"),
                    (_, None) => format!("{name} working"),
                };
                lines.push((at, line));
                second += 1;
            }
        }
        f.collect(&Snapshot {
            run_id: id.into(),
            ..folded(&lines)
        });
        run
    }

    fn stage_names(monitor: &Value) -> Vec<String> {
        monitor["phases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                let name = p["name"].as_str().unwrap();
                match (p["skipped"] == true, p["planned"] == false) {
                    (true, _) => format!("{name}?skipped"),
                    (_, true) => format!("{name}?unplanned"),
                    _ => name.to_owned(),
                }
            })
            .collect()
    }

    #[test]
    fn a_shorter_child_uses_its_live_rate_and_scales_the_stages_still_to_come() {
        let f = Fixture::new("orx-monitor-scaled-child");
        f.view("exp", VIEW);
        plan(
            &f,
            "exp",
            json!([
                {"phase": "download", "of": 100}, {"phase": "tokenize", "of": 20},
                {"phase": "train", "of": 1000}, {"phase": "eval", "of": 100}
            ]),
        )
        .unwrap();
        let mut parent = f.run_in("p1", "exp", "done", local());
        parent.updated_at = T0 + 1_220_000;
        parent.ended_at = Some(parent.updated_at);
        f.store.upsert_run(&parent).unwrap();
        let lines = |rows: &[(i64, &str)]| {
            rows.iter()
                .map(|(t, line)| (T0 + t, format!("::orx:: {line}")))
                .collect::<Vec<_>>()
        };
        f.collect(&Snapshot {
            run_id: "p1".into(),
            ..folded(&lines(&[
                (0, r#"{"phase":"download","step":0,"of":100,"t":0}"#),
                (100_000, r#"{"step":100,"t":100}"#),
                (100_000, r#"{"phase":"tokenize","step":0,"of":20,"t":100}"#),
                (120_000, r#"{"step":20,"t":120}"#),
                (120_000, r#"{"phase":"train","step":0,"of":1000,"t":120}"#),
                (121_000, r#"{"step":1,"m":{"loss":3},"t":121}"#),
                (1_120_000, r#"{"step":1000,"m":{"loss":1},"t":1120}"#),
                (1_120_000, r#"{"phase":"eval","step":0,"of":100,"t":1120}"#),
                (1_220_000, r#"{"step":100,"t":1220}"#),
            ]))
        });
        monitor_at(&f, &parent, T0 + 1_220_000);
        f.child("child", "exp");
        plan(
            &f,
            "child",
            json!([
                {"phase": "download", "of": 10}, {"phase": "tokenize", "of": 10},
                {"phase": "train", "of": 100}, {"phase": "eval", "of": 20}
            ]),
        )
        .unwrap();
        let child = f.run_in("c1", "child", "running", local());
        let mut reports = lines(&[
            (0, r#"{"phase":"download","step":0,"of":10,"t":0}"#),
            (1_000, r#"{"step":2,"t":1}"#),
        ]);
        f.collect(&Snapshot {
            run_id: "c1".into(),
            ..folded(&reports)
        });
        let (downloading, _) = monitor_at(&f, &child, T0 + 1_100);
        assert_eq!(downloading["setup"], Value::Null);
        assert_eq!(downloading["estimate"]["midMs"], 134_000);
        assert_eq!(
            downloading["phases"]
                .as_array()
                .unwrap()
                .iter()
                .map(|stage| stage["priorMs"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            [10_000, 10_000, 100_000, 20_000]
        );

        reports.extend(lines(&[
            (5_000, r#"{"step":10,"t":5}"#),
            (5_000, r#"{"phase":"tokenize","step":0,"of":10,"t":5}"#),
            (15_000, r#"{"step":10,"t":15}"#),
            (15_000, r#"{"phase":"train","step":0,"of":100,"t":15}"#),
            (15_100, r#"{"step":1,"m":{"loss":1.2},"t":15.1}"#),
        ]));
        f.collect(&Snapshot {
            run_id: "c1".into(),
            ..folded(&reports)
        });
        let (training, _) = monitor_at(&f, &child, T0 + 15_100);
        assert_eq!(training["setup"], Value::Null);
        assert_eq!(training["estimate"]["floor"], false);
        assert_eq!(training["estimate"]["midMs"], 119_000);
    }

    #[test]
    fn a_plan_times_future_stages_and_weights_reported_progress() {
        let f = Fixture::new("orx-monitor-plan");
        let pipeline = [
            ("install", 10, None),
            ("download", 30, Some(30)),
            ("train", 60, Some(60)),
            ("eval", 30, Some(30)),
            ("sft", 70, None),
        ];
        let stages = json!(pipeline
            .iter()
            .map(|(name, _, of)| json!({"phase": name, "of": of}))
            .collect::<Vec<_>>());
        plan(&f, "exp", stages).unwrap();
        let parent = staged(&f, "p1", "exp", "done", &pipeline, 200);
        monitor_at(&f, &parent, T0 + 700_000);
        f.child("child", "exp");

        let child = staged(&f, "c1", "child", "running", &pipeline, 5);
        let (monitor, _) = monitor_at(&f, &child, T0 + 5_500);
        assert_eq!(
            stage_names(&monitor),
            PIPELINE,
            "a child keeps its parent's plan"
        );
        let priors: Vec<_> = monitor["phases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["priorMs"].as_i64())
            .collect();
        assert_eq!(
            priors,
            [
                Some(10_000),
                Some(30_000),
                Some(60_000),
                Some(30_000),
                Some(69_000)
            ],
            "the last stage ends with the log"
        );
        assert_eq!(monitor["setup"]["startsAt"], T0 + 10_000);
        assert_eq!(
            monitor["stageOrder"],
            json!({"index": 0, "count": 5, "first": 1})
        );
        let child_with_points = staged(&f, "c2", "child", "running", &pipeline, 45);
        let (training, _) = monitor_at(&f, &child_with_points, T0 + 45_500);
        assert_eq!(training["setup"], Value::Null);
        assert_eq!(
            training["stageOrder"],
            json!({"index": 2, "count": 5, "first": 1})
        );

        assert_eq!(monitor["progress"]["whole"], Value::Null);
        assert_eq!(monitor["estimate"], Value::Null);

        let child = staged(&f, "c1", "child", "running", &pipeline, 15);
        let (monitor, _) = monitor_at(&f, &child, T0 + 15_500);
        let weights: Vec<_> = monitor["progress"]["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| (s["weight"].as_f64().unwrap() * 100.0).round() / 100.0)
            .collect();
        assert_eq!(weights, [0.05, 0.15, 0.3, 0.15, 0.35]);
        let whole = monitor["progress"]["whole"].as_f64().unwrap();
        assert!(
            (whole - (0.05 + 0.15 * 5.0 / 30.0)).abs() < 0.005,
            "install finished and download reported five of thirty: {whole}"
        );
        let estimate = &monitor["estimate"];
        assert_eq!(estimate["floor"], false, "{estimate}");
        let mid = estimate["midMs"].as_i64().unwrap();
        assert!(
            (180_000..190_000).contains(&mid),
            "download has its own rate; train, eval and sft use their priors: {estimate}"
        );
        let summary = current(&f.store, &child)
            .unwrap()
            .unwrap()
            .summary()
            .clone();
        assert_eq!(summary["segments"].as_array().unwrap().len(), 5);
        let mut stages = status_line(&monitor, "stages").split(" · ");
        assert_eq!(stages.next(), Some("✓ install 10s"));
        assert_eq!(stages.next(), Some("● download"));
        mentions(stages.next().unwrap(), &["○ train", "1m"]);

        // At the end of train, eval and sft are still to come: never 100%.
        let child = staged(&f, "c1", "child", "running", &pipeline, 99);
        let (monitor, _) = monitor_at(&f, &child, T0 + 99_500);
        let whole = monitor["progress"]["whole"].as_f64().unwrap();
        assert!((0.45..0.52).contains(&whole), "{whole}");
        let mid = monitor["estimate"]["midMs"].as_i64().unwrap();
        assert!(
            (95_000..110_000).contains(&mid),
            "eval and sft are left: {}",
            monitor["estimate"]
        );
        let mut stages = status_line(&monitor, "stages").split(" · ");
        assert_eq!(stages.next(), Some("✓ install 10s"));
        assert_eq!(stages.next(), Some("✓ download 30s"));
        assert_eq!(stages.next(), Some("● train"));
        mentions(stages.next().unwrap(), &["○ eval", "30s"]);
    }

    #[test]
    fn finalization_uses_its_parent_until_started_then_its_own_rate() {
        let f = Fixture::new("orx-monitor-finalization");
        plan(
            &f,
            "exp",
            json!([
                {"phase": "train", "of": 2},
                {"phase": "train-finalize", "of": 2}
            ]),
        )
        .unwrap();
        let pipeline = [("train", 3, Some(2)), ("train-finalize", 3, Some(2))];
        let parent = staged(&f, "p1", "exp", "done", &pipeline, 10);
        monitor_at(&f, &parent, T0 + 6_000);
        f.child("child", "exp");

        let child = staged(&f, "r1", "child", "running", &pipeline, 2);
        let (monitor, _) = monitor_at(&f, &child, T0 + 2_500);
        assert_eq!(monitor["phases"][1]["priorMs"], 2_000);
        assert!(
            monitor["estimate"]["midMs"].as_i64().unwrap() > 0,
            "{}",
            monitor["estimate"]
        );

        let child = staged(&f, "r1", "child", "running", &pipeline, 3);
        let (monitor, _) = monitor_at(&f, &child, T0 + 3_500);
        assert_eq!(monitor["phases"][1]["priorMs"], 2_000);
        assert_eq!(monitor["estimate"]["pending"], true);
        assert_eq!(monitor["estimate"]["midMs"], Value::Null);

        let child = staged(&f, "r1", "child", "running", &pipeline, 4);
        f.collected(T0 + 4_500);
        let (monitor, _) = monitor_at(&f, &child, T0 + 4_500);
        assert_eq!(monitor["estimate"]["midMs"], 1_000);
        assert_eq!(monitor["estimate"]["at"], T0 + 4_000);
        mentions(status_line(&monitor, "eta"), &["1s", "left"]);
    }

    #[test]
    fn a_zero_duration_parent_does_not_finish_an_unfinished_finalization() {
        let f = Fixture::new("orx-monitor-instant-finalization");
        plan(
            &f,
            "exp",
            json!([
                {"phase": "train", "of": 2},
                {"phase": "train-finalize", "of": 1}
            ]),
        )
        .unwrap();
        let pipeline = [("train", 3, Some(2)), ("train-finalize", 1, Some(1))];
        let parent = staged(&f, "p1", "exp", "done", &pipeline, 10);
        monitor_at(&f, &parent, T0 + 6_000);
        f.child("child", "exp");
        let child = staged(&f, "c1", "child", "running", &pipeline, 3);
        let (monitor, _) = monitor_at(&f, &child, T0 + 3_500);
        assert!(
            monitor["estimate"]["pending"] == true
                || monitor["estimate"]["midMs"]
                    .as_i64()
                    .is_some_and(|ms| ms > 0),
            "{}",
            monitor["estimate"]
        );
    }

    #[test]
    fn finalization_keeps_step_comparisons_with_an_older_training_only_parent() {
        let f = Fixture::new("orx-monitor-finalization-comparison");
        f.view("exp", VIEW);
        plan(
            &f,
            "exp",
            json!([
                {"phase": "train", "of": 2},
                {"phase": "train-finalize", "of": 2}
            ]),
        )
        .unwrap();
        let parent = staged(&f, "p1", "exp", "done", &[("train", 5, Some(4))], 10);
        monitor_at(&f, &parent, T0 + 10_000);
        f.child("child", "exp");
        let child = staged(
            &f,
            "c1",
            "child",
            "running",
            &[("train", 3, Some(2)), ("train-finalize", 3, Some(2))],
            3,
        );
        let (monitor, _) = monitor_at(&f, &child, T0 + 3_500);
        let comparison = &monitor["versus"]["step"]["loss"];
        assert_eq!(comparison["step"].as_f64(), Some(2.0), "{comparison}");
        assert_eq!(comparison["d"].as_f64(), Some(0.0), "{comparison}");
    }

    #[test]
    fn progress_read_from_the_log_times_only_itself_unless_a_plan_says_what_follows() {
        let f = Fixture::new("orx-monitor-pattern-plan");
        let config: orx_signals::Config = serde_json::from_value(json!({"patterns": [
            {"regex": "^epoch (?P<step>\\d+)/(?P<of>\\d+) loss (?P<loss>\\S+)$", "phase": "train", "unit": "epochs"},
        ]}))
        .unwrap();
        let mut snapshot = Snapshot::new("r1", "1-1", "g1");
        snapshot.configure(Some(Arc::new(
            orx_signals::Patterns::compile(&config).unwrap(),
        )));
        let lines: Vec<(i64, String)> = (1..=20)
            .map(|epoch| (T0 + epoch * 1000, format!("epoch {epoch}/150 loss 0.3")))
            .collect();
        fold(&mut snapshot, &lines);
        let run = f.run("running", local());
        f.signals(&snapshot);
        f.collected(T0 + 20_000);
        let (monitor, _) = monitor_at(&f, &run, T0 + 20_500);
        assert_eq!(
            monitor["estimate"]["floor"], true,
            "{}",
            monitor["estimate"]
        );

        plan(
            &f,
            "exp",
            json!([{"phase": "train", "of": 150, "unit": "epochs"}]),
        )
        .unwrap();
        let (monitor, _) = monitor_at(&f, &run, T0 + 21_500);
        assert_eq!(
            monitor["estimate"]["floor"], false,
            "a declared plan says nothing follows: {}",
            monitor["estimate"]
        );
    }

    #[test]
    fn a_stage_no_earlier_run_timed_leaves_the_whole_unknown_and_time_left_a_floor() {
        let f = Fixture::new("orx-monitor-plan-unknown");
        let stages = json!(PIPELINE
            .iter()
            .map(|name| json!({"phase": name}))
            .collect::<Vec<_>>());
        plan(&f, "exp", stages).unwrap();
        let pipeline = [
            ("install", 10, None),
            ("download", 30, Some(30)),
            ("train", 60, Some(60)),
        ];
        let run = staged(&f, "r1", "exp", "running", &pipeline, 99);
        let (monitor, _) = monitor_at(&f, &run, T0 + 99_500);
        assert_eq!(
            monitor["progress"]["whole"],
            Value::Null,
            "no equal split stands in for unknown weights"
        );
        assert_eq!(monitor["progress"]["segments"].as_array().unwrap().len(), 1);
        assert_eq!(
            monitor["estimate"]["floor"], true,
            "{}",
            monitor["estimate"]
        );
        assert_eq!(monitor["estimate"]["untimed"], json!(["eval", "sft"]));
        let text = monitor["statusText"].as_str().unwrap();
        assert!(
            text.starts_with("baseline  running  train (3/5) \u{b7} step 59 of 60"),
            "{text}"
        );
        mentions(
            status_line(&monitor, "eta"),
            &["No estimate", "timed", "eval", "sft"],
        );

        // Its last step done, how long eval and sft take is all that is left, and unknown.
        let pipeline = [
            ("install", 10, None),
            ("download", 30, Some(30)),
            ("train", 61, Some(60)),
        ];
        let run = staged(&f, "r1", "exp", "running", &pipeline, 100);
        let (monitor, _) = monitor_at(&f, &run, T0 + 100_500);
        assert_eq!(
            (
                &monitor["estimate"]["hiMs"],
                &monitor["estimate"]["untimed"]
            ),
            (&json!(0), &json!(["eval", "sft"])),
            "{}",
            monitor["estimate"]
        );
        mentions(
            status_line(&monitor, "eta"),
            &["No estimate", "timed", "eval", "sft"],
        );
    }

    #[test]
    fn setup_is_timed_to_the_first_signal_stage_even_when_a_later_stage_reuses_it() {
        let f = Fixture::new("orx-monitor-setup-first-stage");
        f.view(
            "exp",
            "<signal name=\"loss\"/><view headline=\"loss\"><chart y=\"loss\"/></view>",
        );
        let parent = f.run_in("p1", "exp", "done", local());
        let snapshot = Snapshot {
            run_id: "p1".into(),
            ..folded(&[
                (T0, "::orx:: {\"phase\":\"setup\",\"of\":4,\"t\":0}".into()),
                (
                    T0 + 4_000,
                    "::orx:: {\"phase\":\"train\",\"of\":10,\"t\":4}".into(),
                ),
                (
                    T0 + 5_000,
                    "::orx:: {\"step\":1,\"t\":5,\"m\":{\"loss\":3}}".into(),
                ),
                (
                    T0 + 14_000,
                    "::orx:: {\"phase\":\"sft\",\"step\":0,\"of\":10,\"t\":14,\"m\":{\"loss\":2}}"
                        .into(),
                ),
                (
                    T0 + 24_000,
                    "::orx:: {\"step\":10,\"t\":24,\"m\":{\"loss\":1}}".into(),
                ),
            ])
        };
        assert_eq!(snapshot.first_series_stages["loss"], Some(1));
        assert_eq!(snapshot.series_stages["loss"], 2);
        f.collect(&snapshot);
        monitor_at(&f, &parent, T0 + 700_000);
        f.child("child", "exp");
        plan(&f, "child", json!([{"phase": "setup", "of": 4}, {"phase": "train", "of": 10}, {"phase": "sft", "of": 10}])).unwrap();
        let child = f.run_in("c1", "child", "running", local());
        f.collect(&Snapshot {
            run_id: child.id.clone(),
            ..folded(&reports(&[r#"{"phase":"setup","of":4}"#]))
        });
        let (monitor, _) = monitor_at(&f, &child, T0 + 1_500);
        assert_eq!(monitor["setup"]["startsAt"], T0 + 4_000);
        assert_eq!(
            monitor["stageOrder"],
            json!({"index": 0, "count": 3, "first": 1})
        );
        assert_eq!(
            current(&f.store, &child).unwrap().unwrap().summary()["stageOrder"],
            monitor["stageOrder"]
        );
    }

    /// A parent planned without label and train, which got its first displayed point in eval, and
    /// a child that plans both; `block` is the child's view of `dev_loss`.
    fn labelled_child(f: &Fixture, block: &str) {
        f.view(
            "exp",
            &format!(
                "<signal name=\"wer\" better=\"lower\"/><signal name=\"dev_loss\" better=\"lower\"/>\
                 <view headline=\"wer\">{block}<stats of=\"wer\"/></view>"
            ),
        );
        let stages = |names: &[&str]| {
            json!(names
                .iter()
                .map(|name| json!({"phase": name}))
                .collect::<Vec<_>>())
        };
        plan(f, "exp", stages(&["install", "download", "eval"])).unwrap();
        let parent = f.run_in("p1", "exp", "done", local());
        f.collect(&Snapshot {
            run_id: "p1".into(),
            ..folded(&reports(&[
                r#"{"phase":"install"}"#,
                r#"{"phase":"download"}"#,
                r#"{"phase":"eval","m":{"wer":0.05}}"#,
            ]))
        });
        monitor_at(f, &parent, T0 + 700_000);
        f.child("child", "exp");
        plan(
            f,
            "child",
            stages(&["install", "download", "label", "train", "eval"]),
        )
        .unwrap();
    }

    /// One report a second from `T0`.
    fn reports(lines: &[&str]) -> Vec<(i64, String)> {
        (0..)
            .zip(lines)
            .map(|(second, line)| (T0 + 1000 * second, format!("::orx:: {line}")))
            .collect()
    }

    /// The stage order of a child run that has reported `lines`, at the second after the last.
    fn order_after(f: &Fixture, id: &str, lines: &[&str]) -> Value {
        let run = f.run_in(id, "child", "running", local());
        f.collect(&Snapshot {
            run_id: id.into(),
            ..folded(&reports(lines))
        });
        let (monitor, _) = monitor_at(f, &run, T0 + 1000 * lines.len() as i64);
        json!({"order": monitor["stageOrder"], "setup": monitor["setup"].is_object(), "startsAt": monitor["setup"]["startsAt"]})
    }

    const LABEL: [&str; 3] = [
        r#"{"phase":"install"}"#,
        r#"{"phase":"download"}"#,
        r#"{"phase":"label"}"#,
    ];

    #[test]
    fn setup_ends_at_the_first_stage_the_view_charts_even_where_the_parent_had_none() {
        let f = Fixture::new("orx-monitor-first-viewed");
        labelled_child(&f, r#"<chart y="dev_loss" phase="train"/>"#);

        let downloading = order_after(&f, "c0", &LABEL[..2]);
        assert_eq!(
            downloading["order"],
            json!({"index": 1, "count": 5, "first": 3})
        );
        assert_eq!(
            downloading["startsAt"],
            Value::Null,
            "no earlier run timed label, which comes before train"
        );
        let labelling = order_after(&f, "c1", &LABEL);
        assert_eq!(
            labelling,
            json!({"order": {"index": 2, "count": 5, "first": 3}, "setup": true, "startsAt": null}),
            "label is still untimed"
        );
        let mut training = LABEL.to_vec();
        training.push(r#"{"phase":"train","of":272}"#);
        let waiting = order_after(&f, "c2", &training);
        assert_eq!(
            waiting,
            json!({"order": {"index": 3, "count": 5, "first": 3}, "setup": true, "startsAt": T0 + 3_000}),
            "train began and has yet to report a point"
        );

        training.push(r#"{"step":16,"m":{"dev_loss":0.3}}"#);
        assert_eq!(
            order_after(&f, "c3", &training),
            json!({"order": {"index": 3, "count": 5, "first": 3}, "setup": false, "startsAt": null}),
            "the first point came where it was expected"
        );

        let mut missed = LABEL.to_vec();
        missed.extend([
            r#"{"phase":"train","of":272}"#,
            r#"{"phase":"eval","of":2}"#,
        ]);
        assert_eq!(
            order_after(&f, "c4", &missed)["order"],
            json!({"index": 4, "count": 5, "first": 4}),
            "train passed with no point, so the first comes no earlier than eval"
        );
    }

    #[test]
    fn a_phased_stat_can_end_setup_without_being_a_headline_or_chart() {
        let f = Fixture::new("orx-monitor-first-stat");
        labelled_child(&f, r#"<stats of="dev_loss" phase="train"/>"#);

        let mut training = LABEL.to_vec();
        training.push(r#"{"phase":"train","of":272}"#);
        let waiting = order_after(&f, "c1", &training);
        assert_eq!(waiting["order"], json!({"index":3,"count":5,"first":3}));
        assert_eq!(waiting["setup"], true);

        training.push(r#"{"m":{"dev_loss":0.3}}"#);
        let reported = order_after(&f, "c2", &training);
        assert_eq!(reported["order"], waiting["order"]);
        assert_eq!(reported["setup"], false);
    }

    #[test]
    fn a_view_that_names_no_stage_leaves_setup_to_end_where_the_parent_got_its_first_point() {
        let f = Fixture::new("orx-monitor-first-parent");
        labelled_child(&f, r#"<chart y="dev_loss"/>"#);

        let labelling = order_after(&f, "c1", &LABEL);
        assert_eq!(
            labelling["order"],
            json!({"index": 2, "count": 5, "first": 4})
        );
        let mut training = LABEL.to_vec();
        training.extend([
            r#"{"phase":"train","step":0,"of":272}"#,
            r#"{"step":16,"m":{"dev_loss":0.3}}"#,
        ]);
        assert_eq!(
            order_after(&f, "c3", &training)["order"],
            json!({"index": 3, "count": 5, "first": 3}),
            "a point can still correct the historical guess"
        );

        let mut evaluating = LABEL.to_vec();
        evaluating.extend([
            r#"{"phase":"train","of":272}"#,
            r#"{"phase":"eval","of":2,"m":{"wer":0.04}}"#,
        ]);
        assert_eq!(
            order_after(&f, "c2", &evaluating),
            json!({"order": {"index": 4, "count": 5, "first": 4}, "setup": false, "startsAt": null}),
            "the first point came where the parent's did"
        );
    }

    #[test]
    fn a_count_makes_a_named_stage_work_before_it_has_any_points() {
        let f = Fixture::new("orx-monitor-count-first");
        plan(
            &f,
            "exp",
            json!([{"phase": "train", "of": 20, "unit": "steps"}]),
        )
        .unwrap();
        let run = f.run("running", local());
        f.collect(&folded(&reports(&[
            r#"{"phase":"train","step":0}"#,
            r#"{"step":2}"#,
        ])));

        let (body, _) = monitor_at(&f, &run, T0 + 1_000);
        let summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
        assert!(body["latest"].as_object().unwrap().is_empty());
        assert_eq!(body["setup"], Value::Null);
        assert_eq!(summary["setup"], body["setup"]);
        assert_eq!(
            body["stageOrder"],
            json!({"index": 0, "count": 1, "first": 0})
        );
        assert_eq!(summary["stageOrder"], body["stageOrder"]);
        assert_eq!(summary["stage"]["done"], 2.0);
        assert_eq!(summary["stage"]["unit"], "steps");
        assert_eq!(summary["whole"], 0.1);
        assert_eq!(summary["estimate"]["midMs"], 9_000);
        mentions(
            body["statusText"].as_str().unwrap().lines().next().unwrap(),
            &["train (2/2)", "step 2 of 20"],
        );
        mentions(status_line(&body, "eta"), &["9s left"]);
    }

    #[test]
    fn a_bare_step_count_ends_setup_before_any_metrics() {
        let f = Fixture::new("orx-monitor-bare-count");
        let run = f.run("running", local());
        let lines = reports(&[r#"{"of":100}"#, r#"{"step":10,"of":100,"spend":1,"t":10}"#]);
        f.collect(&folded(&lines[..1]));
        let (waiting, _) = monitor_at(&f, &run, T0);
        assert!(waiting["setup"].is_object());

        f.collect(&folded(&lines));
        let (working, _) = monitor_at(&f, &run, T0 + 1_000);
        let summary = current(&f.store, &run).unwrap().unwrap();
        assert!(working["latest"].as_object().unwrap().is_empty());
        assert_eq!(working["setup"], Value::Null);
        assert_eq!(summary.summary()["setup"], working["setup"]);
        assert_eq!(working["stageOrder"]["first"], 0);
        assert_eq!(working["progress"]["done"], 10.0);
        mentions(
            working["statusText"]
                .as_str()
                .unwrap()
                .lines()
                .next()
                .unwrap(),
            &["running", "step 10 of 100"],
        );
    }

    #[test]
    fn setup_stays_ended_when_the_first_counted_stage_leaves_the_checkpoint() {
        let f = Fixture::new("orx-monitor-omitted-count");
        let run = f.run("running", local());
        f.collect(&folded(&reports(&[r#"{"step":10,"of":100}"#])));
        let (before, _) = monitor_at(&f, &run, T0);
        assert_eq!(before["setup"], Value::Null);

        let [path, _] = signal_paths(f.dir.path(), &run.id);
        let mut snapshot = super::super::adapters::local::signals::load(&path)
            .unwrap()
            .unwrap();
        let later: Vec<_> = (1..=65)
            .map(|n| (T0 + n * 1000, format!(r#"::orx:: {{"phase":"eval-{n}"}}"#)))
            .collect();
        fold(&mut snapshot, &later);
        f.collect(&snapshot);
        f.collected(T0 + 65_000);

        let (body, _) = monitor_at(&f, &run, T0 + 65_000);
        assert_eq!(body["setup"], Value::Null);
        assert_eq!(body["stageOrder"]["first"], 0);
        let summary = current(&f.store, &run).unwrap().unwrap();
        assert_eq!(summary.summary()["setup"], Value::Null);
    }

    #[test]
    fn the_first_observed_stage_stays_first_when_the_other_report_arrives() {
        let f = Fixture::new("orx-monitor-first-observed");
        f.child("child", "exp");
        plan(
            &f,
            "child",
            json!([{"phase": "install"}, {"phase": "train"}, {"phase": "eval"}]),
        )
        .unwrap();
        for (id, first, next) in [
            (
                "count-first",
                r#"{"phase":"train","step":1,"of":20}"#,
                r#"{"phase":"eval","m":{"loss":0.5}}"#,
            ),
            (
                "point-first",
                r#"{"phase":"train","m":{"loss":0.5}}"#,
                r#"{"phase":"eval","step":1,"of":20}"#,
            ),
            (
                "unplanned",
                r#"{"phase":"tokenize","step":1,"of":20}"#,
                r#"{"phase":"train","m":{"loss":0.5}}"#,
            ),
        ] {
            let before = order_after(
                &f,
                &format!("{id}-first"),
                &[r#"{"phase":"install"}"#, first],
            );
            assert_eq!(
                before,
                json!({"order": {"index": 1, "count": if id == "unplanned" { 4 } else { 3 }, "first": 1}, "setup": false, "startsAt": null}),
                "{id}"
            );
            let after = order_after(&f, id, &[r#"{"phase":"install"}"#, first, next]);
            assert_eq!(after["order"]["first"], before["order"]["first"], "{id}");
            assert_eq!(after["order"]["index"], 2, "{id}");
            assert_eq!(after["setup"], false, "{id}");
        }
    }

    #[test]
    fn a_plain_progress_bar_is_work_without_a_stage_plan_or_metrics() {
        let f = Fixture::new("orx-monitor-plain-progress");
        let run = f.run("running", local());
        let mut snapshot = Snapshot::new("r1", "1-1", "g1");
        snapshot
            .feed(
                b"step 00005/00100 | loss 4.2\nstep 00010/00100 | loss 4.0\n",
                true,
                T0 + 1_000,
                true,
            )
            .unwrap();
        f.collect(&snapshot);

        let (body, _) = monitor_at(&f, &run, T0 + 1_000);
        assert_eq!(body["progress"]["done"], 10.0);
        assert!(body["setup"].is_null(), "{}", body["setup"]);
        assert_eq!(
            body["stageOrder"],
            json!({"index": 0, "count": null, "first": 0})
        );
        let summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
        assert!(summary["setup"].is_null());
        assert_eq!(summary["stageOrder"], body["stageOrder"]);
    }

    #[test]
    fn a_run_without_a_plan_is_timed_from_its_parents_stages_and_stalls_before_its_first_step() {
        let f = Fixture::new("orx-monitor-unplanned-prior");
        let pipeline = [("setup", 4, Some(4)), ("train", 60, Some(60))];
        let parent = staged(&f, "p1", "exp", "done", &pipeline, 100);
        monitor_at(&f, &parent, T0 + 700_000);
        f.child("child", "exp");

        // Train reported step 0 at four seconds and nothing since.
        let child = staged(&f, "c1", "child", "running", &pipeline, 4);
        let (monitor, _) = monitor_at(&f, &child, T0 + 60_000);
        assert_eq!(monitor["progress"]["stallMs"], 0, "{}", monitor["progress"]);
        assert_eq!(monitor["phases"][1]["priorMs"], 59_000);
        assert_eq!(
            monitor["progress"]["whole"],
            Value::Null,
            "with no plan, what comes after train is unknown"
        );
        let (monitor, _) = monitor_at(&f, &child, T0 + 100_000);
        assert_eq!(
            monitor["progress"]["stallMs"], 96_000,
            "past 90s with the parent's pace to go by: {}",
            monitor["progress"]
        );
        assert_eq!(monitor["estimate"]["pending"], true);
        assert_eq!(monitor["estimate"]["midMs"], Value::Null);
    }

    #[test]
    fn the_parents_line_goes_under_the_stage_the_child_counts_steps_in() {
        let f = Fixture::new("orx-monitor-ghost-stage");
        f.view("exp", VIEW);
        let run = |id: &str, experiment: &str, status: &str, lines: &[&str]| {
            let run = f.run_in(id, experiment, status, local());
            f.collect(&Snapshot {
                run_id: id.into(),
                ..folded(
                    &lines
                        .iter()
                        .enumerate()
                        .map(|(i, line)| (T0 + 1000 * i as i64, (*line).to_owned()))
                        .collect::<Vec<_>>(),
                )
            });
            run
        };
        let download = "::orx:: {\"phase\":\"download\",\"step\":1,\"of\":3}";
        let train = [
            "::orx:: {\"phase\":\"train\",\"step\":0,\"of\":600}",
            "::orx:: {\"step\":300,\"m\":{\"loss\":2}}",
            "::orx:: {\"step\":600,\"m\":{\"loss\":1}}",
        ];
        let mut whole = vec![download];
        whole.extend(train);
        whole.push("::orx:: {\"phase\":\"eval\",\"m\":{\"loss\":0.9}}");
        let parent = run("p1", "exp", "done", &whole);
        monitor_at(&f, &parent, T0 + 700_000);
        f.child("child", "exp");

        let child = run("c1", "child", "running", &[download]);
        let (monitor, _) = monitor_at(&f, &child, T0 + 1_500);
        assert_eq!(
            monitor["parent"]["series"],
            json!({}),
            "train's line has no place on download's three steps"
        );
        let child = run("c2", "child", "running", &[download, train[0], train[1]]);
        let (monitor, _) = monitor_at(&f, &child, T0 + 3_500);
        assert_eq!(
            monitor["parent"]["series"]["loss"],
            json!([[300.0, 2.0], [600.0, 1.0]]),
            "the parent's eval value has no step to sit at"
        );
        let mut ended = vec![download];
        ended.extend(train);
        ended.push("::orx:: {\"phase\":\"eval\",\"step\":1,\"of\":6,\"m\":{\"score\":0.5}}");
        let child = run("c3", "child", "done", &ended);
        let (monitor, _) = monitor_at(&f, &child, T0 + 700_000);
        assert_eq!(
            monitor["parent"]["series"]["loss"],
            json!([[300.0, 2.0], [600.0, 1.0]]),
            "in eval the child's loss is still train's, and so is the parent's under it"
        );
    }

    #[test]
    fn a_parent_is_compared_only_in_the_stage_the_child_is_in() {
        let f = Fixture::new("orx-monitor-versus-stage");
        f.view("exp", VIEW);
        let run = |id: &str, experiment: &str, status: &str, lines: &[&str]| {
            let run = f.run_in(id, experiment, status, local());
            f.collect(&Snapshot {
                run_id: id.into(),
                ..folded(
                    &lines
                        .iter()
                        .enumerate()
                        .map(|(i, line)| (T0 + 1000 * i as i64, (*line).to_owned()))
                        .collect::<Vec<_>>(),
                )
            });
            run
        };
        let stage = |name: &str, loss: f64| {
            [
                format!("::orx:: {{\"phase\":\"{name}\",\"step\":0,\"of\":600}}"),
                format!("::orx:: {{\"step\":300,\"m\":{{\"loss\":{loss}}}}}"),
                format!("::orx:: {{\"step\":600,\"m\":{{\"loss\":{loss}}}}}"),
            ]
        };
        let lines = |stages: &[[String; 3]]| -> Vec<String> { stages.concat() };
        fn refs(lines: &[String]) -> Vec<&str> {
            lines.iter().map(String::as_str).collect()
        }
        // Pretrain, then a finetune whose loss restarts at step 0: a finished parent's loss is finetune's.
        let parent = lines(&[stage("pretrain", 2.0), stage("finetune", 1.2)]);
        monitor_at(&f, &run("p1", "exp", "done", &refs(&parent)), T0 + 700_000);
        f.child("child", "exp");
        let child = lines(&[stage("pretrain", 6.0)]);
        let (monitor, _) = monitor_at(
            &f,
            &run("c1", "child", "running", &refs(&child)),
            T0 + 3_500,
        );
        assert_eq!(
            monitor["versus"]["step"].get("loss"),
            None,
            "the parent's loss now is finetune's, not pretrain's: {}",
            monitor["versus"]
        );

        // The same stage name again later is another stage: train, eval, then train once more.
        let parent = lines(&[stage("train", 2.0), stage("eval", 0.5), stage("train", 1.2)]);
        monitor_at(&f, &run("p2", "exp", "done", &refs(&parent)), T0 + 700_000);
        f.child("child2", "exp");
        let first_train = lines(&[stage("train", 6.0)]);
        let (monitor, _) = monitor_at(
            &f,
            &run("c2", "child2", "running", &refs(&first_train)),
            T0 + 3_500,
        );
        assert_eq!(
            monitor["versus"]["step"].get("loss"),
            None,
            "{}",
            monitor["versus"]
        );
        assert_eq!(monitor["parent"]["series"], json!({}));
    }

    #[test]
    fn a_stage_going_far_slower_than_the_parents_says_by_how_much() {
        let f = Fixture::new("orx-monitor-pace");
        let parent = staged(&f, "p1", "exp", "done", &[("train", 60, Some(60))], 100);
        monitor_at(&f, &parent, T0 + 700_000);
        f.child("child", "exp");
        let paced = |id: &str, seconds_per_step: i64| {
            let run = f.run_in(id, "child", "running", local());
            f.collect(&Snapshot {
                run_id: id.into(),
                ..folded(
                    &(0..=20)
                        .map(|step| {
                            (
                                T0 + step * seconds_per_step * 1000,
                                format!(
                                    "::orx:: {{\"phase\":\"train\",\"step\":{step},\"of\":60}}"
                                ),
                            )
                        })
                        .collect::<Vec<_>>(),
                )
            });
            let (monitor, _) = monitor_at(&f, &run, T0 + 20 * seconds_per_step * 1000 + 500);
            monitor
        };
        let slow = paced("slow", 3);
        assert_eq!(slow["pace"], json!({"times": 3.0, "than": "parent"}));
        let text = slow["statusText"].as_str().unwrap();
        mentions(text, &["3\u{d7}", "slower", "parent"]);
        assert_eq!(paced("level", 1)["pace"], Value::Null);
    }

    #[test]
    fn a_stage_is_timed_by_the_experiments_own_last_run_before_its_parents() {
        let f = Fixture::new("orx-monitor-earlier-prior");
        let parent = [
            ("setup", 4, Some(4)),
            ("train", 60, Some(60)),
            ("eval", 10, Some(10)),
        ];
        let stages = json!(parent
            .iter()
            .map(|(name, _, of)| json!({"phase": name, "of": of}))
            .collect::<Vec<_>>());
        plan(&f, "exp", stages).unwrap();
        let parent = staged(&f, "p1", "exp", "done", &parent, 200);
        monitor_at(&f, &parent, T0 + 700_000);
        f.child("child", "exp");
        let own = [("setup", 4, Some(4)), ("train", 30, Some(60))];
        // The run before this one: a run's first write fixes when it was created.
        f.store
            .upsert_run(&StoredRun {
                id: "e1".into(),
                experiment_id: "child".into(),
                created_at: T0 - 1_000_000,
                ..f.run_in("template", "elsewhere", "done", local())
            })
            .unwrap();
        staged(&f, "e1", "child", "done", &own, 200);

        let child = staged(&f, "c1", "child", "running", &own, 2);
        let (monitor, _) = monitor_at(&f, &child, T0 + 2_500);
        let priors: Vec<_> = monitor["phases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["priorMs"].clone())
            .collect();
        assert_eq!(
            priors,
            [json!(4_000), json!(29_000), json!(9_000)],
            "its own last run timed setup and train; only the parent went through eval"
        );
    }

    #[test]
    fn a_stage_that_printed_no_total_reads_the_one_its_plan_gives() {
        let f = Fixture::new("orx-monitor-plan-total");
        plan(
            &f,
            "exp",
            json!([{"phase": "train", "of": 3}, {"phase": "eval", "of": 10}]),
        )
        .unwrap();
        let run = f.run("running", local());
        let mut lines: Vec<(i64, String)> = (0..=3)
            .map(|n| {
                (
                    T0 + 1000 * n,
                    format!("::orx:: {{\"phase\":\"train\",\"step\":{n},\"of\":3}}"),
                )
            })
            .collect();
        lines.push((T0 + 4000, "::orx:: {\"phase\":\"eval\",\"step\":0}".into()));
        lines.push((T0 + 5000, "::orx:: {\"step\":4}".into()));
        f.collect(&folded(&lines));
        let (monitor, _) = monitor_at(&f, &run, T0 + 5_500);
        let text = monitor["statusText"].as_str().unwrap();
        assert!(
            text.contains("  eval (3/3) \u{b7} step 4 of 10  "),
            "{text}"
        );
        let summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
        assert_eq!(summary["stage"]["of"], 10.0);
    }

    #[test]
    fn a_run_that_reports_in_chunks_is_almost_done_past_its_midpoint_until_it_is_late() {
        let monitor = |total, now| {
            let snapshot = folded(
                &(0..=3)
                    .map(|n| {
                        let step = 10 * n;
                        let line =
                            format!("::orx:: {{\"step\":{step},\"of\":{total},\"t\":{step}}}");
                        (T0 + 10_000 * n, line)
                    })
                    .collect::<Vec<_>>(),
            );
            let f = Fixture::new("orx-monitor-chunks");
            let run = f.run("running", local());
            f.collect(&snapshot);
            f.collected(now);
            monitor_at(&f, &run, now).0
        };

        let waiting = monitor(40, T0 + 40_200);
        assert_eq!(status_line(&waiting, "eta"), "Almost done");
        let late = monitor(40, T0 + 41_000);
        mentions(
            status_line(&late, "eta"),
            &["No progress for 11s", "was due"],
        );

        let now = T0 + 130_200;
        let stalled = monitor(130, now);
        assert!(stalled["progress"]["stallMs"].as_i64().unwrap() > 0);
        let estimate = &stalled["estimate"];
        assert!(now < estimate["at"].as_i64().unwrap() + estimate["hiMs"].as_i64().unwrap());
        mentions(
            status_line(&stalled, "eta"),
            &["No progress for", "was due"],
        );
    }

    #[test]
    fn a_counter_floor_expires_below_one_second_at_the_midpoint() {
        let snapshot = folded(
            &(1..=3)
                .map(|n| (T0 + 6_000 * n, format!("chunk {n}/10 done, {}s", 124 * n)))
                .collect::<Vec<_>>(),
        );
        for (now, eta) in [
            (T0 + 59_000, &["At least", "1s", "left"][..]),
            (T0 + 59_001, &["No estimate"][..]),
        ] {
            let f = Fixture::new("orx-monitor-floor-out");
            let run = f.run("running", local());
            f.collect(&snapshot);
            f.collected(now);
            let (monitor, _) = monitor_at(&f, &run, now);
            let line = status_line(&monitor, "eta");
            mentions(line, eta);
            mentions(line, &["progress in the log", "more work"]);
        }
    }

    #[test]
    fn a_run_shows_its_unplanned_stage_and_the_ones_it_skipped() {
        let f = Fixture::new("orx-monitor-plan-skips");
        let stages = json!(PIPELINE
            .iter()
            .map(|name| json!({"phase": name}))
            .collect::<Vec<_>>());
        plan(&f, "exp", stages).unwrap();
        let pipeline = [
            ("install", 10, None),
            ("train", 20, Some(20)),
            ("export", 10, Some(10)),
            ("eval", 10, Some(10)),
        ];
        let run = staged(&f, "r1", "exp", "done", &pipeline, 60);
        let (monitor, _) = monitor_at(&f, &run, T0 + 700_000);
        assert_eq!(
            stage_names(&monitor),
            [
                "install",
                "download?skipped",
                "train",
                "export?unplanned",
                "eval",
                "sft?skipped"
            ]
        );
        assert_eq!(monitor["progress"]["phase"], 4);
        assert_eq!(
            monitor["progress"]["whole"], 1.0,
            "a run that finished did all it had to"
        );
        assert!(monitor["statusText"]
            .as_str()
            .unwrap()
            .starts_with("baseline  done  100%"));
        let text = monitor["statusText"].as_str().unwrap();
        assert!(text.contains("\u{2013} download skipped"), "{text}");
        assert!(text.contains("\u{2713} export 10s (unplanned)"), "{text}");
        assert!(
            text.contains("\u{2713} eval 9s \u{b7} \u{2013} sft skipped"),
            "{text}"
        );
    }

    #[test]
    fn a_plan_the_run_reports_only_as_rows_is_said_to_disagree() {
        let f = Fixture::new("orx-monitor-plan-rows");
        plan(
            &f,
            "exp",
            json!([{"phase": "bench", "of": 16, "unit": "cases"}]),
        )
        .unwrap();
        let run = f.run_in("r1", "exp", "done", local());
        let lines: Vec<(i64, String)> = (0..20)
            .map(|case| {
                let line =
                    format!("::orx:: {{\"row\":\"cases\",\"m\":{{\"n\":{case},\"correct\":1}}}}");
                (T0 + case * 1000, line)
            })
            .collect();
        f.collect(&Snapshot {
            run_id: "r1".into(),
            ..folded(&lines)
        });
        let (monitor, _) = monitor_at(&f, &run, T0 + 60_000);
        assert_eq!(
            monitor["disagrees"],
            json!({"stage": "bench", "of": 16, "unit": "cases", "table": "cases", "rows": 20})
        );
        let warning = status_line(&monitor, "plan");
        mentions(
            warning,
            &["16 cases", "bench", "20 rows", "cases", "no step"],
        );

        let counted = staged(&f, "r2", "exp", "done", &[("bench", 16, Some(16))], 60);
        assert_eq!(
            monitor_at(&f, &counted, T0 + 60_000).0["disagrees"],
            Value::Null
        );
    }

    #[test]
    fn a_flag_raised_once_the_run_is_done_colours_it_and_waits_for_the_user() {
        let f = Fixture::new("orx-monitor-flag-after-end");
        let run = f.run("done", local());
        f.store.put_chat_session("chat-1", "claude-code");
        f.store.set_chat_session_archived("chat-1", true).unwrap();
        f.store.conn_execute_for_tests(
            "UPDATE runs SET chat_session_id = 'chat-1', ended_at = 1 WHERE id = 'r1'",
        );
        f.collect(&folded(&declared(0..=20, 20)));
        let row = f.store.get_run("r1").unwrap().unwrap();
        let body = json!({"id": "f1", "source": "look", "at": T0 + 30_000, "template": "**12** of 20 cases report correct 0.",
            "detail": "**12** of 20 cases report correct 0.", "text": "**12** of 20 cases report correct 0.", "evidence": null});
        let flag = super::super::flags::new_flag(&f.store, &row, body, true).unwrap();
        f.store.raise_flags(&[flag], T0 + 30_000).unwrap();
        let (monitor, _) = monitor_at(&f, &run, T0 + 31_000);
        let summary = current(&f.store, &row).unwrap().unwrap().summary().clone();
        assert_eq!(summary["tone"], "flag");
        assert_eq!(summary["flag"]["id"], "f1");
        assert_eq!(summary["flag"]["state"], "waiting");
        assert_eq!(monitor["flagId"], monitor["flags"][0]["id"]);
        assert_eq!(monitor["flags"][0]["state"], "waiting");
    }

    #[test]
    fn a_run_declares_its_own_plan_over_its_experiments() {
        let f = Fixture::new("orx-monitor-plan-wire");
        plan(&f, "exp", json!([{"phase": "train"}])).unwrap();
        let run = f.run("running", local());
        f.collect(&folded(&[
            (T0, "::orx:: {\"plan\":[{\"phase\":\"warmup\"},{\"phase\":\"train\",\"of\":5000,\"unit\":\"steps\"}]}".into()),
            (T0 + 1000, "::orx:: {\"phase\":\"warmup\"}".into()),
        ]));
        let (monitor, _) = monitor_at(&f, &run, T0 + 1500);
        assert_eq!(stage_names(&monitor), ["warmup", "train"]);
        assert_eq!(monitor["phases"][1]["of"], 5000.0);
        assert_eq!(monitor["phases"][1]["unit"], "steps");
        assert_eq!(monitor["progress"]["phase"], 0);
    }

    #[test]
    fn saving_settings_acknowledges_the_edit_with_an_unreadable_view() {
        use crate::engine::view::{etag, set, Source};

        let f = Fixture::new("orx-settings-save");
        let previous = f
            .store
            .patch_run_monitor_json("exp", &etag(None), |_| {
                Ok(json!({"view": "<view><future-block/></view>"}).to_string())
            })
            .unwrap()
            .unwrap();
        let plan = json!([{"phase": "eval"}]);
        let saved = set(
            &f.store,
            "exp",
            &etag(Some(&previous)),
            None,
            None,
            Some(&json!({"plan": plan})),
            &Source {
                file: None,
                patterns: None,
                config: Some("plan.json"),
            },
        );
        let raw = f.store.run_monitor_json("exp").unwrap().unwrap();
        let settings: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(settings["plan"], plan);
        assert_eq!(settings["view"], "<view><future-block/></view>");
        let saved = saved.unwrap();
        assert_eq!(saved["etag"], etag(Some(&raw)));
        assert_eq!(saved["plan"], plan);
    }

    #[cfg(unix)]
    #[test]
    fn reading_a_view_gets_all_settings_from_before_a_concurrent_save() {
        use crate::engine::view::{described, etag};
        use std::io::Write;
        use std::os::unix::ffi::OsStrExt;

        let f = Fixture::new("orx-settings-read");
        let run = f.run("running", local());
        let previous = f
            .store
            .patch_run_monitor_json("exp", &etag(None), |_| {
                Ok(json!({"plan": [{"phase": "train"}]}).to_string())
            })
            .unwrap()
            .unwrap();
        let token = etag(Some(&previous));
        let [path, _] = signal_paths(f.dir.path(), &run.id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

        let data = f.dir.path().to_owned();
        let writer = std::thread::spawn({
            let token = token.clone();
            move || {
                // Opening the pipe waits until the reader starts deriving the default view.
                let mut pipe = std::fs::OpenOptions::new().write(true).open(path).unwrap();
                let saved = Store::open_at(data)
                    .unwrap()
                    .patch_run_monitor_json("exp", &token, |_| {
                        Ok(json!({
                            "view": "<view />",
                            "plan": [{"phase": "eval"}],
                            "patterns": [{"regex": "^eval (?P<accuracy>\\d+)$"}]
                        })
                        .to_string())
                    })
                    .unwrap()
                    .unwrap();
                let mut snapshot = Snapshot::new(&run.id, "1-1", "g1");
                snapshot
                    .feed(b"::orx:: {\"m\":{\"loss\":2}}\n", false, T0, true)
                    .unwrap();
                pipe.write_all(&serde_json::to_vec(&snapshot).unwrap())
                    .unwrap();
                saved
            }
        });
        let first = described(&f.store, "exp");
        let saved = writer.join().unwrap();
        let first = first.unwrap();
        assert_eq!(first["etag"], token);
        assert_eq!(first["isDefault"], true);
        assert_eq!(first["plan"], json!([{"phase": "train"}]));
        assert_eq!(first["patterns"], json!([]));
        let second = described(&f.store, "exp").unwrap();
        assert_eq!(second["etag"], etag(Some(&saved)));
        assert_eq!(second["markup"], "<view />");
        assert_eq!(second["plan"], json!([{"phase": "eval"}]));
        assert_eq!(second["patterns"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_view_can_be_saved_with_a_pattern_for_an_unreported_table() {
        use crate::engine::view::{etag, set, Source};

        let f = Fixture::new("orx-view-with-new-pattern");
        f.run("running", local());
        f.collect(&folded(&[(
            T0,
            r#"::orx:: {"row":"training","m":{"loss":2}}"#.into(),
        )]));
        let pattern = json!([{"row": "validation", "regex": "^test (?P<accuracy>\\d+)$"}]);
        let markup = r#"<view><table name="validation" columns="accuracy" /></view>"#;
        let source = Source {
            file: Some("view.xml"),
            patterns: Some("pattern.json"),
            config: None,
        };
        assert!(set(
            &f.store,
            "exp",
            &etag(None),
            Some(markup),
            None,
            None,
            &source
        )
        .is_err());
        let saved = set(
            &f.store,
            "exp",
            &etag(None),
            Some(markup),
            Some(&pattern),
            None,
            &source,
        )
        .unwrap();
        assert_eq!(saved["pending"], json!(["validation", "accuracy"]));
        assert_eq!(saved["patterns"], pattern);
    }

    #[test]
    fn checking_bars_waits_for_an_outcome_column_to_be_reported() {
        use crate::engine::view::{check, Source};

        let f = Fixture::new("orx-bars-without-outcomes");
        let run = f.run("running", local());
        f.collect(&folded(&[(
            T0,
            r#"::orx:: {"row":"items","m":{"category":"validation"}}"#.into(),
        )]));
        let report = check(
            &f.store,
            &run.id,
            Some(r#"<view><bars from="items" by="category" /></view>"#),
            None,
            &Source {
                file: Some("view.xml"),
                patterns: None,
                config: None,
            },
        )
        .unwrap();
        assert_eq!(report["pending"], json!(["outcome"]));
    }

    #[test]
    fn saving_a_plan_refuses_each_mistake_in_its_own_words() {
        let f = Fixture::new("orx-monitor-plan-save");
        let invalid = |answer: Result<Value, crate::engine::view::Refusal>| match answer {
            Err(crate::engine::view::Refusal::Invalid(why)) => why,
            other => panic!("{other:?}"),
        };
        let raw = |config: Value| {
            let source = crate::engine::view::Source {
                file: None,
                patterns: None,
                config: Some("config.json"),
            };
            crate::engine::view::set(
                &f.store,
                "exp",
                &crate::engine::view::etag(None),
                None,
                None,
                Some(&config),
                &source,
            )
        };
        let refusal = |answer, facts: &[&str]| {
            let error = invalid(answer);
            mentions(
                &error,
                &["config.json", "orx exp view set exp --config config.json"],
            );
            mentions(&error, facts);
        };
        refusal(
            raw(json!({"plan": [], "notes": []})),
            &["notes", "plan", "rules", "brief", "watch"],
        );
        refusal(
            raw(json!({})),
            &["JSON object", "plan", "rules", "brief", "watch"],
        );
        refusal(
            raw(
                json!({"rules": [{"name": "nan", "text": "Stop at NaN.", "does": "halt", "when": {"kind": "nonFinite", "signal": "loss"}}]}),
            ),
            &["rules[0]", "nan", "does", "stop", "ask"],
        );
        for (config, facts) in [
            (
                json!({"brief": {"goal": "x"}}),
                &[
                    "brief",
                    "goal",
                    "question",
                    "healthy",
                    "noise",
                    "worth_a_flag",
                    "from_parent",
                    "decided",
                ][..],
            ),
            (
                json!({"brief": {"question": 3}}),
                &["brief", "question", "text"],
            ),
            (
                json!({"brief": {"question": "x".repeat(3300)}}),
                &["3300 bytes", "3200", "800 tokens"],
            ),
            (
                json!({"watch": {"minGapS": 5}}),
                &["minGapS", "whole number", "seconds", "30", "3600"],
            ),
            (
                json!({"watch": {"everyS": 60}}),
                &["everyS", "minGapS", "maxGapS"],
            ),
            (
                json!({"watch": {"minGapS": 600, "maxGapS": 300}}),
                &["minGapS", "longer", "maxGapS"],
            ),
        ] {
            refusal(raw(config), facts);
        }
        let save = |config: Value| {
            let source = crate::engine::view::Source {
                file: None,
                patterns: None,
                config: Some("config.json"),
            };
            let current = f.store.run_monitor_json("exp").unwrap();
            let etag = crate::engine::view::etag(current.as_deref());
            crate::engine::view::set(&f.store, "exp", &etag, None, None, Some(&config), &source)
        };
        let briefed = save(
            json!({"brief": {"question": "Does depth 8 beat depth 4?"}, "watch": {"maxGapS": 600}}),
        )
        .unwrap();
        assert_eq!(briefed["brief"]["question"], "Does depth 8 beat depth 4?");
        assert_eq!(briefed["watch"], json!({"maxGapS": 600}));
        let cleared = save(json!({"brief": {}})).unwrap();
        assert!(
            cleared.get("brief").is_none(),
            "an empty brief clears it, and keeps the gaps"
        );
        assert_eq!(cleared["watch"], json!({"maxGapS": 600}));
        refusal(
            plan(&f, "exp", json!([{"phase": "train"}, {"phase": "train"}])),
            &["stages 1 and 2", "train", "another stage", "log"],
        );
        let saved = plan(
            &f,
            "exp",
            json!([{"phase": "train", "of": 10}, {"phase": "eval"}, {"phase": "train"}]),
        )
        .unwrap();
        assert_eq!(
            saved["plan"],
            json!([{"phase": "train", "of": 10}, {"phase": "eval"}, {"phase": "train"}])
        );
        let cleared = plan(&f, "exp", json!([])).unwrap();
        assert_eq!(cleared["plan"], json!([]));
        assert!(!f
            .store
            .run_monitor_json("exp")
            .unwrap()
            .unwrap()
            .contains("plan"));
    }

    /// Rules saved on an experiment reach its children, and each run checks the ones it was
    /// launched with whatever the experiment says later.
    #[test]
    fn a_run_keeps_the_rules_it_was_launched_with_and_status_lists_them() {
        let f = Fixture::new("orx-monitor-rules-frozen");
        let source = crate::engine::view::Source {
            file: None,
            patterns: None,
            config: Some("config.json"),
        };
        let configure = |experiment: &str, config: Value| {
            let raw = f.store.run_monitor_json(experiment).unwrap();
            crate::engine::view::set(
                &f.store,
                experiment,
                &crate::engine::view::etag(raw.as_deref()),
                None,
                None,
                Some(&config),
                &source,
            )
            .unwrap()
        };
        let nan = json!({"name": "nan-loss", "text": "Stop when the loss is NaN on 3 reports in a row.",
            "does": "stop", "when": {"kind": "nonFinite", "signal": "loss", "reports": 3}});
        let plateau = json!({"name": "plateau", "text": "Ask when val loss stops improving.", "does": "ask",
            "when": {"kind": "noImprovement", "signal": "val_loss", "min": 0.01, "reports": 4, "better": "lower"}});
        let spend = json!({"name": "spend", "text": "Stop when the run reports $5 spent.", "does": "stop",
            "when": {"kind": "costAbove", "usd": 5.0}});
        let saved = configure("exp", json!({"rules": [nan, plateau, spend]}));
        assert_eq!(saved["rules"], json!([nan, plateau, spend]));
        assert_eq!(saved["plan"], json!([]));
        f.child("child", "exp");
        let launched = |run: &str, experiment: &str| {
            let settings = f.store.run_monitor_json(experiment).unwrap();
            let config =
                crate::engine::rules::freeze(experiment, settings.as_deref(), false).unwrap();
            f.run_in(run, experiment, "running", local());
            f.store.set_run_monitor_config(run, config.as_deref());
        };
        launched("r1", "child");
        configure("child", json!({"rules": []}));
        launched("r2", "child");

        let (_, monitor) =
            super::monitor(&f.store, &f.store.get_run("r1").unwrap().unwrap()).unwrap();
        assert_eq!(
            monitor["rules"],
            json!([
                {"name": "nan-loss", "text": "Stop when the loss is NaN on 3 reports in a row.", "does": "stop"},
                {"name": "plateau", "text": "Ask when val loss stops improving.", "does": "ask"},
                {"name": "spend", "text": "Stop when the run reports $5 spent.", "does": "stop"},
            ])
        );
        let rules: Vec<_> = status_line(&monitor, "rules").split('·').collect();
        assert_eq!(rules.len(), 3);
        mentions(rules[0], &["nan-loss", "stops"]);
        mentions(rules[1], &["plateau", "wakes", "agent"]);
        mentions(rules[2], &["spend", "stops", "$5.00"]);
        let (_, later) =
            super::monitor(&f.store, &f.store.get_run("r2").unwrap().unwrap()).unwrap();
        assert_eq!(later["rules"], json!([]));
        assert_eq!(status_line(&later, "rules"), "");

        // A run's rules written by a newer orx are said to be unreadable, not dropped.
        f.run_in("r3", "child", "running", local());
        f.store.set_run_monitor_config(
            "r3",
            Some(&json!({"rules": [{"name": "r", "text": "t", "does": "stop", "when": {"kind": "lossAbove", "value": 1}}]}).to_string()),
        );
        let (_, newer) =
            super::monitor(&f.store, &f.store.get_run("r3").unwrap().unwrap()).unwrap();
        mentions(
            status_line(&newer, "rules"),
            &["r3", "rules[0]", "(r)", "unknown", "lossAbove"],
        );
    }

    /// A running run in `experiment` launched with `rules`.
    fn ruled(f: &Fixture, id: &str, experiment: &str, backend: Value, rules: Value) -> StoredRun {
        let run = f.run_in(id, experiment, "running", backend);
        let config = json!({"rules": rules}).to_string();
        crate::engine::rules::frozen(id, Some(&config)).unwrap();
        f.store.set_run_monitor_config(id, Some(&config));
        run
    }

    /// The run's checkpoint once it has reported `values` of `signal`, one report a second from
    /// `T0`, steps by tens, out of `of` steps.
    fn reported(f: &Fixture, id: &str, signal: &str, values: &[&str], of: i64) {
        let lines: Vec<(i64, String)> = values
            .iter()
            .enumerate()
            .map(|(i, value)| {
                let i = i as i64;
                let step = i * 10;
                let line = format!(
                    "::orx:: {{\"step\":{step},\"of\":{of},\"t\":{i},\"m\":{{\"{signal}\":{value}}}}}"
                );
                (T0 + i * 1000, line)
            })
            .collect();
        f.collect(&Snapshot {
            run_id: id.into(),
            ..folded(&lines)
        });
        f.confirmed(id, lines.last().unwrap().0);
    }

    /// One pass of the rules over the run at `now`, as `orx up` makes each second.
    fn rules_at(f: &Fixture, id: &str, now: i64) {
        let run = f.store.get_run(id).unwrap().unwrap();
        let (projection, _) = publish(&f.store, &run, now, None).unwrap();
        check_rules(&f.store, None, &run, &projection, now).unwrap();
    }

    fn stopped_by(f: &Fixture, id: &str) -> Option<crate::store::StopContext> {
        f.store.run_stop(id).unwrap()
    }

    #[test]
    fn a_new_stage_or_generation_starts_a_new_rule_streak() {
        for reset in ["stage", "generation"] {
            let f = Fixture::new("orx-rule-reset");
            ruled(
                &f,
                "r1",
                "exp",
                local(),
                json!([{"name": "nan", "text": "t", "does": "stop",
                    "when": {"kind": "nonFinite", "signal": "loss", "reports": 2}}]),
            );
            let bad = |step| format!("::orx:: {{\"step\":{step},\"m\":{{\"loss\":\"NaN\"}}}}\n");
            let mut snapshot = Snapshot::new("r1", "1-1", "g1");
            snapshot.feed(bad(100).as_bytes(), false, T0, true).unwrap();
            f.collect(&snapshot);
            f.confirmed("r1", T0);
            rules_at(&f, "r1", T0 + 1_000);

            if reset == "stage" {
                snapshot
                    .feed(b"::orx:: {\"phase\":\"eval\"}\n", false, T0 + 2_000, true)
                    .unwrap();
            } else {
                snapshot = Snapshot::new("r1", "2-2", "g2");
                snapshot
                    .feed(b"new log\n", false, T0 + 2_000, true)
                    .unwrap();
            }
            snapshot
                .feed(bad(10).as_bytes(), false, T0 + 2_000, true)
                .unwrap();
            f.collect(&snapshot);
            f.confirmed("r1", T0 + 2_000);
            rules_at(&f, "r1", T0 + 3_000);
            assert_eq!(stopped_by(&f, "r1"), None, "one bad report after {reset}");

            snapshot
                .feed(bad(20).as_bytes(), false, T0 + 4_000, true)
                .unwrap();
            f.collect(&snapshot);
            f.confirmed("r1", T0 + 4_000);
            rules_at(&f, "r1", T0 + 5_000);
            assert!(
                stopped_by(&f, "r1").is_some(),
                "two bad reports after {reset}"
            );
        }
    }

    #[test]
    fn same_step_reports_count_and_recovery_breaks_the_streak() {
        let f = Fixture::new("orx-rule-same-step");
        ruled(
            &f,
            "r1",
            "exp",
            local(),
            json!([{"name": "nan", "text": "t", "does": "stop",
                "when": {"kind": "nonFinite", "signal": "loss", "reports": 2}}]),
        );
        let mut snapshot = Snapshot::new("r1", "1-1", "g1");
        for (i, loss) in ["\"NaN\"", "1.0", "\"NaN\"", "\"NaN\""]
            .into_iter()
            .enumerate()
        {
            let at = T0 + i as i64 * 2_000;
            let report = format!("::orx:: {{\"step\":5,\"t\":{i},\"m\":{{\"loss\":{loss}}}}}\n");
            snapshot.feed(report.as_bytes(), false, at, true).unwrap();
            f.collect(&snapshot);
            f.confirmed("r1", at);
            rules_at(&f, "r1", at + 1_000);
            assert_eq!(stopped_by(&f, "r1").is_some(), i == 3, "report {i}");
        }
    }

    #[test]
    fn a_rule_counts_the_contiguous_tail_after_chart_compaction() {
        let f = Fixture::new("orx-rule-compaction");
        ruled(
            &f,
            "r1",
            "exp",
            local(),
            json!([{"name": "nan", "text": "t", "does": "stop",
                "when": {"kind": "nonFinite", "signal": "loss", "reports": 256}}]),
        );
        reported(&f, "r1", "loss", &vec!["NaN"; 255], 3000);
        rules_at(&f, "r1", T0 + 255_000);
        assert_eq!(stopped_by(&f, "r1"), None);

        let mut losses = vec!["NaN"; 2049];
        losses.push("\"warmup\"");
        losses.extend(["NaN"; 255]);
        reported(&f, "r1", "loss", &losses, 3000);
        rules_at(&f, "r1", T0 + losses.len() as i64 * 1_000);
        assert_eq!(stopped_by(&f, "r1"), None, "warmup breaks the streak");

        losses.push("NaN");
        reported(&f, "r1", "loss", &losses, 3000);
        rules_at(&f, "r1", T0 + losses.len() as i64 * 1_000);
        assert!(stopped_by(&f, "r1").is_some());
    }

    #[test]
    fn a_rule_needs_a_current_collection_of_the_checkpoint() {
        let f = Fixture::new("orx-rule-confirmed");
        let run = ruled(
            &f,
            "r1",
            "exp",
            local(),
            json!([{"name": "nan", "text": "t", "does": "stop",
                "when": {"kind": "nonFinite", "signal": "loss"}}]),
        );
        let mut snapshot = Snapshot::new("r1", "1-1", "g1");
        snapshot
            .feed(
                b"::orx:: {\"step\":1,\"m\":{\"loss\":\"NaN\"}}\n",
                false,
                T0,
                true,
            )
            .unwrap();
        f.collect(&snapshot);
        rules_at(&f, "r1", T0 + 1_000);
        assert_eq!(
            stopped_by(&f, "r1"),
            None,
            "published without a collection status"
        );

        f.confirmed("r1", T0 + 2_000);
        let (old, _) = publish(&f.store, &run, T0 + 2_000, None).unwrap();
        check_rules(&f.store, None, &run, &old, T0 + 110_000).unwrap();
        assert_eq!(stopped_by(&f, "r1"), None, "cached across freshness expiry");

        snapshot.coverage.add_reason("rebuilding");
        f.collect(&snapshot);
        check_rules(&f.store, None, &run, &old, T0 + 3_000).unwrap();
        assert_eq!(stopped_by(&f, "r1"), None, "rebuild began after evaluation");

        snapshot.remove_reason("rebuilding");
        snapshot
            .feed(b"another line\n", false, T0 + 5_000, true)
            .unwrap();
        f.collect(&snapshot);
        rules_at(&f, "r1", T0 + 6_000);
        assert_eq!(
            stopped_by(&f, "r1"),
            None,
            "a new revision and the old heartbeat"
        );

        f.confirmed("r1", T0 + 112_000);
        rules_at(&f, "r1", T0 + 113_000);
        assert!(
            stopped_by(&f, "r1").is_some(),
            "confirmed by a current scan"
        );
    }

    #[test]
    fn a_rule_stops_a_run_at_the_report_that_meets_it_and_never_twice() {
        let f = Fixture::new("orx-rules-stop");
        ruled(
            &f,
            "r1",
            "exp",
            local(),
            json!([
                {"name": "nan-loss", "text": "Stop when the loss is NaN on 3 reports in a row.",
                    "does": "stop", "when": {"kind": "nonFinite", "signal": "loss", "reports": 3}},
                {"name": "also-stop", "text": "Check the same reports.",
                    "does": "stop", "when": {"kind": "nonFinite", "signal": "loss", "reports": 3}},
                {"name": "also-ask", "text": "Review the same reports.",
                    "does": "ask", "when": {"kind": "nonFinite", "signal": "loss", "reports": 3}}
            ]),
        );
        let mut losses = vec!["2.5", "2.4", "NaN", "2.3", "NaN", "NaN"];
        reported(&f, "r1", "loss", &losses, 600);
        rules_at(&f, "r1", T0 + 6_000);
        assert_eq!(stopped_by(&f, "r1"), None, "two in a row is not three");
        losses.push("NaN");
        reported(&f, "r1", "loss", &losses, 600);
        rules_at(&f, "r1", T0 + 7_000);
        let stop = stopped_by(&f, "r1").expect("the third NaN in a row stops it");
        assert_eq!(stop.by, crate::store::StopBy::Rule);
        let config = f.store.run_monitor_config("r1").unwrap().unwrap();
        let frozen: Value = serde_json::from_str(&config).unwrap();
        assert_eq!(stop.rule.as_ref(), Some(&frozen["rules"][0]));
        let reason = stop.reason.as_deref().unwrap();
        mentions(reason, &["loss", "NaN", "3 reports", "step 40 to 60"]);
        let run = f.store.get_run("r1").unwrap().unwrap();
        mentions(
            run.cancel_reason.as_deref().unwrap(),
            &["Stopped", "rule", "nan-loss", reason, "No model"],
        );
        assert!(super::super::flags::read(&f.store, &run)
            .unwrap()
            .is_empty());

        // A count this orx cannot read is never taken for none.
        ruled(
            &f,
            "unread",
            "exp",
            local(),
            json!([{"name": "nan-loss", "text": "t", "does": "stop", "when": {"kind": "nonFinite", "signal": "loss"}}]),
        );
        f.store.put_run_monitor_state(
            "unread",
            &json!({"ruleState": {"basis": ["g1", 0], "counts": {"nan-loss": {"held": "three"}}}}),
        );
        reported(&f, "unread", "loss", &["NaN"], 600);
        rules_at(&f, "unread", T0 + 2_000);
        assert_eq!(stopped_by(&f, "unread"), None);

        // A restart forgets nothing it counted: the same reports never stop it again.
        f.restart();
        f.store.conn_execute_for_tests(
            "UPDATE runs SET cancel_accepted_at = NULL, stop_context_json = NULL WHERE id = 'r1'",
        );
        rules_at(&f, "r1", T0 + 8_000);
        assert_eq!(stopped_by(&f, "r1"), None);
    }

    #[test]
    fn a_rule_lands_when_another_writer_commits_during_its_check() {
        use crate::store::{StopBy, StopContext};

        let f = Fixture::new("orx-rule-concurrent-writer");
        let run = f.run("running", local());
        let writer = Store::open_at(f.dir.path().to_path_buf()).unwrap();
        let counts = json!({"basis": ["g1", 0]});

        let accepted = f
            .store
            .record_rules(
                &run,
                &counts,
                &[],
                Some(("Rule stopped it.", &StopContext::by(StopBy::Rule))),
                |settings| {
                    assert_eq!(settings, None);
                    writer.subscribe_run("r1", "observer", T0)?;
                    Ok(true)
                },
            )
            .unwrap();

        assert!(accepted.is_some());
        assert_eq!(stopped_by(&f, "r1").unwrap().by, StopBy::Rule);
        assert_eq!(
            f.store.run_monitor_state("r1").unwrap().unwrap()["ruleState"],
            counts
        );
    }

    #[test]
    fn a_user_cancel_accepted_before_recording_wins_over_a_rule() {
        use crate::store::{StopBy, StopContext};

        let f = Fixture::new("orx-rule-cancel");
        let run = f.run("running", local());
        f.store
            .accept_run_stop(
                &run.id,
                Some("User stopped it."),
                Some(&StopContext::by(StopBy::User)),
            )
            .unwrap()
            .unwrap();

        let accepted = f
            .store
            .record_rules(
                &run,
                &json!({}),
                &[],
                Some(("Rule stopped it.", &StopContext::by(StopBy::Rule))),
                |_| Ok(true),
            )
            .unwrap();
        assert_eq!(accepted, None);
        assert_eq!(stopped_by(&f, "r1").unwrap().by, StopBy::User);
        assert_eq!(f.store.run_monitor_state("r1").unwrap(), None);
    }

    #[test]
    fn a_value_that_recovered_stops_nothing() {
        let f = Fixture::new("orx-rules-recovered");
        ruled(
            &f,
            "r1",
            "exp",
            local(),
            json!([{"name": "nan-loss", "text": "t", "does": "stop", "when": {"kind": "nonFinite", "signal": "loss", "reports": 3}},
                   {"name": "low-eval", "text": "t", "does": "stop",
                    "when": {"kind": "compare", "signal": "loss", "op": "gt", "value": 9, "reports": 2}}]),
        );
        // Read at once, as after a sleep: the NaNs and the spike are history the run left behind.
        reported(
            &f,
            "r1",
            "loss",
            &["2.5", "NaN", "NaN", "NaN", "12", "13", "2.1"],
            600,
        );
        rules_at(&f, "r1", T0 + 7_000);
        assert_eq!(stopped_by(&f, "r1"), None);
        assert_eq!(
            f.store.run_monitor_state("r1").unwrap().unwrap()["ruleState"]["counts"]["nan-loss"]
                ["after"],
            7,
            "the reports are counted"
        );
    }

    #[test]
    fn a_finished_runs_review_is_reprojected_without_a_turn_event() {
        let f = Fixture::new("orx-finished-flag-review");
        let run = f.run("running", local());
        let chat = "reviewer";
        f.store.put_chat_session(chat, "claude-code");
        f.store.subscribe_run(&run.id, chat, T0).unwrap();
        let flag = super::super::flags::new_flag(
            &f.store,
            &run,
            json!({"id": "f1", "template": "The loss rose.", "source": "look"}),
            true,
        )
        .unwrap();
        f.store.raise_flags(&[flag], T0).unwrap();
        let batch = f.store.run_notice_batch_at(chat, T0).unwrap();
        let turn = &batch[0].client_turn_id;
        f.store.put_chat_turn(chat, turn, "running", T0);
        f.store.set_run_status(&run.id, "done");
        watch(&f.store, Some(run.id.clone()), BTreeSet::new(), None).unwrap();
        let before = f.store.run_monitor_state(&run.id).unwrap().unwrap();
        assert_eq!(before["published"]["summary"]["flag"]["state"], "waking");

        f.store
            .update_chat_turn_progress(turn, "running", "accepted", 1, None)
            .unwrap();
        let watched = watch(&f.store, None, BTreeSet::new(), None).unwrap();
        let after = f.store.run_monitor_state(&run.id).unwrap().unwrap();
        assert_eq!(
            after["published"]["summary"]["flag"]["state"],
            "agent_looking"
        );

        f.store.complete_chat_turn(turn).unwrap();
        watch(&f.store, None, watched, None).unwrap();
        let after = f.store.run_monitor_state(&run.id).unwrap().unwrap();
        assert_eq!(after["published"]["summary"]["flag"]["state"], "waking");
    }

    #[test]
    fn a_finished_runs_waiting_summary_updates_after_an_ask_finishes() {
        let f = Fixture::new("orx-ended-ask-handback");
        let run = f.run("done", local());
        let chat = "reviewer";
        f.store.put_chat_session(chat, "claude-code");
        let body = json!({"id": "f1", "source": "look", "text": "Loss rose."});
        let flag = super::super::flags::new_flag(&f.store, &run, body.clone(), true).unwrap();
        f.store.raise_flags(&[flag], T0).unwrap();
        f.store.subscribe_run(&run.id, chat, T0).unwrap();
        let before = advance(&f.store, &run, T0).unwrap();
        assert_eq!(before.summary()["flag"]["state"], "waiting");

        let ask = crate::store::FlagChoice {
            run_id: run.id.clone(),
            body: json!({"flagId": "f1", "c": "ask", "by": "user", "requestId": "first"}),
            wakes: vec![(chat.into(), super::super::flags::wake(&run, &body, true))],
        };
        f.store.record_flag_choices(&[ask], None, T0 + 1).unwrap();
        let batch = f.store.run_notice_batch_at(chat, T0 + 1).unwrap();
        let turn = &batch[0].client_turn_id;
        f.store.put_chat_turn(chat, turn, "running", T0 + 1);
        assert!(f.store.complete_chat_turn(turn).unwrap());

        let after = current(&f.store, &run).unwrap().unwrap();
        let ask = f.store.run_flag_events(&run.id).unwrap().pop().unwrap();
        assert_eq!(after.summary()["flag"]["state"], "waiting");
        assert_eq!(after.summary()["flag"]["handback"], ask.seq);
    }

    #[test]
    fn an_ask_flags_the_run_wakes_its_chat_once_and_the_run_keeps_going() {
        let f = Fixture::new("orx-rules-ask");
        ruled(
            &f,
            "r1",
            "exp",
            local(),
            json!([{"name": "eval-zero", "text": "Ask when the eval score falls below 0.1.", "does": "ask",
                "when": {"kind": "compare", "signal": "eval_score", "op": "lt", "value": 0.1}}]),
        );
        f.store.put_chat_session("chat-1", "claude-code");
        f.store.subscribe_run("r1", "chat-1", T0).unwrap();
        reported(&f, "r1", "eval_score", &["0.5", "0.05"], 600);
        rules_at(&f, "r1", T0 + 2_000);
        reported(&f, "r1", "eval_score", &["0.5", "0.05", "0.0"], 600);
        rules_at(&f, "r1", T0 + 3_000);
        assert_eq!(stopped_by(&f, "r1"), None);
        assert!(f
            .store
            .engine_run_state("r1")
            .unwrap()
            .unwrap()
            .cancel_accepted_at
            .is_none());
        assert_eq!(f.store.pending_run_notice_sessions().unwrap(), ["chat-1"]);
        let run = f.store.get_run("r1").unwrap().unwrap();
        let flags = super::super::flags::read(&f.store, &run).unwrap();
        assert_eq!(flags.len(), 1);
        let (flag, id) = (&flags[0], flags[0]["id"].as_str().unwrap());
        assert_eq!(
            (flag["source"].as_str(), flag["rule"].as_str()),
            (Some("rule"), Some("eval-zero"))
        );
        let summary = advance(&f.store, &run, T0 + 3_000)
            .unwrap()
            .summary()
            .clone();
        assert_eq!(summary["tone"], "flag");
        assert_eq!(summary["flag"]["id"], id);
        assert_eq!(summary["flag"]["state"], "raised");

        let notices = f.store.run_notice_batch("chat-1").unwrap();
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert_eq!(notices[0].run_id, "r1");
        let body = &notices[0].body;
        let at = clock_text(flag["at"].as_i64().unwrap());
        mentions(
            body,
            &[
                "[orx]",
                "r1",
                "exp",
                "flagged",
                "eval-zero",
                "Ask when the eval score falls below 0.1.",
                &at,
                "eval_score",
                "0.05",
                "below 0.1",
                "step 10",
                "no model",
                "confirm",
                "orx exp status exp --run r1",
                "orx logs r1",
            ],
        );
        for action in ["stop", "keep", "ask"] {
            let command = format!("orx exp flag r1 --flag {id} --{action} --reason");
            mentions(body, &[&command]);
        }
    }

    #[test]
    fn a_run_worse_than_its_parent_at_the_same_step_is_stopped_where_the_parent_reported() {
        let f = Fixture::new("orx-rules-parent");
        loss_run(&f, "parent", "exp", "done", 60, 1.0);
        rules_at(&f, "parent", T0 + 61_000);
        f.child("child", "exp");
        ruled(
            &f,
            "r1",
            "child",
            local(),
            json!([{"name": "diverging", "text": "Stop when the loss is 1 worse than the parent's.",
                "does": "stop", "when": {"kind": "worseThanParent", "signal": "loss", "by": 1, "better": "lower", "reports": 3}}]),
        );
        // The parent's loss at step 10i is 3/(i+1): 3, 1.5, 1, 0.75, 0.6.
        reported(&f, "r1", "loss", &["3", "2.4", "2.0"], 400);
        rules_at(&f, "r1", T0 + 3_000);
        assert_eq!(
            stopped_by(&f, "r1"),
            None,
            "0.9 and 1 worse is within the rule"
        );
        reported(
            &f,
            "r1",
            "loss",
            &["3", "2.4", "2.0", "1.9", "1.8", "1.7"],
            400,
        );
        rules_at(&f, "r1", T0 + 6_000);
        let stop = stopped_by(&f, "r1").expect("worse by more than 1 on three reports");
        mentions(
            stop.reason.as_deref().unwrap(),
            &[
                "loss",
                "1.2",
                "worse",
                "parent",
                "same step",
                "1.7",
                "0.5",
                "3 reports",
                "step 30 to 50",
            ],
        );
    }

    #[test]
    fn a_parent_whose_later_stage_reuses_the_signal_stops_no_run_in_an_earlier_stage() {
        let f = Fixture::new("orx-rules-parent-stage");
        let staged = |stages: &[(&str, f64)]| -> Vec<(i64, String)> {
            let mut lines = Vec::new();
            for (stage, loss) in stages {
                lines.push(format!(
                    "::orx:: {{\"phase\":\"{stage}\",\"step\":0,\"of\":100}}"
                ));
                for step in (10..=60).step_by(10) {
                    lines.push(format!(
                        "::orx:: {{\"step\":{step},\"m\":{{\"loss\":{loss}}}}}"
                    ));
                }
            }
            lines
                .into_iter()
                .enumerate()
                .map(|(i, line)| (T0 + 1000 * i as i64, line))
                .collect()
        };
        f.run_in("parent", "exp", "done", local());
        f.collect(&Snapshot {
            run_id: "parent".into(),
            ..folded(&staged(&[("pretrain", 2.0), ("finetune", 1.2)]))
        });
        rules_at(&f, "parent", T0 + 61_000);
        f.child("child", "exp");
        ruled(
            &f,
            "r1",
            "child",
            local(),
            json!([{"name": "diverging", "text": "Stop when the loss is 1 worse than the parent's.",
                "does": "stop", "when": {"kind": "worseThanParent", "signal": "loss", "by": 1, "better": "lower", "reports": 3}}]),
        );
        f.collect(&Snapshot {
            run_id: "r1".into(),
            ..folded(&staged(&[("pretrain", 6.0)]))
        });
        f.confirmed("r1", T0 + 7_000);
        rules_at(&f, "r1", T0 + 10_000);
        assert_eq!(
            stopped_by(&f, "r1"),
            None,
            "pretrain's 6.0 is never held against finetune's 1.2"
        );
    }

    #[test]
    fn a_parent_that_counted_seconds_stops_no_run_that_counts_steps() {
        let f = Fixture::new("orx-rules-parent-units");
        let timed = |lines: Vec<String>| -> Vec<(i64, String)> {
            lines
                .into_iter()
                .enumerate()
                .map(|(i, line)| (T0 + 1000 * i as i64, line))
                .collect()
        };
        f.run_in("parent", "exp", "done", local());
        f.collect(&Snapshot {
            run_id: "parent".into(),
            ..folded(&timed(
                (0..=70)
                    .map(|_| "::orx:: {\"m\":{\"loss\":0.5}}".to_string())
                    .collect(),
            ))
        });
        rules_at(&f, "parent", T0 + 71_000);
        f.child("child", "exp");
        ruled(
            &f,
            "r1",
            "child",
            local(),
            json!([{"name": "diverging", "text": "Stop when the loss is 1 worse than the parent's.",
                "does": "stop", "when": {"kind": "worseThanParent", "signal": "loss", "by": 1, "better": "lower", "reports": 3}}]),
        );
        let mut child = vec!["::orx:: {\"phase\":\"train\",\"step\":0,\"of\":100}".to_string()];
        child.extend(
            (10..=60)
                .step_by(10)
                .map(|step| format!("::orx:: {{\"step\":{step},\"m\":{{\"loss\":6.0}}}}")),
        );
        f.collect(&Snapshot {
            run_id: "r1".into(),
            ..folded(&timed(child))
        });
        f.confirmed("r1", T0 + 7_000);
        rules_at(&f, "r1", T0 + 10_000);
        assert_eq!(
            stopped_by(&f, "r1"),
            None,
            "the child's step 10 is never held against the parent's tenth second"
        );
    }

    #[test]
    fn a_run_that_stopped_improving_is_stopped_once_its_window_is_full() {
        let f = Fixture::new("orx-rules-plateau");
        ruled(
            &f,
            "r1",
            "exp",
            local(),
            json!([{"name": "plateau", "text": "t", "does": "stop",
                "when": {"kind": "noImprovement", "signal": "val_loss", "min": 0.05, "reports": 3, "better": "lower"}}]),
        );
        let mut losses = vec!["3.0", "2.0", "1.5", "1.49", "1.48"];
        reported(&f, "r1", "val_loss", &losses, 600);
        rules_at(&f, "r1", T0 + 5_000);
        assert_eq!(stopped_by(&f, "r1"), None, "1.5 beat 2.0 within the window");
        losses.push("1.47");
        reported(&f, "r1", "val_loss", &losses, 600);
        rules_at(&f, "r1", T0 + 6_000);
        let stop = stopped_by(&f, "r1").unwrap();
        mentions(
            stop.reason.as_deref().unwrap(),
            &[
                "val_loss",
                "improved",
                "0.03",
                "3 reports",
                "step 30 to 50",
                "best",
                "1.5",
                "needs",
                "0.05",
            ],
        );
    }

    #[test]
    fn a_planned_run_reporting_metrics_without_steps_can_trip_no_progress() {
        let f = Fixture::new("orx-rules-no-step");
        let run = ruled(
            &f,
            "r1",
            "exp",
            local(),
            json!([{"name": "stuck", "text": "Stop if no step is reported for ten minutes.",
                "does": "stop", "when": {"kind": "noProgress", "seconds": 600}}]),
        );
        plan(&f, "exp", json!([{"phase":"train","of":100}])).unwrap();
        f.collect(&folded(&[
            (T0, r#"::orx:: {"phase":"train","m":{"loss":2}}"#.into()),
            (T0 + 590_000, r#"::orx:: {"m":{"loss":1}}"#.into()),
        ]));

        let before = T0 + 599_000;
        f.confirmed("r1", before);
        let (monitor, _) = monitor_at(&f, &run, before);
        assert!(monitor["progress"].is_object());
        assert!(monitor["progress"]["lastAt"].is_null());
        assert_eq!(monitor["latest"]["loss"], 1.0);
        rules_at(&f, "r1", before);
        assert!(stopped_by(&f, "r1").is_none());

        let deadline = T0 + 600_000;
        f.confirmed("r1", deadline);
        rules_at(&f, "r1", deadline);
        let stop = stopped_by(&f, "r1").expect("no step since the run started");
        assert_eq!(stop.rule.unwrap()["name"], "stuck");
    }

    #[test]
    fn a_silent_run_spend_and_a_projection_each_stop_when_they_should() {
        let f = Fixture::new("orx-rules-time");
        ruled(
            &f,
            "r1",
            "exp",
            local(),
            json!([{"name": "stuck", "text": "t", "does": "stop", "when": {"kind": "noProgress", "seconds": 600}}]),
        );
        reported(&f, "r1", "loss", &["2.0", "1.9"], 600);
        f.confirmed("r1", T0 + 500_000);
        rules_at(&f, "r1", T0 + 500_000);
        assert_eq!(stopped_by(&f, "r1"), None);
        f.confirmed("r1", T0 + 602_000);
        rules_at(&f, "r1", T0 + 602_000);
        let stop = stopped_by(&f, "r1").unwrap();
        mentions(
            stop.reason.as_deref().unwrap(),
            &["no progress", "10m", "step 10"],
        );

        ruled(
            &f,
            "spent",
            "exp",
            local(),
            json!([{"name": "budget", "text": "Stop at $5 spent.", "does": "stop", "when": {"kind": "costAbove", "usd": 5}}]),
        );
        let lines = [(T0, r#"::orx:: {"step":1,"of":100,"spend":5}"#.to_owned())];
        f.collect(&Snapshot {
            run_id: "spent".into(),
            ..folded(&lines)
        });
        f.confirmed("spent", T0);
        rules_at(&f, "spent", T0 + 1_000);
        let stop = stopped_by(&f, "spent").unwrap();
        mentions(
            stop.reason.as_deref().unwrap(),
            &["cost", "reached", "$5.00", "budget"],
        );

        let limited = json!({"kind": "local_job", "timeoutSecs": 120});
        ruled(
            &f,
            "slow",
            "exp",
            limited,
            json!([{"name": "too-slow", "text": "t", "does": "stop", "when": {"kind": "projectedFinishPastLimit"}}]),
        );
        let values = |n: usize| vec!["1.0"; n];
        // Ten steps a second of 6,000 end long after the 2m limit, but a slice or two is no pace.
        reported(&f, "slow", "loss", &values(30), 6000);
        rules_at(&f, "slow", T0 + 30_000);
        reported(&f, "slow", "loss", &values(31), 6000);
        rules_at(&f, "slow", T0 + 31_000);
        assert_eq!(stopped_by(&f, "slow"), None);
        reported(&f, "slow", "loss", &values(80), 6000);
        rules_at(&f, "slow", T0 + 80_000);
        assert_eq!(stopped_by(&f, "slow"), None, "one revision is not two");
        rules_at(&f, "slow", T0 + 81_000);
        assert_eq!(
            stopped_by(&f, "slow"),
            None,
            "the same revision counts once"
        );
        let [path, _] = signal_paths(f.dir.path(), "slow");
        let mut rebuilt = crate::engine::adapters::local::signals::load(&path)
            .unwrap()
            .unwrap();
        rebuilt.generation = "rebuilt".into();
        rebuilt.rev = 1;
        f.collect(&rebuilt);
        f.confirmed("slow", T0 + 82_000);
        rules_at(&f, "slow", T0 + 82_000);
        assert_eq!(
            stopped_by(&f, "slow"),
            None,
            "a new generation needs its own two votes"
        );

        fold(
            &mut rebuilt,
            &[(
                T0 + 83_000,
                r#"::orx:: {"step":800,"of":6000,"t":80,"m":{"loss":1}}"#.into(),
            )],
        );
        f.collect(&rebuilt);
        f.confirmed("slow", T0 + 83_000);
        rules_at(&f, "slow", T0 + 83_000);
        let reason = stopped_by(&f, "slow").unwrap().reason.unwrap();
        mentions(&reason, &["finish", "past", "time limit"]);
    }

    #[test]
    fn projected_spend_stops_only_after_two_revisions_above_the_budget() {
        let f = Fixture::new("orx-rules-projected-spend");
        let run = ruled(
            &f,
            "r1",
            "exp",
            local(),
            json!([{"name": "budget", "text": "Stop at $8 spent.", "does": "stop", "when": {"kind": "costAbove", "usd": 8}}]),
        );
        let mut snapshot = Snapshot::new("r1", "1-1", "g1");
        for (from, through, extra) in [(0, 64, 0.0), (65, 65, 0.0), (66, 66, 0.25), (67, 67, 0.25)]
        {
            let lines: Vec<_> = (from..=through)
                .map(|step| {
                    let spend = step as f64 / 16.0 + extra;
                    (
                        T0 + step * 1000,
                        format!(r#"::orx:: {{"step":{step},"of":128,"t":{step},"spend":{spend}}}"#),
                    )
                })
                .collect();
            fold(&mut snapshot, &lines);
            f.collect(&snapshot);
            let now = T0 + through * 1000;
            f.confirmed("r1", now);
            let (monitor, _) = monitor_at(&f, &run, now);
            let projected = monitor["cost"]["projected"].as_f64().unwrap();
            assert_eq!(projected > 8.0, extra > 0.0);
            assert!(monitor["cost"]["soFar"].as_f64().unwrap() < 8.0);
            rules_at(&f, "r1", now);
            assert_eq!(stopped_by(&f, "r1").is_some(), through == 67);
        }
    }

    #[test]
    fn hosted_spend_is_reported_without_a_machine_total() {
        let f = Fixture::new("orx-monitor-hosted-spend");
        let run = f.run("running", json!({"kind": "hf_job"}));
        f.signals(&folded(&[(
            T0,
            r#"::orx:: {"step":10,"of":100,"spend":1}"#.into(),
        )]));
        f.collected(T0);
        let (monitor, _) = monitor_at(&f, &run, T0 + 1000);
        let cost = &monitor["cost"];
        assert_eq!(cost["spend"], 1.0);
        assert!(cost["soFar"].is_null());
        assert!(cost["projected"].is_null());
    }

    #[test]
    fn rules_act_only_on_a_live_run_seen_live() {
        let f = Fixture::new("orx-rules-live");
        let nan = json!([{"name": "nan", "text": "t", "does": "stop", "when": {"kind": "nonFinite", "signal": "loss"}}]);
        ruled(&f, "r1", "exp", local(), nan.clone());
        reported(&f, "r1", "loss", &["NaN"], 600);
        // Collection stopped two minutes ago: what the run is doing now is unknown.
        f.confirmed("r1", T0);
        rules_at(&f, "r1", T0 + 120_000);
        assert_eq!(stopped_by(&f, "r1"), None);
        assert!(f
            .store
            .run_monitor_state("r1")
            .unwrap()
            .unwrap()
            .get("ruleState")
            .is_none());
        f.confirmed("r1", T0 + 120_000);
        f.store
            .set_run_contact(
                "r1",
                &Contact::Unreachable {
                    since: T0,
                    cause: "offline".into(),
                },
            )
            .unwrap();
        rules_at(&f, "r1", T0 + 121_000);
        assert_eq!(stopped_by(&f, "r1"), None, "lost contact acts on nothing");

        ruled(&f, "done", "exp", local(), nan);
        reported(&f, "done", "loss", &["NaN"], 600);
        f.store.set_run_status("done", "done");
        rules_at(&f, "done", T0 + 2_000);
        assert_eq!(
            stopped_by(&f, "done"),
            None,
            "a run that ended is never stopped"
        );
    }

    /// A stopped run reads Revising while its agent repairs the experiment, and stays watched so
    /// its line hears at once when the turn ends without a new run.
    #[test]
    fn a_revising_run_is_watched_until_its_revision_ends() {
        let f = Fixture::new("orx-monitor-revising");
        let run = f.run("cancelled", local());
        f.signals(&folded(&declared(0..=10, 100)));
        let tone = || current(&f.store, &run).unwrap().unwrap().summary()["tone"].clone();
        advance(&f.store, &run, T0).unwrap();
        assert_eq!(tone(), "stopped");
        f.store.put_chat_turn("chat", "turn", "running", T0);
        f.store.record_revision("r1", "turn").unwrap();
        let watched = watch(&f.store, Some("r1".into()), BTreeSet::new(), None).unwrap();
        assert!(watched.contains("r1"));
        assert_eq!(tone(), "revising");
        let (_, monitor) = super::monitor(&f.store, &run).unwrap();
        assert_eq!(monitor["revising"], true);
        f.restart();
        let reads = CHECKPOINT_READS.with(std::cell::Cell::get);
        assert_eq!(tone(), "revising");
        assert_eq!(CHECKPOINT_READS.with(std::cell::Cell::get), reads);

        f.store
            .conn_execute_for_tests("UPDATE chat_turns SET state = 'completed' WHERE id = 'turn'");
        assert_eq!(tone(), "stopped");
        let watched = watch(&f.store, None, watched, None).unwrap();
        assert!(!watched.contains("r1"));
        assert_eq!(tone(), "stopped");
    }

    /// `orx exp status` says who stopped a run and why, in the lead the page gives it.
    #[test]
    fn status_says_who_stopped_a_run() {
        use crate::store::{StopBy, StopContext};
        let f = Fixture::new("orx-monitor-stop-words");
        let stopped = |id: &str, by: StopBy, reason: Option<&str>, rule: Option<Value>| {
            f.run_named(id, "running", local());
            let stop = StopContext {
                reason: reason.map(str::to_owned),
                rule,
                ..StopContext::by(by)
            };
            f.store
                .accept_run_stop(id, Some("words"), Some(&stop))
                .unwrap()
                .unwrap();
            let live = super::monitor(&f.store, &f.store.get_run(id).unwrap().unwrap())
                .unwrap()
                .1;
            let asked = status_line(&live, "stop").to_owned();
            f.store.set_run_status(id, "cancelled");
            let ended = super::monitor(&f.store, &f.store.get_run(id).unwrap().unwrap())
                .unwrap()
                .1;
            (asked, status_line(&ended, "stop").to_owned())
        };
        let (asked, ended) = stopped("u", StopBy::User, None, None);
        mentions(&asked, &["user", "asked", "stop"]);
        mentions(&ended, &["Stopped", "user"]);
        mentions(
            &stopped("a", StopBy::Agent, Some("Loss went NaN"), None).1,
            &["Auto-stopped", "agent", "Loss went NaN"],
        );
        let reason = "loss was NaN on 3 reports in a row";
        let (asked, ended) = stopped(
            "r",
            StopBy::Rule,
            Some(reason),
            Some(json!({"name": "nan-loss"})),
        );
        mentions(
            &asked,
            &["Stopping", "rule", "nan-loss", reason, "No model"],
        );
        mentions(
            &ended,
            &["Auto-stopped", "rule", "nan-loss", reason, "No model"],
        );
        f.run_named("n", "cancelled", local());
        let never = super::monitor(&f.store, &f.store.get_run("n").unwrap().unwrap())
            .unwrap()
            .1;
        assert_eq!(
            status_line(&never, "stop"),
            "",
            "no one is named for an old stop"
        );
    }

    /// Experiment settings as orx stored them before plans, and with one beside a key a newer
    /// orx may add: each reads, and saving one part keeps every other.
    #[test]
    fn stored_settings_with_and_without_a_plan_read_and_keep_each_other() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/contracts/run-monitor-config.json"
        ))
        .unwrap();
        let (before, planned) = (
            fixture["before"].as_str().unwrap(),
            fixture["planned"].as_str().unwrap(),
        );
        use crate::engine::view::{stored, stored_patterns, stored_plan};
        assert_eq!(stored_plan("exp", Some(before)).unwrap(), vec![]);
        let names: Vec<_> = stored_plan("exp", Some(planned))
            .unwrap()
            .into_iter()
            .map(|stage| stage.phase)
            .collect();
        assert_eq!(names, ["install", "train", "eval", "train"]);
        assert!(
            stored("exp", Some(planned)).unwrap().is_some(),
            "a reader of the view ignores the plan"
        );
        assert!(stored_patterns("exp", Some(planned))
            .unwrap()
            .patterns
            .is_empty());

        let f = Fixture::new("orx-monitor-plan-contract");
        let keep = |raw: &str| {
            f.store
                .patch_run_monitor_json(
                    "exp",
                    &crate::engine::view::etag(f.store.run_monitor_json("exp").unwrap().as_deref()),
                    |_| Ok(raw.to_owned()),
                )
                .unwrap()
                .unwrap();
        };
        keep(planned);
        f.view("exp", "<view />");
        let saved: Value =
            serde_json::from_str(&f.store.run_monitor_json("exp").unwrap().unwrap()).unwrap();
        assert_eq!(
            saved["plan"].as_array().unwrap().len(),
            4,
            "a new view keeps the plan"
        );
        assert_eq!(saved["later"], json!({"kept": true}));

        let ruled = fixture["ruled"].as_str().unwrap();
        let names: Vec<_> = crate::engine::rules::stored("exp", Some(ruled))
            .unwrap()
            .into_iter()
            .map(|rule| rule.name)
            .collect();
        assert_eq!(names, ["nan-loss", "diverging"]);
        assert!(crate::engine::rules::stored("exp", Some(planned))
            .unwrap()
            .is_empty());
        keep(ruled);
        plan(&f, "exp", json!([{"phase": "eval"}])).unwrap();
        let saved: Value =
            serde_json::from_str(&f.store.run_monitor_json("exp").unwrap().unwrap()).unwrap();
        let stored: Value = serde_json::from_str(ruled).unwrap();
        assert_eq!(saved["rules"], stored["rules"], "a plan keeps the rules");
        assert_eq!(saved["later"], json!({"kept": true}));

        let watched = fixture["watched"].as_str().unwrap();
        let configure = |config: Value| {
            let source = crate::engine::view::Source {
                file: None,
                patterns: None,
                config: Some("config.json"),
            };
            let raw = f.store.run_monitor_json("exp").unwrap();
            let etag = crate::engine::view::etag(raw.as_deref());
            crate::engine::view::set(&f.store, "exp", &etag, None, None, Some(&config), &source)
                .unwrap()
        };
        let stored: Value = serde_json::from_str(watched).unwrap();
        keep(watched);
        let described = configure(
            json!({"rules": [{"name": "stuck", "text": "Stop after 10 minutes without progress.",
            "does": "stop", "when": {"kind": "noProgress", "seconds": 600}}]}),
        );
        assert_eq!(described["brief"], stored["brief"], "rules keep the brief");
        assert_eq!(described["watch"], stored["watch"], "rules keep the gaps");
        keep(watched);
        let described = configure(json!({"brief": {"noise": "Eval loss jumps at each shard."}}));
        assert_eq!(
            described["rules"], stored["rules"],
            "a brief keeps the rules"
        );
        assert_eq!(described["plan"], stored["plan"], "a brief keeps the plan");
        let saved: Value =
            serde_json::from_str(&f.store.run_monitor_json("exp").unwrap().unwrap()).unwrap();
        assert_eq!(saved["watch"], stored["watch"]);
        assert_eq!(saved["later"], json!({"kept": true}));

        keep(before);
        plan(&f, "exp", json!([{"phase": "train"}])).unwrap();
        let saved: Value =
            serde_json::from_str(&f.store.run_monitor_json("exp").unwrap().unwrap()).unwrap();
        assert_eq!(
            saved["patterns"].as_array().unwrap().len(),
            1,
            "a plan keeps the patterns"
        );
        assert!(saved["view"].as_str().unwrap().contains("<chart"));
        let run = f.run("running", local());
        f.collect(&folded(&declared(0..=20, 400)));
        let (monitor, _) = monitor_at(&f, &run, T0 + 20_500);
        assert!(monitor["phases"][0].get("planned").is_some());

        keep(before);
        let (monitor, _) = monitor_at(&f, &run, T0 + 20_600);
        assert!(
            monitor["phases"][0].get("planned").is_none(),
            "with no plan a run shows as it did"
        );
        assert_eq!(monitor["progress"]["segments"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_child_finds_an_unpublished_parent_curve() {
        let f = Fixture::new("orx-monitor-unpublished-parent");
        f.view("exp", &VIEW.replace(" ghost=\"parent\"", ""));
        f.run_in("p1", "exp", "done", local());
        let mut parent = Snapshot::new("p1", "1-1", "g1");
        fold(&mut parent, &[(T0, "::orx:: {\"phase\":\"train\"}".into())]);
        fold(&mut parent, &declared(0..=20, 400));
        f.signals(&parent);
        f.child("child", "exp");
        let child = loss_run(&f, "c1", "child", "running", 10, 1.0);
        let (monitor, _) = monitor_at(&f, &child, T0 + 10_500);
        assert_eq!(monitor["parent"]["runId"], "p1");
        assert!(
            monitor["parent"]["series"]["loss"]
                .as_array()
                .unwrap()
                .len()
                > 1
        );

        let mut child_signals = Snapshot::new("c1", "1-1", "g2");
        fold(
            &mut child_signals,
            &[(T0, "::orx:: {\"phase\":\"eval\"}".into())],
        );
        fold(&mut child_signals, &declared(0..=2, 100));
        f.signals(&child_signals);
        let (monitor, _) = monitor_at(&f, &child, T0 + 12_500);
        assert!(monitor["parent"]["series"].as_object().unwrap().is_empty());
    }

    #[test]
    fn phase_gates_wait_for_a_named_phase() {
        let f = Fixture::new("orx-monitor-unknown-phase");
        f.view("exp", "<view><chart y=\"loss\" phase=\"train\" /></view>");
        let run = f.run("running", local());
        let mut snapshot = folded(&declared(0..=2, 100));
        f.signals(&snapshot);
        let (monitor, _) = monitor_at(&f, &run, T0 + 2_500);
        assert!(monitor["view"]["blocks"][0].get("phase").is_none());

        fold(
            &mut snapshot,
            &[(T0 + 3_000, "::orx:: {\"phase\":\"eval\"}".into())],
        );
        f.signals(&snapshot);
        let (monitor, _) = monitor_at(&f, &run, T0 + 3_500);
        assert_eq!(monitor["view"]["blocks"][0]["phase"], "train");
    }

    #[test]
    fn a_run_compares_with_its_parent_at_the_same_step_and_nowhere_else() {
        let f = Fixture::new("orx-monitor-parent");
        f.view("exp", VIEW);
        let parent = loss_run(&f, "p1", "exp", "done", 20, 1.0);
        monitor_at(&f, &parent, T0 + 700_000);
        f.child("child", "exp");
        f.view("exp", "<view><stats of=\"loss\" /></view>");
        let child = loss_run(&f, "c1", "child", "running", 10, 2.5 / 3.0);
        let (monitor, _) = monitor_at(&f, &child, T0 + 10_500);

        assert_eq!(
            monitor["viewFrom"]["markup"], VIEW,
            "a child keeps the view its parent had"
        );
        let (mine, theirs) = (2.5 / 11.0, 3.0 / 11.0);
        let vs = &monitor["versus"]["step"]["loss"];
        assert!(
            (vs["d"].as_f64().unwrap() - (mine - theirs)).abs() < 1e-12,
            "{vs}"
        );
        assert_eq!(
            (&vs["good"], &vs["flat"], &vs["mag"], &vs["parent"]),
            (&json!(true), &json!(false), &json!(0.045), &json!(theirs))
        );
        assert_eq!(monitor["parent"]["runId"], "p1");
        assert_eq!(
            monitor["parent"]["series"]["loss"]
                .as_array()
                .unwrap()
                .len(),
            21,
            "drawn under the child's curve"
        );
        let text = monitor["statusText"].as_str().unwrap();
        mentions(
            text,
            &["Loss 0.227", "0.045", "better", "parent", "step 100"],
        );
        let summary = current(&f.store, &child)
            .unwrap()
            .unwrap()
            .summary()
            .clone();
        assert_eq!(
            summary["bindings"]["loss.vs"], *vs,
            "a running child meets the same step"
        );
        assert_eq!(summary["bindings"]["val_loss.vs"], Value::Null);

        // Past the parent's last step there is nothing to compare with: no comparison, not level.
        let child = loss_run(&f, "c2", "child", "running", 30, 2.5 / 3.0);
        let (monitor, _) = monitor_at(&f, &child, T0 + 30_500);
        assert_eq!(monitor["versus"]["step"], json!({}));
        assert!(!monitor["statusText"].as_str().unwrap().contains("parent"));
    }

    /// A run that trains for `steps` of 10 at loss `scale * 3 / (i + 1)`, then evaluates without
    /// steps, ending at loss `last` when it is given.
    fn staged_run(
        f: &Fixture,
        id: &str,
        experiment: &str,
        status: &str,
        steps: i64,
        scale: f64,
        last: Option<f64>,
    ) -> StoredRun {
        let run = f.run_in(id, experiment, status, local());
        let mut lines: Vec<(i64, String)> = (0..=steps)
            .map(|i| {
                let loss = scale * 3.0 / (i + 1) as f64;
                let line = format!("::orx:: {{\"phase\":\"train\",\"step\":{},\"of\":{},\"t\":{i},\"m\":{{\"loss\":{loss}}}}}", i * 10, steps * 10);
                (T0 + i * 1000, line)
            })
            .collect();
        let after = steps + 1;
        lines.push((
            T0 + after * 1000,
            format!("::orx:: {{\"phase\":\"eval\",\"t\":{after}}}"),
        ));
        if let Some(loss) = last {
            lines.push((
                T0 + after * 1000 + 500,
                format!("::orx:: {{\"t\":{after}.5,\"m\":{{\"loss\":{loss}}}}}"),
            ));
        }
        f.collect(&Snapshot {
            run_id: id.into(),
            ..folded(&lines)
        });
        run
    }

    #[test]
    fn a_stage_that_ended_compares_at_its_last_step_until_the_run_ends() {
        let f = Fixture::new("orx-monitor-stage-ended");
        f.view("exp", VIEW);
        let parent = staged_run(&f, "p1", "exp", "done", 500, 1.0, None);
        monitor_at(&f, &parent, T0 + 700_000);
        f.child("child", "exp");
        let child = staged_run(&f, "c1", "child", "running", 3, 0.9, None);
        let (monitor, event) = monitor_at(&f, &child, T0 + 5_000);

        // The child's last train step is 30, at 0.675 against the parent's 0.75 there; the parent
        // ended its 5,000 steps at 0.006.
        let vs = &monitor["versus"]["step"]["loss"];
        assert_eq!(
            (&vs["good"], &vs["step"], &vs["mag"]),
            (&json!(true), &json!(30.0), &json!(0.075)),
            "{vs}"
        );
        assert_eq!(monitor["versus"]["final"], json!({}));
        assert_eq!(event.unwrap()["versus"], monitor["versus"]);
        let text = monitor["statusText"].as_str().unwrap();
        mentions(
            text,
            &["Loss 0.675", "0.075", "better", "parent", "step 30"],
        );

        // An unstepped report does not use the old step, even when the value is the same.
        let child = staged_run(&f, "c2", "child", "running", 3, 0.9, Some(0.675));
        let (monitor, _) = monitor_at(&f, &child, T0 + 5_000);
        assert_eq!(
            monitor["versus"]["step"].get("loss"),
            None,
            "{}",
            monitor["versus"]
        );

        let ended = staged_run(&f, "c3", "child", "done", 3, 0.9, None);
        let (monitor, _) = monitor_at(&f, &ended, T0 + 700_000);
        let text = monitor["statusText"].as_str().unwrap();
        mentions(text, &["Loss 0.675", "0.669", "worse", "parent", "final"]);

        f.view(
            "child",
            &VIEW.replace(
                "better=\"lower\" />\n<signal name=\"val_loss\"",
                "better=\"lower\" vs=\"step\" />\n<signal name=\"val_loss\"",
            ),
        );
        let (monitor, _) = monitor_at(&f, &ended, T0 + 701_000);
        let text = monitor["statusText"].as_str().unwrap();
        mentions(
            text,
            &["Loss 0.675", "0.075", "better", "parent", "step 30"],
        );
    }

    #[test]
    fn a_result_reported_after_its_steps_compares_with_what_the_parent_ended_with() {
        let f = Fixture::new("orx-monitor-result");
        f.view("exp", "<signal name=\"front_gain\" label=\"Frontier gain\" better=\"higher\" sign=\"true\" />\n<view headline=\"front_gain\"><stats of=\"front_gain\" /></view>");
        let config: orx_signals::Config = serde_json::from_value(json!({"patterns": [
            {"regex": "^chunk (?P<step>\\d+)/(?P<of>\\d+) done", "phase": "compress", "unit": "chunks"},
            {"regex": "^== reference self-check", "phase": "score"},
            {"regex": "^METRIC (?P<name>\\w+) (?P<value>\\S+)$"},
        ]}))
        .unwrap();
        let patterns = Arc::new(orx_signals::Patterns::compile(&config).unwrap());
        let log = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("crates/orx-signals/tests/fixtures/codec.out"),
        )
        .unwrap();
        let folded = |id: &str, log: &str| {
            let mut snapshot = Snapshot::new(id, "1-1", "g1");
            snapshot.configure(Some(patterns.clone()));
            // Read live, line by line, a second apart: points sit at elapsed time, not at a step.
            for (i, line) in log.lines().enumerate() {
                snapshot
                    .feed(
                        format!("{line}\n").as_bytes(),
                        false,
                        T0 + i as i64 * 1000,
                        true,
                    )
                    .unwrap();
            }
            snapshot
        };
        let parent = f.run_in("p1", "exp", "done", local());
        f.collect(&folded("p1", &log));
        monitor_at(&f, &parent, T0 + 700_000);
        f.child("child", "exp");
        let child_log = log.replace("METRIC front_gain 0.0213", "METRIC front_gain 0.412");
        let child = f.run_in("c1", "child", "running", local());
        f.collect(&folded("c1", &child_log));
        let (monitor, _) = monitor_at(&f, &child, T0 + 40_000);
        assert_eq!(monitor["versus"], json!({"step": {}, "final": {}}));

        let child = f.run_in("c2", "child", "done", local());
        f.collect(&folded("c2", &child_log));
        let (monitor, _) = monitor_at(&f, &child, T0 + 700_000);
        let vs = &monitor["versus"]["final"]["front_gain"];
        assert_eq!(
            (&vs["good"], &vs["parent"]),
            (&json!(true), &json!(0.0213)),
            "{vs}"
        );
        let text = monitor["statusText"].as_str().unwrap();
        mentions(
            text,
            &["Frontier gain +0.412", "0.391", "better", "parent", "final"],
        );
    }

    #[test]
    fn a_comparison_has_a_direction_only_when_the_view_declares_one() {
        let f = Fixture::new("orx-monitor-better");
        f.view("exp", "<signal name=\"loss\" label=\"Loss\" />\n<view headline=\"loss\"><chart y=\"loss\" /></view>");
        let parent = loss_run(&f, "p1", "exp", "done", 20, 1.0);
        monitor_at(&f, &parent, T0 + 700_000);
        f.child("child", "exp");
        let child = loss_run(&f, "c1", "child", "running", 10, 1.0);
        let (monitor, _) = monitor_at(&f, &child, T0 + 10_500);
        assert_eq!(monitor["versus"], json!({"step": {}, "final": {}}));
        assert_eq!(monitor["parent"]["runId"], "p1");
    }

    #[test]
    fn a_child_keeps_the_view_it_was_created_with() {
        let f = Fixture::new("orx-monitor-inherit");
        f.view("exp", VIEW);
        f.child("child", "exp");
        f.view("exp", "<view><stats of=\"loss\" /></view>");
        let stored = |id: &str| {
            crate::engine::view::stored(id, f.store.run_monitor_json(id).unwrap().as_deref())
                .unwrap()
        };
        assert_eq!(stored("child").as_deref(), Some(VIEW));
        assert_eq!(
            stored("exp").as_deref(),
            Some("<view><stats of=\"loss\" /></view>")
        );
        f.child("grandchild", "child");
        assert_eq!(stored("grandchild").as_deref(), Some(VIEW));
    }

    #[test]
    fn a_saved_view_this_orx_cannot_read_draws_the_default_and_says_why() {
        let f = Fixture::new("orx-monitor-unreadable");
        let run = loss_run(&f, "r1", "exp", "running", 5, 1.0);
        f.collected(T0 + 5_000);
        f.store
            .patch_run_monitor_json("exp", &crate::engine::view::etag(None), |_| {
                Ok(json!({"view": "<view><heatmap /></view>", "brief": []}).to_string())
            })
            .unwrap()
            .unwrap();
        let (monitor, _) = monitor_at(&f, &run, T0 + 5_500);
        assert_eq!(monitor["viewFrom"]["isDefault"], true);
        mentions(
            status_line(&monitor, "view"),
            &["exp:1:7", "unknown element", "heatmap", "default view"],
        );
        // Saving keeps what a newer orx stored beside the view.
        f.view("exp", "<view><stats of=\"loss\" /></view>");
        let raw: Value =
            serde_json::from_str(&f.store.run_monitor_json("exp").unwrap().unwrap()).unwrap();
        assert_eq!(raw["brief"], json!([]));
    }

    #[test]
    fn saving_a_view_refuses_each_mistake_in_its_own_words() {
        use crate::engine::view::{check, etag, set, Refusal, Source};
        let f = Fixture::new("orx-monitor-refusals");
        let file = Source {
            file: Some("view.xml"),
            patterns: Some("patterns.json"),
            config: None,
        };
        let invalid = |answer: std::result::Result<Value, Refusal>| match answer {
            Err(Refusal::Invalid(message)) => message,
            other => panic!("{other:?}"),
        };
        let view = |markup| {
            set(
                &f.store,
                "exp",
                &etag(None),
                Some(markup),
                None,
                None,
                &file,
            )
        };
        // Before any run there is nothing to check a view against.
        let unknown_element = |command| {
            let error = invalid(view("<view><html/></view>"));
            mentions(
                &error,
                &["view.xml:1:7", "unknown element", "html", command],
            );
            mentions(&error, crate::engine::view::ELEMENTS);
        };
        unknown_element("orx exp view set exp --file view.xml");
        loss_run(&f, "r1", "exp", "running", 5, 1.0);
        unknown_element("orx exp view check r1 --file view.xml");
        let error = invalid(view("<view>\n  <chart y=\"los\" />\n</view>"));
        mentions(
            &error,
            &[
                "view.xml:2:3",
                "blocks[0].chart[0]",
                "no signal",
                "los",
                "loss",
                "orx exp view set exp --file view.xml",
            ],
        );
        let Err(Refusal::Changed(message)) = set(
            &f.store,
            "exp",
            "\"stale\"",
            Some("<view/>"),
            None,
            None,
            &file,
        ) else {
            panic!("a stale etag should be refused");
        };
        mentions(&message, &["exp", "changed", "orx exp view get exp"]);
        let Err(Refusal::Missing(message)) = set(
            &f.store,
            "nope",
            &etag(None),
            Some("<view/>"),
            None,
            None,
            &file,
        ) else {
            panic!("a missing experiment should be refused");
        };
        mentions(&message, &["nope", "not found"]);
        let patterns =
            |list: Value| set(&f.store, "exp", &etag(None), None, Some(&list), None, &file);
        let error = invalid(patterns(json!([{"regex": "^chunk (?P<step\\d+)"}])));
        mentions(
            &error,
            &[
                "patterns.json",
                "pattern 1",
                "invalid regex",
                "column 16",
                "capture group",
                "orx exp view check r1 --pattern patterns.json",
            ],
        );
        let error = invalid(patterns(json!([{"regex": "^x", "candidates": "^y"}])));
        mentions(
            &error,
            &[
                "patterns.json",
                "JSON list",
                "candidates",
                "regex",
                "phase",
                "unit",
                "row",
                "orx exp view check r1 --pattern patterns.json",
            ],
        );
        let stdin = Source {
            file: None,
            patterns: None,
            config: None,
        };
        let error = invalid(check(
            &f.store,
            "r1",
            Some("<view><chart y=\"lr\" /></view>"),
            None,
            &stdin,
        ));
        mentions(
            &error,
            &[
                "<stdin>:1:7",
                "blocks[0].chart[0]",
                "no signal",
                "lr",
                "loss",
                "orx exp view set exp --stdin",
            ],
        );
        let error = invalid(set(
            &f.store,
            "exp",
            &etag(None),
            Some("<view><html/></view>"),
            None,
            None,
            &stdin,
        ));
        assert!(error.contains("orx exp view set exp --stdin"), "{error}");
        let report = check(&f.store, "r1", None, None, &file).unwrap();
        assert_eq!(report["markup"], Value::Null);
        assert_eq!(report["seen"]["signals"], json!(["loss"]));
    }

    /// A frozen benchmark harness, read by patterns saved beside the experiment's view.
    #[test]
    fn patterns_saved_on_an_experiment_reach_its_runs_and_its_check() {
        use crate::engine::view::{check, etag, set, Source};
        let f = Fixture::new("orx-monitor-patterns");
        let run = f.run("running", local());
        let list = json!([
            {"regex": "^chunk (?P<step>\\d+)/(?P<of>\\d+) done", "phase": "compress", "unit": "chunk"},
            {"regex": "^== reference self-check", "phase": "score"},
            {"regex": "^METRIC (?P<name>\\w+) (?P<value>\\S+)$"},
            {"regex": "^never$"},
        ]);
        let source = Source {
            file: None,
            patterns: Some("patterns.json"),
            config: None,
        };
        let saved = set(
            &f.store,
            "exp",
            &etag(None),
            None,
            Some(&list),
            None,
            &source,
        )
        .unwrap();
        assert_eq!(saved["patterns"], list);
        assert_eq!(saved["markup"], Value::Null, "the view is kept as it was");
        let config = f.store.run_patterns(&run.id).unwrap();
        assert_eq!(json!(config.patterns), list);

        let mut folded = Snapshot::new(&run.id, "1-1", "check");
        folded.tally = Some(Default::default());
        folded.configure(Some(Arc::new(
            orx_signals::Patterns::compile(&config).unwrap(),
        )));
        let log = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("crates/orx-signals/tests/fixtures/codec.out"),
        )
        .unwrap();
        folded.feed(&log, true, T0, false).unwrap();
        let report = check(&f.store, &run.id, None, Some(&folded), &source).unwrap();
        assert_eq!(report["lines"], 40);
        let matched: Vec<&Value> = report["patterns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| &p["matched"])
            .collect();
        assert_eq!(matched, [&json!(10), &json!(1), &json!(10), &json!(0)]);
        assert_eq!(
            report["patterns"][1]["first"],
            json!({"line": 17, "text": "== reference self-check: ok"})
        );
        let stages: Vec<&Value> = report["stages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| &s["name"])
            .collect();
        assert_eq!(stages, [&json!("compress"), &json!("score")]);
        assert!(report["seen"]["signals"]
            .as_array()
            .unwrap()
            .contains(&json!("front_gain")));
    }

    /// What this server writes is what the page's tests read: `tests/contracts/run-monitor-*.json`.
    #[test]
    fn the_monitor_and_its_events_match_the_approved_fixtures() {
        let f = Fixture::new("orx-monitor-contract");
        let run = f.run("running", local());
        let mut snapshot = folded(&declared(0..=20, 400));
        f.collect(&snapshot);
        f.collected(T0 + 20_000);
        let (before, first) = monitor_at(&f, &run, T0 + 20_500);
        fold(&mut snapshot, &declared(21..=22, 400));
        f.collect(&snapshot);
        f.collected(T0 + 22_000);
        let (after, next) = monitor_at(&f, &run, T0 + 22_500);

        let mut events = [first.unwrap(), next.unwrap()];
        assert_eq!(events[0]["statusText"], before["statusText"]);
        assert_eq!(events[1]["statusText"], after["statusText"]);

        let open = folded(&[
            (T0 + 1000, "::orx:: {\"step\":23,\"m\":{\"loss\":2}}".into()),
            (
                T0 + 2000,
                "::orx:: {\"step\":24,\"m\":{\"loss\":1.9}}".into(),
            ),
        ]);
        let limited = folded(&[(T0 + 1000, "::orx:: {\"step\":5,\"m\":{\"loss\":2}}".into())]);
        let slurm = json!({"kind": "slurm_job", "timeoutSecs": 3600});
        let states: serde_json::Map<String, Value> = [
            state(
                "running",
                "running",
                local(),
                &folded(&declared(0..=20, 400)),
                T0 + 20_000,
                T0 + 20_500,
                json!({"duration": "19s", "range": ["18s", "20s"], "cli": ["left"]}),
            ),
            state(
                "nan",
                "running",
                local(),
                &folded(&declared(0..=5, 400)),
                T0 + 5_000,
                T0 + 5_000,
                json!({"duration": "35s", "range": ["31s", "38s"], "cli": ["left"], "measures": {"loss": "NaN"}}),
            ),
            state(
                "stall",
                "running",
                local(),
                &folded(&declared(0..=10, 400)),
                T0 + 3_601_000,
                T0 + 3_601_000,
                json!({"duration": "59m", "cli": ["No progress", "due", "00:45 UTC"]}),
            ),
            state(
                "limit",
                "running",
                slurm,
                &limited,
                T0 + 900_000,
                T0 + 900_000,
                json!({"duration": "45m", "cli": ["limit", "01:45 UTC"]}),
            ),
            state(
                "open",
                "running",
                local(),
                &open,
                T0 + 2_000,
                T0 + 20_000,
                json!({"noEstimate": "noTotal", "cli": ["No estimate", "no total", "limit"]}),
            ),
            state(
                "ended",
                "done",
                local(),
                &folded(&declared(0..=10, 100)),
                T0 + 10_000,
                T0 + 700_000,
                json!({"duration": "10m", "cli": ["Done"]}),
            ),
            state(
                "full",
                "running",
                local(),
                &epochs(0, 4, 4),
                T0 + 125_000,
                T0 + 125_000,
                json!({"noEstimate": "unknownLeft", "cli": ["No estimate", "left", "unknown"]}),
            ),
            state(
                "long",
                "running",
                local(),
                &folded(&declared(0..=10, 1_000_000)),
                T0 + 10_000,
                T0 + 10_000,
                json!({"duration": "1d 3h", "range": ["13h 53m", "1d 20h"], "cli": ["left"]}),
            ),
            state(
                "epochs",
                "running",
                local(),
                &epochs(3, 4, 1),
                T0 + 20_000,
                T0 + 20_000,
                json!({"noEstimate": "unknownLeft", "cli": ["No estimate", "left", "unknown"]}),
            ),
            state(
                "first",
                "running",
                local(),
                &folded(&[(T0 + 1000, "::orx:: {\"m\":{\"loss\":2.5}}".into())]),
                T0 + 1000,
                T0 + 10_000,
                json!({"noEstimate": "noProgress", "cli": ["No estimate", "no progress", "log"]}),
            ),
            state(
                "measures",
                "running",
                local(),
                &folded(&[
                    (T0 + 1000, "::orx:: {\"row\":\"rounds\",\"m\":{\"round\":14,\"win_rate\":0.507,\"elo\":1290.5,\"game_len\":79.94}}".into()),
                    (T0 + 1000, "::orx:: {\"phase\":\"round\",\"unit\":\"rounds\",\"step\":14,\"of\":24,\"m\":{\"lr\":0.0003,\"lr_min\":0.000132,\"loss\":0.48213,\"ratio\":1.125,\"drift\":-0.0,\"tokens\":123456789,\"flops\":1e15}}".into()),
                ]),
                T0 + 1000,
                T0 + 5_000,
                json!({
                    "cli": ["Estimating", "timed progress"],
                    "stage": "round 14 of 24",
                    "columns": ["round", "win_rate", "elo", "game_len"],
                    "headline": "lr",
                    "measures": {
                        "round": "14", "win_rate": "0.507", "elo": "1,291", "game_len": "79.9",
                        "lr": "3e-4", "lr_min": "1.32e-4", "loss": "0.482", "ratio": "1.13",
                        "drift": "0", "tokens": "123,456,789", "flops": "1e15",
                    },
                    "declared": [
                        {"value": 0.0003, "spec": {"label": "lr", "sign": true, "unit": "%"}, "words": "+3e-4%"},
                        {"value": -0.000132, "spec": {"label": "lr", "sign": true}, "words": "-1.32e-4"},
                        {"value": 1.125e15, "spec": {"label": "flops", "unit": "%"}, "words": "1.13e15%"},
                        {"value": 2.675, "spec": {"label": "gain", "sign": true, "unit": "%"}, "words": "+2.68%"},
                        {"value": -0.0, "spec": {"label": "drift", "sign": true}, "words": "0"},
                        {"value": 0.48213, "spec": {"label": "Loss", "digits": 2}, "words": "0.48"},
                        {"value": 1234.567, "spec": {"label": "elo", "digits": 2}, "words": "1,235"},
                        {"value": 2.675, "spec": {"label": "gain", "digits": 1, "sign": true, "unit": "%"}, "words": "+2.7%"},
                        {"value": -0.004, "spec": {"label": "drift", "digits": 2, "sign": true}, "words": "0.00"},
                        {"value": 0.123, "spec": {"label": "accuracy", "digits": 1, "scale": 100, "unit": "%"}, "words": "12.3%"},
                        {"value": 123456789, "spec": {"label": "tokens", "as": "compact"}, "words": "123M"},
                        {"value": 1234, "spec": {"label": "tokens", "as": "compact"}, "words": "1.2K"},
                        {"value": 3725, "spec": {"label": "wall", "as": "duration"}, "words": "1h 2m"},
                        {"value": 1, "spec": {"label": "Bit-exact", "labels": {"1": "valid", "0": "invalid"}}, "words": "valid"},
                        {"value": 0.0, "spec": {"label": "Bit-exact", "labels": {"1": "valid", "0": "invalid"}}, "words": "invalid"},
                        {"value": "1", "spec": {"label": "Bit-exact", "labels": {"1": "valid", "0": "invalid"}}, "words": "valid"},
                        {"value": 2, "spec": {"label": "Bit-exact", "labels": {"1": "valid", "0": "invalid"}}, "words": "2"},
                    ],
                }),
            ),
            state(
                "announced",
                "running",
                local(),
                &folded(&[(T0 + 1000, "::orx:: {\"phase\":\"train\"}".into())]),
                T0 + 1000,
                T0 + 5_000,
                json!({"cli": ["train", "started"]}),
            ),
            state(
                "setup",
                "running",
                local(),
                &folded(&[
                    (T0 + 1000, "Downloading shard_00001.parquet...".into()),
                    (T0 + 2000, "Successfully downloaded shard_00001.parquet".into()),
                ]),
                T0 + 2_000,
                T0 + 10_000,
                json!({"cli": ["Setting up", "Successfully downloaded shard_00001.parquet"]}),
            ),
            state(
                "detected",
                "running",
                local(),
                &folded(
                    &(1..=3)
                        .map(|n| (T0 + 6_000 * n, format!("chunk {n}/10 done, {}s", 124 * n)))
                        .collect::<Vec<_>>(),
                ),
                T0 + 18_000,
                T0 + 18_000,
                json!({
                    "duration": "42s",
                    "cli": ["At least", "left", "progress in the log", "more work"],
                    "stage": "3 of 10 chunks",
                }),
            ),
            state(
                "planned",
                "running",
                local(),
                &folded(
                    &std::iter::once((T0, "::orx:: {\"plan\":[{\"phase\":\"download\",\"of\":3},{\"phase\":\"train\",\"of\":10,\"unit\":\"steps\"},{\"phase\":\"eval\"}]}".to_owned()))
                        .chain((0..=3).map(|n| (T0 + 1000 * n, format!("::orx:: {{\"phase\":\"download\",\"step\":{n},\"of\":3}}"))))
                        .chain((0..=5).map(|n| (T0 + 4000 + 1000 * n, format!("::orx:: {{\"phase\":\"train\",\"step\":{n},\"of\":10,\"unit\":\"steps\",\"m\":{{\"loss\":{}}}}}", 3.0 / (n + 1) as f64))))
                        .collect::<Vec<_>>(),
                ),
                T0 + 9_000,
                T0 + 9_000,
                json!({
                    "duration": "5s",
                    "cli": ["5s of train left", "earlier run", "timed", "eval"],
                    "stage": "train (3/4) · step 5 of 10",
                }),
            ),
            state(
                "named",
                "running",
                local(),
                &folded(&[(T0 + 1000, "::orx:: {\"phase\":\"train\",\"unit\":\"steps\",\"step\":2280,\"of\":4200}".into())]),
                T0 + 1000,
                T0 + 5_000,
                json!({
                    "cli": ["Estimating", "timed progress"],
                    "stage": "train · step 2,280 of 4,200",
                }),
            ),
        ]
        .into_iter()
        .collect();
        let mut states = states;
        states.insert("failing".into(), failing_state());
        states.insert("unreadable".into(), unreadable_state());
        states.insert("failing_done".into(), failing_done_state());
        states.insert("compared".into(), compared_state());
        states.insert("flagged".into(), flagged_state());
        // Keep the wire field and check its type without snapshotting the CLI's prose.
        for body in states
            .values_mut()
            .map(|state| &mut state["monitor"])
            .chain(&mut events)
        {
            let text = body["statusText"].as_str().expect("statusText is a string");
            assert!(!text.is_empty());
            body["statusText"] = json!("");
        }
        let api = json!({"states": states});
        let events = json!(events);
        for (name, actual) in [
            ("run-monitor-api.json", api),
            ("run-monitor-events.json", events),
        ] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/contracts")
                .join(name);
            if std::env::var_os("ORX_WRITE_CONTRACTS").is_some() {
                std::fs::write(&path, serde_json::to_string_pretty(&actual).unwrap() + "\n")
                    .unwrap();
            }
            let expected: Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(
                actual,
                expected,
                "{name}: {}",
                serde_json::to_string_pretty(&actual).unwrap()
            );
        }
    }

    /// A run whose store refuses its next projection: the summary every list then carries, beside
    /// the last monitor a page holds.
    fn failing_state() -> Value {
        let f = Fixture::new("orx-monitor-contract-failing");
        let run = f.run("running", local());
        let mut snapshot = folded(&declared(0..=20, 400));
        f.collect(&snapshot);
        f.collected(T0 + 20_000);
        let (monitor, _) = monitor_at(&f, &run, T0 + 20_000);
        f.failing("r1", true);
        fold(&mut snapshot, &declared(21..=22, 400));
        f.collect(&snapshot);
        f.collected(T0 + 22_000);
        assert!(publish(&f.store, &run, T0 + 22_000, None).is_err());
        let mut summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
        assert_eq!(summary["unavailable"], MONITOR_FAILED);
        summary["unavailable"] = json!("Projection failed");
        let run = json!({
            "status": "running", "createdAt": T0, "startedAt": T0, "updatedAt": T0,
            "endedAt": null, "backend": local(),
        });
        let expect = json!({});
        json!({"run": run, "now": T0 + 22_000, "monitor": monitor, "summary": summary, "expect": expect})
    }

    /// A run that ended while its projection fails: its summary says how it ended all the same.
    fn failing_done_state() -> Value {
        let f = Fixture::new("orx-monitor-contract-failing-done");
        let run = f.run("running", local());
        f.collect(&folded(&declared(0..=20, 400)));
        f.collected(T0 + 20_000);
        let (monitor, _) = monitor_at(&f, &run, T0 + 20_000);
        f.failing("r1", true);
        let run = f.run("done", local());
        assert!(publish(&f.store, &run, T0 + 22_000, None).is_err());
        let mut summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
        assert_eq!(summary["unavailable"], MONITOR_FAILED);
        summary["unavailable"] = json!("Projection failed");
        let run = json!({
            "status": "done", "createdAt": T0, "startedAt": T0, "updatedAt": T0 + 22_000,
            "endedAt": T0 + 22_000, "backend": local(),
        });
        let expect = json!({});
        json!({"run": run, "now": T0 + 22_000, "monitor": monitor, "summary": summary, "expect": expect})
    }

    /// A run drawn with the view its experiment's agent wrote, compared with its parent's run.
    fn compared_state() -> Value {
        let f = Fixture::new("orx-monitor-contract-compared");
        f.view("exp", VIEW);
        let parent = loss_run(&f, "p1", "exp", "done", 20, 1.0);
        monitor_at(&f, &parent, T0 + 700_000);
        f.child("child", "exp");
        let child = loss_run(&f, "c1", "child", "running", 10, 2.5 / 3.0);
        let (monitor, _) = monitor_at(&f, &child, T0 + 10_000);
        let summary = current(&f.store, &child)
            .unwrap()
            .unwrap()
            .summary()
            .clone();
        let run = json!({
            "status": "running", "createdAt": T0, "startedAt": T0, "updatedAt": T0,
            "endedAt": null, "backend": local(),
        });
        let expect = json!({"delta": "0.045"});
        mentions(
            status_line(&monitor, "signals"),
            &[
                "Loss 0.227",
                expect["delta"].as_str().unwrap(),
                "better",
                "parent",
                "step 100",
            ],
        );
        json!({"run": run, "now": T0 + 10_000, "monitor": monitor, "summary": summary, "expect": expect})
    }

    /// A run the watcher flagged whose chat is marked done: the flag waits for the user's choice.
    fn flagged_state() -> Value {
        let f = Fixture::new("orx-monitor-contract-flagged");
        let run = f.run("running", local());
        f.store.put_chat_session("chat-1", "claude-code");
        f.store.set_chat_session_archived("chat-1", true).unwrap();
        f.store
            .conn_execute_for_tests("UPDATE runs SET chat_session_id = 'chat-1' WHERE id = 'r1'");
        f.collect(&folded(&declared(0..=20, 400)));
        f.collected(T0 + 20_000);
        let row = f.store.get_run("r1").unwrap().unwrap();
        let body = json!({"id": "f1", "source": "look", "lookId": "7", "at": T0 + 19_000,
            "whole": 0.05, "template": "Loss **{loss}** rose at step 18.",
            "text": "Loss **2.31** rose at step 18.",
            "evidence": {"lines": ["step 18 loss 2.31"], "from": 19, "to": 19, "ident": "1-7", "byteFrom": 812, "byteTo": 830}});
        let flag = super::super::flags::new_flag(&f.store, &row, body, true).unwrap();
        f.store.raise_flags(&[flag], T0 + 19_000).unwrap();
        let (monitor, _) = monitor_at(&f, &run, T0 + 20_500);
        let summary = current(&f.store, &row).unwrap().unwrap().summary().clone();
        let run = json!({
            "status": "running", "createdAt": T0, "startedAt": T0, "updatedAt": T0,
            "endedAt": null, "backend": local(),
        });
        mentions(
            status_line(&monitor, "watch"),
            &["Claude Haiku", "not looked"],
        );
        mentions(
            status_line(&monitor, "flag"),
            &[
                "Loss 2.31 rose at step 18.",
                "log line 19",
                "flag f1",
                "waits for the user",
            ],
        );
        let expect = json!({});
        assert_eq!(summary["flag"]["state"], "waiting");
        json!({"run": run, "now": T0 + 20_500, "monitor": monitor, "summary": summary, "expect": expect})
    }

    fn unreadable_state() -> Value {
        let f = Fixture::new("orx-monitor-contract-unreadable");
        let run = f.run("running", local());
        f.collect(&folded(&declared(0..=20, 400)));
        f.collected(T0 + 20_000);
        monitor_at(&f, &run, T0 + 20_000);
        std::fs::write(&signal_paths(f.dir.path(), "r1")[0], "{not json").unwrap();
        let (mut monitor, _) = monitor_at(&f, &run, T0 + 22_000);
        let path = signal_paths(f.dir.path(), "r1")[0].display().to_string();
        mentions(
            monitor["availability"]["reason"].as_str().unwrap(),
            &[&path],
        );
        mentions(
            monitor["statusText"].as_str().unwrap(),
            &["could not be read", "orx logs r1", &path],
        );
        monitor["availability"]["reason"] = json!("Unreadable checkpoint");
        let mut summary = current(&f.store, &run).unwrap().unwrap().summary().clone();
        assert_eq!(summary["unavailable"], SIGNALS_UNREADABLE);
        summary["unavailable"] = json!("Unreadable checkpoint");
        let run = json!({
            "status": "running", "createdAt": T0, "startedAt": T0, "updatedAt": T0,
            "endedAt": null, "backend": local(),
        });
        let expect = json!({});
        json!({"run": run, "now": T0 + 22_000, "monitor": monitor, "summary": summary, "expect": expect})
    }

    #[test]
    fn a_large_monitor_keeps_the_newest_points_in_each_series() {
        let points: Vec<Value> = (0..500_000).map(|i| json!([i, 1.5])).collect();
        let monitor =
            json!({"rev": 1, "series": {"a": points.clone(), "b": points[..10].to_vec()}});
        let first = bounded(monitor);
        assert!(first.to_string().len() <= MAX_BODY);
        assert_eq!(
            first["series"]["b"].as_array().unwrap().len(),
            10,
            "a short series is whole"
        );
        assert_eq!(
            first["series"]["a"].as_array().unwrap().last(),
            points.last()
        );
    }

    #[test]
    fn a_check_without_patterns_reads_the_checkpoint_not_the_log() {
        let fixture = Fixture::new("check-without-patterns");
        fixture.run("running", local());
        let log = crate::store::log_path(fixture.dir.path(), "r1");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, "::orx:: {\"step\":1,\"m\":{\"loss\":2.5}}\n").unwrap();
        let source = crate::engine::view::Source {
            file: None,
            patterns: None,
            config: None,
        };

        let unfolded =
            crate::engine::view::check(&fixture.store, "r1", None, None, &source).unwrap();

        assert_eq!(unfolded["seen"]["signals"], json!([]));
        fixture.collect(&folded(&[(
            T0,
            "::orx:: {\"step\":1,\"m\":{\"loss\":2.5}}".into(),
        )]));
        let collected =
            crate::engine::view::check(&fixture.store, "r1", None, None, &source).unwrap();
        assert_eq!(collected["seen"]["signals"], json!(["loss"]));
    }

    #[test]
    fn a_run_without_steps_is_compared_with_its_parent_by_elapsed_time_between_its_reports() {
        let f = Fixture::new("orx-rules-parent-elapsed");
        let timed = |id: &str, points: &[(i64, f64)]| {
            let lines: Vec<(i64, String)> = points
                .iter()
                .map(|(t, loss)| {
                    let line = format!("::orx:: {{\"t\":{t},\"m\":{{\"loss\":{loss}}}}}");
                    (T0 + t * 1000, line)
                })
                .collect();
            f.collect(&Snapshot {
                run_id: id.into(),
                ..folded(&lines)
            });
            f.confirmed(id, lines.last().unwrap().0);
        };
        f.run_in("parent", "exp", "done", local());
        timed("parent", &[(0, 1.0), (10, 2.0)]);
        rules_at(&f, "parent", T0 + 11_000);
        f.child("child", "exp");
        ruled(
            &f,
            "r1",
            "child",
            local(),
            json!([{"name": "diverging", "text": "Stop when the loss is more than 1 above the parent's.",
                "does": "stop", "when": {"kind": "worseThanParent", "signal": "loss", "by": 1, "better": "lower"}}]),
        );

        timed("r1", &[(0, 1.0), (5, 2.6)]);
        rules_at(&f, "r1", T0 + 6_000);

        let stop = stopped_by(&f, "r1")
            .expect("2.6 is 1.1 above the parent's 1.5 halfway between its reports");
        mentions(
            stop.reason.as_deref().unwrap(),
            &["loss", "worse", "parent", "2.6", "1.5"],
        );
    }
}
