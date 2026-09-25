//! Shared detection primitives for the harness registry — the wire types every
//! harness reports (`HarnessInfo`, `ModelInfo`) and the best-effort probes
//! (`--version`, auth-file reads, JWT decode) the per-harness impls build on.
//!
//! Detection is read-only and best-effort: missing files or unparseable JSON
//! just mean "not detected", never an error.

use std::future::Future;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

pub(super) const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// Bound on simultaneous detection children. Every probe below is a CLI
/// spawn — cheap on Unix, but on Windows each costs seconds, and a fill that
/// fires a dozen at once thrashes (AV scans, node bootstrap) enough that the
/// probes then take *longer* than running a few at a time. A handful of lanes
/// keeps the longest chains overlapped without the measured free-for-all
/// slowdown. `ORX_DETECT_MAX_SPAWNS` overrides the default for tuning.
fn detect_spawn() -> &'static tokio::sync::Semaphore {
    static SPAWN: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    SPAWN.get_or_init(|| {
        let lanes = std::env::var("ORX_DETECT_MAX_SPAWNS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(8);
        tokio::sync::Semaphore::new(lanes)
    })
}

/// A permit bounding simultaneous detection children — see [`detect_spawn`].
/// Acquired *before* a probe's own timeout so queue time never counts as
/// spawn time (which would misreport a queued probe as a hung one).
pub(crate) async fn detect_spawn_permit() -> tokio::sync::SemaphorePermit<'static> {
    detect_spawn()
        .acquire()
        .await
        .expect("detect spawn semaphore never closes")
}

/// Spawn `cmd` on the blocking pool under an already-held `permit`. The permit
/// travels *into* the closure and comes back with the child: a caller dropped
/// while `CreateProcess` is still mid-scan releases its lane when the spawn
/// actually finishes, not when it gives up — the semaphore counts real
/// children, and `kill_on_drop` reaps the process nobody waited for.
pub(crate) async fn spawn_with_permit(
    mut cmd: tokio::process::Command,
    permit: tokio::sync::SemaphorePermit<'static>,
) -> std::io::Result<(tokio::process::Child, tokio::sync::SemaphorePermit<'static>)> {
    cmd.kill_on_drop(true);
    tokio::task::spawn_blocking(move || {
        spawn_retrying_busy(|| cmd.spawn()).map(|child| (child, permit))
    })
    .await?
}

/// On Unix, `execve` reports ETXTBSY while the target inode has an open
/// writer anywhere — a sibling fork still holding an inherited fd, a
/// just-written file, an installer replacing the binary in place. The window
/// is milliseconds; wait out ~0.8s of escalating beats rather than report a
/// healthy binary as broken. Only `ExecutableFileBusy` retries: other kinds
/// are either conclusive (NotFound/PermissionDenied) or mean real resource
/// pressure a tight retry would worsen.
pub(crate) fn spawn_retrying_busy<T>(
    mut spawn: impl FnMut() -> std::io::Result<T>,
) -> std::io::Result<T> {
    let mut backoff = Duration::from_millis(25);
    let mut retries = 5;
    loop {
        match spawn() {
            Err(e) if e.kind() == ErrorKind::ExecutableFileBusy && retries > 0 => {
                retries -= 1;
                std::thread::sleep(backoff);
                backoff *= 2;
            }
            result => return result,
        }
    }
}

/// Run a detection child to completion under a deadline. The permit is
/// acquired before the clock starts — a queued probe reports `None` only when
/// execution itself exceeded `within`, never because it waited for a lane —
/// then [`spawn_with_permit`] runs `CreateProcess` on the blocking pool (on
/// Windows it blocks on the AV scan for seconds) and holds the lane for the
/// child's whole run.
pub(crate) async fn detect_spawn_output_timed(
    mut cmd: tokio::process::Command,
    within: Duration,
) -> Option<std::io::Result<std::process::Output>> {
    let permit = detect_spawn_permit().await;
    // `Command::output` implied these; `spawn` leaves them inherited.
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    tokio::time::timeout(within, async move {
        let (child, _permit) = spawn_with_permit(cmd, permit).await?;
        child.wait_with_output().await
    })
    .await
    .ok()
}

/// Same reasoning as [`spawn_with_permit`], for callers that drive the
/// child's stdio themselves. The returned permit guards the caller's own IO
/// loop — keep it alive until the child is reaped.
pub(crate) async fn detect_spawn_child(
    cmd: tokio::process::Command,
) -> std::io::Result<(tokio::process::Child, tokio::sync::SemaphorePermit<'static>)> {
    let permit = detect_spawn_permit().await;
    spawn_with_permit(cmd, permit).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HarnessAuthState {
    Ready,
    NeedsLogin,
    Unknown,
    Unsupported,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    /// Reasoning/effort choices this *specific* model accepts, led by the
    /// `Default` sentinel. `None` means "this model has no list of its own" —
    /// the composer then falls back to the harness-wide
    /// [`HarnessOptions::reasoning_levels`](super::HarnessOptions).
    ///
    /// `Some(vec![])` is meaningfully different from `None`: it means the model
    /// was *checked* and genuinely exposes no reasoning control (an OpenCode
    /// model with an empty `variants` map), so the picker is hidden entirely
    /// rather than falling back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_levels: Option<Vec<super::options::OptionChoice>>,
    /// The catalog's own human name for the model (`Opus`, `GPT-5.6 Sol`,
    /// `Big Pickle`). Absent for statically-listed fallback models, where the
    /// UI derives a label from the id instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The catalog's one-line blurb — for Claude this is where the resolved
    /// version lives (`Opus 4.8 with 1M context · Best for everyday, complex
    /// tasks`), since its picker aliases (`opus[1m]`) are unversioned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The tier that actually runs when the user picks nothing — set only when
    /// the CLI reports it (codex's `defaultReasoningEffort`, resolved against a
    /// `config.toml` override). When present, `reasoning_levels` carries no
    /// `default` sentinel and the composer preselects this concrete tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_reasoning_level: Option<String>,
    /// Additional processing tiers this model advertises (Codex Fast mode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tiers: Option<Vec<super::options::OptionChoice>>,
}

impl ModelInfo {
    /// A model with no per-model reasoning metadata (falls back to the
    /// harness-wide list).
    pub(super) fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            reasoning_levels: None,
            display_name: None,
            description: None,
            default_reasoning_level: None,
            service_tiers: None,
        }
    }

    /// Attach reasoning choices *with a known concrete default*: no sentinel
    /// row, the default tier preselected instead. For catalogs that report
    /// which tier runs when nothing is chosen (codex).
    pub(super) fn with_reasoning_default(mut self, ids: &[&str], default: &str) -> Self {
        self.reasoning_levels = Some(super::options::reasoning_tiers(ids));
        // A default outside the advertised tiers would be unselectable — leave
        // it unset then, and the composer preselects the first tier.
        self.default_reasoning_level = ids.contains(&default).then(|| default.to_string());
        self
    }

    /// Attach the catalog's display name / description, when it has them.
    pub(super) fn with_label(
        mut self,
        display_name: Option<&str>,
        description: Option<&str>,
    ) -> Self {
        self.display_name = display_name.map(str::to_string);
        self.description = description.map(str::to_string);
        self
    }

    /// Attach this model's own reasoning choices, from native ids. An empty
    /// `ids` yields an empty (not absent) list — "checked, none supported".
    pub(super) fn with_reasoning(mut self, ids: &[&str]) -> Self {
        self.reasoning_levels = Some(if ids.is_empty() {
            Vec::new()
        } else {
            super::options::reasoning_choices(ids)
        });
        self
    }

    pub(super) fn with_service_tiers(mut self, tiers: Vec<super::options::OptionChoice>) -> Self {
        self.service_tiers = Some(tiers);
        self
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub installed: bool,
    /// On PATH, but the binary can't run: `--version` failed conclusively.
    /// Never `agent_ready` — spawning it just dumps its own crash into the chat.
    pub install_broken: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bin_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// A signed-in setup was found (auth file / OAuth account).
    pub authenticated: bool,
    /// Live credential readiness. Account metadata never implies this state.
    pub auth_state: HarnessAuthState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_method: Option<&'static str>, // "oauth" | "apiKey" | "local"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Usable as a chat backend right now, including a reachable local provider.
    pub agent_ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_note: Option<String>,
    /// Setup is blocked by something the harness's own install/login commands
    /// cannot repair — an environment credential that overrides the saved
    /// login, a database the CLI will not open. Signing in again or updating
    /// runs a command that provably cannot help, so the UI shows `agent_note`
    /// as the repair instead of offering one.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub needs_config_repair: bool,
    /// Whether a running turn accepts further user input, which is what lets
    /// the composer steer instead of parking the message until the turn ends.
    pub supports_steering: bool,
    /// Set when a snapshot detection answered from file/discovery evidence
    /// and deferred the expensive probes — model catalog, live auth,
    /// capability checks — to a background full pass that replaces the
    /// entry. Everything on the card is provisional until the flag clears.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub catalog_pending: bool,
    pub models: Vec<ModelInfo>,
    /// Composer toggle vocabulary (permission modes, reasoning levels).
    pub options: super::HarnessOptions,
}

impl HarnessInfo {
    pub(super) fn new(id: &'static str, name: &'static str) -> Self {
        Self {
            id,
            name,
            installed: false,
            install_broken: false,
            bin_path: None,
            version: None,
            authenticated: false,
            auth_state: HarnessAuthState::Unknown,
            auth_method: None,
            account: None,
            org: None,
            plan: None,
            agent_ready: false,
            agent_note: None,
            needs_config_repair: false,
            supports_steering: false,
            catalog_pending: false,
            models: Vec::new(),
            options: super::HarnessOptions::none(),
        }
    }

    /// Attach the chat model list. Each `ModelInfo` carries its own reasoning
    /// choices where the harness knows them (issue #123).
    pub(super) fn with_models(mut self, models: Vec<ModelInfo>) -> Self {
        self.models = models;
        self
    }

    pub(super) fn record_bin(&mut self, bin: &Path, probe: BinProbe) {
        self.installed = true;
        self.bin_path = Some(bin.to_string_lossy().into_owned());
        match probe {
            BinProbe::Answered(version) => self.version = version,
            // The evidence never reaches `agent_note` — that stays a clean
            // "reinstall it" — so log it for the bug report that follows.
            BinProbe::Broken(evidence) => {
                self.install_broken = true;
                eprintln!("orx up: {} --version failed: {evidence}", bin.display());
            }
            BinProbe::Unknown => {}
        }
    }

    pub(super) fn ready(&self) -> bool {
        self.installed && !self.install_broken && self.authenticated
    }

    pub(super) fn broken_note(&self, fix: &str) -> String {
        format!(
            "{} is installed but failed to run. {fix}, then re-check this harness.",
            self.name
        )
    }
}

/// Dereference symlinks to the real installed binary. Installers commonly drop
/// a lone symlink into `~/.local/bin`, but some CLIs locate sibling helper
/// executables relative to the path they were *invoked as*, without resolving
/// symlinks — codex >= 0.144 launches `codex-code-mode-host` this way and every
/// command fails with "No such file or directory" when codex is spawned via the
/// symlink. Spawning the resolved path keeps helpers real siblings. Best-effort:
/// a path that can't be resolved is returned unchanged.
pub(super) fn resolve_symlinks(path: PathBuf) -> PathBuf {
    crate::paths::canonicalize(&path).unwrap_or(path)
}

/// The first candidate that runs, in discovery order, with its `--version`
/// answer. `minimum` is only for a harness that rejects old versions (Claude's
/// OAuth gate); all-broken returns the first, so the caller says
/// installed-but-broken rather than "not found".
pub(super) async fn select_working(
    key: &'static str,
    candidates: Vec<PathBuf>,
    minimum: Option<(u64, u64, u64)>,
) -> Option<(PathBuf, BinProbe)> {
    select_working_with_path(key, candidates, minimum, None).await
}

/// [`select_working`] for a launcher that needs `path` as its PATH to run at
/// all — an npm `.cmd` shim calls `node` from PATH, and the Node.js its
/// installer provisioned is not on the user's PATH.
pub(super) async fn select_working_with_path(
    key: &'static str,
    candidates: Vec<PathBuf>,
    minimum: Option<(u64, u64, u64)>,
    path: Option<std::ffi::OsString>,
) -> Option<(PathBuf, BinProbe)> {
    let selected = select_working_from(candidates, minimum, path.as_deref()).await;
    // The sync `find_*` callers cannot probe; publishing the choice keeps them
    // on the verified binary instead of the first PATH hit it skipped.
    if let Some((path, _)) = &selected {
        remember_selected(key, path);
    }
    selected
}

/// Remember the binary a detection pass picked — see `select_working`.
pub(super) fn remember_selected(key: &'static str, path: &Path) {
    selected_bins()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key, path.to_path_buf());
}

/// The executable the last detection pass selected for `key`, else the first
/// candidate. A selection that no longer exists or has dropped out of discovery
/// is stale and ignored.
pub(super) fn selected_bin(key: &str, candidates: Vec<PathBuf>) -> Option<PathBuf> {
    remembered_bin(key)
        .filter(|path| candidates.contains(path))
        .or_else(|| candidates.into_iter().next())
}

/// The selection the last pass remembered for `key`, if it still exists —
/// without re-running discovery. Lets a later pass trust the pick cheaply when
/// validating it against the candidate list is itself expensive.
pub(super) fn remembered_bin(key: &str) -> Option<PathBuf> {
    selected_bins()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(key)
        .cloned()
        .filter(|path| path.exists())
}

fn selected_bins() -> &'static std::sync::Mutex<std::collections::HashMap<&'static str, PathBuf>> {
    static SELECTED: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<&'static str, PathBuf>>,
    > = std::sync::OnceLock::new();
    SELECTED.get_or_init(Default::default)
}

async fn select_working_from(
    candidates: Vec<PathBuf>,
    minimum: Option<(u64, u64, u64)>,
    path: Option<&std::ffi::OsStr>,
) -> Option<(PathBuf, BinProbe)> {
    // Probe candidates in parallel: a node CLI's `--version` can take seconds
    // (cold start, AV scan). The pick still walks results in discovery order;
    // this trades extra spawns (the old short-circuit probed one binary in
    // the common case) for bounded wall-clock.
    let candidates = unique(candidates);
    let probes =
        futures::future::join_all(candidates.iter().map(|c| probe_bin_with_path(c, path))).await;
    let mut fallback: Option<(PathBuf, BinProbe)> = None;
    for (candidate, probe) in candidates.into_iter().zip(probes) {
        let version = match &probe {
            BinProbe::Answered(version) => version.as_deref().and_then(parse_version),
            // Not a working install: hold the first one only until something answers.
            BinProbe::Unknown | BinProbe::Broken(_) => {
                fallback.get_or_insert((candidate, probe));
                continue;
            }
        };
        // Only a harness with a minimum keeps looking past a binary that ran.
        if minimum.is_some_and(|min| version.is_none_or(|version| version < min)) {
            if !matches!(fallback, Some((_, BinProbe::Answered(_)))) {
                fallback = Some((candidate, probe));
            }
            continue;
        }
        return Some((candidate, probe));
    }
    fallback
}

/// Discovery order, one entry per path. `Vec::dedup` drops only *adjacent*
/// repeats, and on Windows the same binary reached through two PATH spellings
/// (`system32` vs `System32`) is not a repeat at all — the canonicalized key
/// is what dedupes, or one install eats two multi-second `--version` spawns.
pub(crate) fn unique(mut candidates: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|path| {
        seen.insert(crate::paths::canonicalize(path).unwrap_or_else(|_| path.clone()))
    });
    candidates
}

/// Full-pass binary selection overlapped with speculative probes on the
/// binary discovery already prefers (`selected_bin`: the remembered pick,
/// else the first candidate). The catalog/auth children run concurrently
/// with the `--version` sweep instead of behind it — each spawn costs
/// seconds on Windows, and serializing them made the fill the sum of every
/// probe rather than the slowest one. When selection lands on a different
/// binary (a stale first candidate), the probes re-run on the winner; a
/// failed selection discards the speculation.
pub(super) async fn select_and_speculate<F, Fut, Out>(
    key: &'static str,
    candidates: Vec<PathBuf>,
    minimum: Option<(u64, u64, u64)>,
    probes: F,
) -> (Option<(PathBuf, BinProbe)>, Option<Out>)
where
    F: Fn(PathBuf) -> Fut,
    Fut: Future<Output = Out>,
{
    let spec_bin = selected_bin(key, candidates.clone());
    let speculated = spec_bin.clone().map(&probes);
    let (selected, speculated) = tokio::join!(
        timed_probe(key, "select", select_working(key, candidates, minimum)),
        async move {
            match speculated {
                Some(fut) => Some(timed_probe(key, "spec", fut).await),
                None => None,
            }
        },
    );
    match selected {
        Some((bin, probe)) => {
            let hit = spec_bin.as_ref() == Some(&bin);
            if detect_timing() {
                eprintln!(
                    "orx detect: {key} spec {} (spec={:?} selected={:?})",
                    if hit { "hit" } else { "miss" },
                    spec_bin,
                    bin
                );
            }
            let out = match speculated.filter(|_| hit) {
                Some(out) => Some(out),
                None => Some(timed_probe(key, "spec", probes(bin.clone())).await),
            };
            (Some((bin, probe)), out)
        }
        None => (None, None),
    }
}

/// Binary selection shared by every `detect_at`: the snapshot pass answers
/// install from discovery alone — the remembered or first existing candidate —
/// with no `--version` child; the full pass supplies the version and any
/// `Broken` verdict.
pub(super) async fn record_selected(
    info: &mut HarnessInfo,
    snapshot: bool,
    key: &'static str,
    discover: impl FnOnce() -> Option<PathBuf>,
    working: impl Future<Output = Option<(PathBuf, BinProbe)>>,
) {
    let selected = if snapshot {
        discover().map(|bin| (bin, BinProbe::Unknown))
    } else {
        timed_probe(key, "select", working).await
    };
    if let Some((bin, probe)) = selected {
        remember_selected(key, &bin);
        info.record_bin(&bin, probe);
    }
}

/// What `<bin> --version` said about an install found on PATH.
#[derive(Debug)]
pub(super) enum BinProbe {
    /// Ran and named itself — the first line, when it printed one.
    Answered(Option<String>),
    /// Said nothing usable and failed in a way that won't fix itself. Every
    /// healthy CLI answers `--version`, so this means the install is broken.
    /// Carries the CLI's own first complaint, for the log.
    Broken(String),
    /// No evidence either way (a timeout, a transient spawn failure); the
    /// install is left alone and re-probed on the next detection pass.
    Unknown,
}

/// Whether `ORX_DETECT_TIMING` per-probe logging is on.
pub(crate) fn detect_timing() -> bool {
    std::env::var_os("ORX_DETECT_TIMING").is_some()
}

// The timing sink of the catalog fill a probe belongs to, when it runs inside
// one. Scoped by the fill task — one-off detects (`detect_harness`, refresh
// sweeps) run outside it and their timings are never recorded, so a fill's
// analytics payload can only carry its own probes. `tokio::spawn` does not
// inherit task-locals, so speculative probes spawned mid-fill go through
// `spawn_timed_probe`.
tokio::task_local! {
    static DETECT_TIMINGS: std::sync::Arc<std::sync::Mutex<Vec<ProbeTiming>>>;
}

/// A catalog fill's probe-timing collector. Create one per fill, wrap the
/// fill body in [`probe_timing_scope`], and drain it when the fill ends.
pub(crate) type ProbeTimingSink = std::sync::Arc<std::sync::Mutex<Vec<ProbeTiming>>>;

/// Run `fut` with `sink` as the ambient probe-timing collector — every probe
/// the fill awaits (directly or through the detection stream) records into it.
pub(crate) fn probe_timing_scope<F>(
    sink: ProbeTimingSink,
    fut: F,
) -> impl Future<Output = F::Output> + Send
where
    F: Future + Send,
{
    DETECT_TIMINGS.scope(sink, fut)
}

/// Spawn a probe that still belongs to the ambient fill: `tokio::spawn` drops
/// task-locals, so the scope is re-applied inside the spawned task.
pub(crate) fn spawn_timed_probe<F>(
    harness: &'static str,
    probe: &'static str,
    fut: F,
) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let fut = timed_probe(harness, probe, fut);
    match DETECT_TIMINGS.try_with(|sink| sink.clone()) {
        Ok(sink) => tokio::spawn(DETECT_TIMINGS.scope(sink, fut)),
        Err(_) => tokio::spawn(fut),
    }
}

/// One probe's wall clock inside a detection pass, recorded for the
/// `harness_detect_probe` analytics event. `probe` is a fixed label —
/// `"select"`, `"spec"`, `"resolve"`, `"status"`, `"models"`, `"config"`,
/// `"auth+ultracode"`, or `"total"` for the per-harness pass total — never a
/// path or other machine-local value.
pub(crate) struct ProbeTiming {
    pub(crate) harness: &'static str,
    pub(crate) probe: &'static str,
    pub(crate) ms: u64,
}

/// The cap is generous — a real fill records ~15 probes — and only bounds a
/// fill that spawns pathological speculation.
const MAX_PROBE_TIMINGS: usize = 64;

/// Record one probe's wall clock. `timed_probe` calls this for every wrapped
/// probe; `detect_one` uses it for the per-harness `"total"` row. Probes
/// outside a fill scope are not recorded.
pub(crate) fn record_probe_timing(harness: &'static str, probe: &'static str, ms: u64) {
    let Ok(sink) = DETECT_TIMINGS.try_with(|sink| sink.clone()) else {
        return;
    };
    let Ok(mut timings) = sink.lock() else {
        return;
    };
    if timings.len() < MAX_PROBE_TIMINGS {
        timings.push(ProbeTiming { harness, probe, ms });
    }
}

/// Records on drop, so a probe cancelled mid-flight (an aborted speculation)
/// still leaves its consumed wall clock in the fill's telemetry rather than
/// disappearing. The sink is captured when the probe wraps — a dropped future
/// cannot count on the task-local being set during teardown.
struct ProbeTimingGuard {
    harness: &'static str,
    probe: &'static str,
    start: std::time::Instant,
    sink: Option<ProbeTimingSink>,
}

impl ProbeTimingGuard {
    fn new(harness: &'static str, probe: &'static str) -> Self {
        Self {
            harness,
            probe,
            start: std::time::Instant::now(),
            sink: DETECT_TIMINGS.try_with(|sink| sink.clone()).ok(),
        }
    }
}

impl Drop for ProbeTimingGuard {
    fn drop(&mut self) {
        let Some(sink) = &self.sink else {
            return;
        };
        let Ok(mut timings) = sink.lock() else {
            return;
        };
        if timings.len() < MAX_PROBE_TIMINGS {
            timings.push(ProbeTiming {
                harness: self.harness,
                probe: self.probe,
                ms: self.start.elapsed().as_millis() as u64,
            });
        }
    }
}

/// Times a detection probe: records its wall-clock duration — which includes
/// any spawn-lane wait, since the permit is acquired inside `fut` — for the
/// catalog fill's telemetry and, under `ORX_DETECT_TIMING`, logs start and
/// end relative to the first instrumented probe.
pub(crate) async fn timed_probe<T>(
    harness: &'static str,
    probe: &'static str,
    fut: impl Future<Output = T>,
) -> T {
    let timing = detect_timing();
    let start_ms = timing.then(|| detect_epoch().elapsed().as_millis());
    let guard = ProbeTimingGuard::new(harness, probe);
    let out = fut.await;
    if timing {
        eprintln!(
            "orx detect: probe {harness} {probe} took {}ms (start {}ms)",
            guard.start.elapsed().as_millis(),
            start_ms.unwrap_or(0),
        );
    }
    out
}

/// Epoch for `ORX_DETECT_TIMING` start offsets: the first instrumented probe
/// of the process, not process start itself.
fn detect_epoch() -> std::time::Instant {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    *EPOCH.get_or_init(std::time::Instant::now)
}

/// A version recoverable from the install path alone, where packaging bakes
/// it into a directory name — codex's standalone layout is
/// `releases/<semver>-<target-triple>/bin/codex.exe`, so the `<triple>` is
/// what follows the semver. Skipping the `--version` child saves a
/// multi-second spawn on Windows; the trade-off is no exec check, which the
/// speculative probes still supply on authenticated installs.
fn path_version(bin: &Path) -> Option<String> {
    let bin_dir = bin.parent()?.file_name()?.to_str()?;
    if !bin_dir.eq_ignore_ascii_case("bin") {
        return None;
    }
    let release = bin.parent()?.parent()?.file_name()?.to_str()?;
    let (version, _triple) = release.split_once('-')?;
    semver::Version::parse(version).ok()?;
    Some(version.to_string())
}

/// The PE version resource of a binary whose publisher is known to stamp it —
/// claude's native installer writes `FileVersion = 2.1.x.y`, matching what
/// `--version` prints. Reading it needs no child process; on Windows each of
/// those pays a multi-second Defender scan. Deliberately narrow: codex ships
/// no version resource, and opencode's resource describes the embedded Bun
/// runtime, not opencode — those keep their exec check.
#[cfg(windows)]
fn pe_version(bin: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
    };
    if !bin
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.to_ascii_lowercase().starts_with("claude"))
    {
        return None;
    }
    let wide: Vec<u16> = bin.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let size = GetFileVersionInfoSizeW(wide.as_ptr(), std::ptr::null_mut());
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(wide.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
            return None;
        }
        let mut info: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let root: Vec<u16> = "\\".encode_utf16().chain(Some(0)).collect();
        if VerQueryValueW(data.as_ptr().cast(), root.as_ptr(), &mut info, &mut len) == 0
            || info.is_null()
            || (len as usize) < size_of::<VS_FIXEDFILEINFO>()
        {
            return None;
        }
        let ffi = std::ptr::read_unaligned(info as *const VS_FIXEDFILEINFO);
        let ms = ffi.dwFileVersionMS;
        let ls = ffi.dwFileVersionLS;
        Some(format!("{}.{}.{}", ms >> 16, ms & 0xffff, ls >> 16))
    }
}

#[cfg(not(windows))]
fn pe_version(_bin: &Path) -> Option<String> {
    None
}

/// `<bin> --version`, with a timeout (node CLIs can be slow).
pub(super) async fn probe_bin(bin: &Path) -> BinProbe {
    probe_bin_with_path(bin, None).await
}

/// [`probe_bin`] with `path`, when given, as the child's PATH.
async fn probe_bin_with_path(bin: &Path, path: Option<&std::ffi::OsStr>) -> BinProbe {
    if let Some(version) = path_version(bin).or_else(|| pe_version(bin)) {
        return BinProbe::Answered(Some(version));
    }
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("--version").stdin(std::process::Stdio::null());
    // An unparseable version downgrades a signed-in harness to `Unknown` —
    // which is why a synced `FORCE_COLOR` must not reach the version line.
    crate::local::chat::prepare_env(&mut cmd);
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    cmd.env("NO_COLOR", "1");
    let Some(result) = detect_spawn_output_timed(cmd, VERSION_TIMEOUT).await else {
        return BinProbe::Unknown;
    };
    let out = match result {
        Ok(out) => out,
        // Only a stable cause is evidence of a broken install. Fork/descriptor
        // pressure would otherwise tell a user to reinstall three working CLIs.
        Err(error) => {
            return match error.kind() {
                ErrorKind::NotFound | ErrorKind::PermissionDenied => broken(&error.to_string()),
                _ => BinProbe::Unknown,
            }
        }
    };
    // A version has a digit in it; anything else is the CLI complaining, and
    // must not reach the `Version` row as though it were one.
    let version = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| line.chars().any(|c| c.is_ascii_digit()))
        .map(str::to_string);
    // A CLI that named itself works, whatever it did with its exit code.
    if version.is_none() && !out.status.success() {
        return broken(&String::from_utf8_lossy(&out.stderr));
    }
    BinProbe::Answered(version)
}

fn broken(evidence: &str) -> BinProbe {
    BinProbe::Broken(
        evidence
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("no output")
            .to_string(),
    )
}

/// `<bin> --version`, first line — for callers that only want the version and
/// treat every failure the same.
pub(super) async fn bin_version(bin: &Path) -> Option<String> {
    match probe_bin(bin).await {
        BinProbe::Answered(version) => version,
        BinProbe::Broken(_) | BinProbe::Unknown => None,
    }
}

/// An API key from the process env, else orx's own synced env file — the two
/// sources `prepare_env` actually hands the harness child. Detecting only the
/// former would report a working setup as signed out.
pub(super) fn api_key(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .or_else(|| crate::config::synced_env_var(key))
}

pub(super) fn read_json(path: PathBuf) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub(super) fn nonempty_str(v: &Value, key: &str) -> Option<String> {
    v.get(key)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Decode a JWT's payload without verifying — we only surface the account
/// email and plan the user is already signed in as, locally.
pub(super) fn jwt_payload(token: &str) -> Option<Value> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Parse a `major.minor.patch` triple out of a `--version` line. The first
/// whitespace-separated token that parses wins, so `"codex-cli 0.144.0"`,
/// `"2.1.197 (Claude Code)"`, and a bare `"0.144.0"` all resolve; a `-suffix`
/// on the patch is tolerated. `None` when no token has the shape, which each
/// caller treats as "assume the older behaviour".
pub(super) fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    version.split_whitespace().find_map(|token| {
        let mut parts = token.splitn(3, '.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts
            .next()?
            .split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()?;
        Some((major, minor, patch))
    })
}

pub(super) fn title_case(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn resolve_symlinks_dereferences_to_real_binary() {
        let dir = std::env::temp_dir().join(format!("orx-detect-test-{}", std::process::id()));
        let install = dir.join("install");
        let bin = dir.join("bin");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let real = install.join("codex");
        std::fs::write(&real, "").unwrap();
        let link = bin.join("codex");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert_eq!(
            resolve_symlinks(link),
            crate::paths::canonicalize(&real).unwrap()
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_symlinks_keeps_unresolvable_path() {
        let missing = PathBuf::from("/nonexistent/orx-detect-test/codex");
        assert_eq!(resolve_symlinks(missing.clone()), missing);
    }

    #[test]
    fn spawn_retrying_busy_retries_only_busy_errors() {
        // A writer that lets go inside the budget recovers.
        let mut calls = 0;
        let out = spawn_retrying_busy(|| {
            calls += 1;
            if calls < 3 {
                Err(ErrorKind::ExecutableFileBusy.into())
            } else {
                Ok(calls)
            }
        });
        assert!(matches!(out, Ok(3)));

        // One that outlasts it surfaces the last error: 1 try + 5 retries.
        let mut calls = 0;
        let out: std::io::Result<()> = spawn_retrying_busy(|| {
            calls += 1;
            Err(ErrorKind::ExecutableFileBusy.into())
        });
        assert_eq!(calls, 6);
        assert!(matches!(out, Err(e) if e.kind() == ErrorKind::ExecutableFileBusy));

        // Conclusive failures are not retried.
        let mut calls = 0;
        let out: std::io::Result<()> = spawn_retrying_busy(|| {
            calls += 1;
            Err(ErrorKind::PermissionDenied.into())
        });
        assert_eq!(calls, 1);
        assert!(matches!(out, Err(e) if e.kind() == ErrorKind::PermissionDenied));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn probe_bin_reports_broken_only_on_conclusive_evidence() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("orx-probe-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = |name: &str, body: &str| {
            let path = dir.join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };

        let working = script("working", "#!/bin/sh\necho 'codex-cli 0.147.0'\n");
        assert!(matches!(
            probe_bin(&working).await,
            BinProbe::Answered(Some(v)) if v == "codex-cli 0.147.0"
        ));

        // The shape of the reported bug: the wrapper runs, fails to find the
        // binary it wraps, and exits non-zero having printed no version.
        let broken = script("broken", "#!/bin/sh\necho 'spawn ENOENT' >&2\nexit 1\n");
        assert!(matches!(probe_bin(&broken).await, BinProbe::Broken(e) if e == "spawn ENOENT"));
        // Stands in for the shebang whose interpreter is gone — there the spawn
        // itself fails, before the CLI can say anything.
        assert!(matches!(
            probe_bin(&dir.join("absent")).await,
            BinProbe::Broken(_)
        ));

        // Answered, but with nothing usable — still a working install.
        let quiet = script("quiet", "#!/bin/sh\nexit 0\n");
        assert!(matches!(probe_bin(&quiet).await, BinProbe::Answered(None)));

        // A CLI that names itself is working, whatever its exit code says —
        // but a complaint on stdout is not a version.
        let grumpy = script("grumpy", "#!/bin/sh\necho '1.2.3'\necho warn >&2\nexit 1\n");
        assert!(matches!(
            probe_bin(&grumpy).await,
            BinProbe::Answered(Some(v)) if v == "1.2.3"
        ));
        let sulky = script("sulky", "#!/bin/sh\necho 'cannot find module'\nexit 1\n");
        assert!(matches!(probe_bin(&sulky).await, BinProbe::Broken(_)));

        std::fs::remove_dir_all(&dir).ok();
    }

    fn signed_in_with(probe: BinProbe) -> HarnessInfo {
        let mut info = HarnessInfo::new("codex", "Codex");
        info.record_bin(&PathBuf::from("/usr/local/bin/codex"), probe);
        info.authenticated = true;
        info
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn select_working_takes_the_first_healthy_candidate_in_path_order() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("orx-select-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = |name: &str, body: &str| {
            let path = dir.join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };

        // The reported shape: an npm shim first on PATH whose Node is gone,
        // and the healthy native binary the installer just wrote.
        let stale = script("stale", "#!/bin/sh\necho 'spawn ENOENT' >&2\nexit 1\n");
        let fresh = script("fresh", "#!/bin/sh\necho '1.18.31'\n");
        let (picked, _) = select_working_from(vec![stale.clone(), fresh.clone()], None, None)
            .await
            .unwrap();
        assert_eq!(
            picked, fresh,
            "a launcher that cannot run is not an install"
        );

        // PATH order is the user's choice: a working older binary keeps it.
        let old = script("old", "#!/bin/sh\necho '2.1.210 (Claude Code)'\n");
        let new = script("new", "#!/bin/sh\necho '2.1.277 (Claude Code)'\n");
        assert_eq!(
            select_working_from(vec![old.clone(), new.clone()], None, None)
                .await
                .unwrap()
                .0,
            old
        );
        // Only a version the harness actually rejects makes it look further.
        let (picked, probe) =
            select_working_from(vec![old.clone(), new.clone()], Some((2, 1, 211)), None)
                .await
                .unwrap();
        assert_eq!(picked, new);
        assert!(matches!(probe, BinProbe::Answered(Some(v)) if v.starts_with("2.1.277")));
        // No candidate clears the bar: the one that runs is still reported.
        assert_eq!(
            select_working_from(vec![old.clone()], Some((2, 1, 211)), None)
                .await
                .unwrap()
                .0,
            old
        );

        // Every candidate broken still reports an install (the first), so the
        // UI says "installed but failed to run", not "not found".
        let also_stale = script("also-stale", "#!/bin/sh\nexit 1\n");
        let (picked, probe) = select_working_from(vec![stale.clone(), also_stale], None, None)
            .await
            .unwrap();
        assert_eq!(picked, stale);
        assert!(matches!(probe, BinProbe::Broken(_)));

        // Answered-but-unversioned is a working install, and wins outright.
        let quiet = script("quiet", "#!/bin/sh\nexit 0\n");
        assert_eq!(
            select_working_from(vec![quiet.clone(), fresh.clone()], None, None)
                .await
                .unwrap()
                .0,
            quiet
        );
        // ...but it cannot clear a minimum, so the versioned one wins there.
        assert_eq!(
            select_working_from(vec![quiet, fresh.clone()], Some((1, 0, 0)), None)
                .await
                .unwrap()
                .0,
            fresh
        );
        assert!(select_working_from(Vec::new(), None, None).await.is_none());

        // An npm shim runs `node` from PATH: without the installer's Node
        // there it is broken, with it the same launcher answers.
        let node_dir = dir.join("managed-node");
        std::fs::create_dir_all(&node_dir).unwrap();
        let node = node_dir.join("orx-test-node");
        std::fs::write(&node, "#!/bin/sh\necho '0.5.4'\n").unwrap();
        std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();
        let shim = script("shim", "#!/bin/sh\nexec orx-test-node \"$@\"\n");
        assert!(matches!(
            select_working_from(vec![shim.clone()], None, None).await,
            Some((_, BinProbe::Broken(_)))
        ));
        let path = std::env::join_paths([node_dir, "/usr/bin".into(), "/bin".into()]).unwrap();
        assert!(matches!(
            select_working_from(vec![shim], None, Some(&path)).await,
            Some((_, BinProbe::Answered(Some(v)))) if v == "0.5.4"
        ));

        // Non-adjacent repeats are one candidate, so the same binary is never
        // probed twice at `VERSION_TIMEOUT`.
        assert_eq!(
            unique(vec![fresh.clone(), stale.clone(), fresh.clone()]),
            vec![fresh.clone(), stale.clone()]
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn selected_bin_ignores_a_choice_that_left_discovery() {
        let dir = std::env::temp_dir().join(format!("orx-selected-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let chosen = dir.join("chosen");
        std::fs::write(&chosen, "").unwrap();
        let first = dir.join("first");
        std::fs::write(&first, "").unwrap();
        selected_bins()
            .lock()
            .unwrap()
            .insert("test-harness", chosen.clone());

        // Detection's choice beats PATH order for the sync callers.
        let candidates = vec![first.clone(), chosen.clone()];
        assert_eq!(
            selected_bin("test-harness", candidates),
            Some(chosen.clone())
        );
        // Dropped out of discovery, and deleted: both make the choice stale.
        assert_eq!(
            selected_bin("test-harness", vec![first.clone()]),
            Some(first.clone())
        );
        std::fs::remove_file(&chosen).unwrap();
        assert_eq!(
            selected_bin("test-harness", vec![first.clone(), dir.join("chosen")]),
            Some(first)
        );
        assert_eq!(selected_bin("unknown-harness", Vec::new()), None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn record_bin_broken_is_never_ready() {
        let info = signed_in_with(BinProbe::Broken("spawn ENOENT".into()));

        assert!(info.installed, "a broken install is still on PATH");
        assert!(!info.ready(), "a CLI that cannot run cannot run a turn");
        assert!(info
            .broken_note("Reinstall it")
            .starts_with("Codex is installed but failed to run."));
    }

    #[test]
    fn record_bin_answered_keeps_version_and_path() {
        let info = signed_in_with(BinProbe::Answered(Some("0.147.0".into())));

        assert!(info.ready());
        assert_eq!(info.version.as_deref(), Some("0.147.0"));
        assert_eq!(info.bin_path.as_deref(), Some("/usr/local/bin/codex"));
    }

    #[test]
    fn record_bin_unknown_leaves_the_install_alone() {
        let info = signed_in_with(BinProbe::Unknown);

        assert!(info.ready(), "an inconclusive probe must not lock chat out");
        assert!(!info.install_broken);
        assert!(info.version.is_none());
    }
}
