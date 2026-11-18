//! The harness compatibility layer: one `Harness` trait that every coding-agent
//! integration (Claude Code, Codex, OpenCode, Cursor, Antigravity) implements, plus the
//! shared registry for dispatch and discovery.
//!
//! A harness can offer up to three capabilities, and no harness is required to
//! offer all of them:
//!
//! * **detection** (`detect`) — is the CLI installed, is the user signed in,
//!   which account/models. Powers `orx up`'s harness picker.
//! * **chat** (`run_turn`) — drive one chat turn by spawning the CLI and
//!   normalizing its native event stream into wire parts. Detection-only or
//!   install-only harnesses leave this at its default (unsupported).
//! * **skill install** (`skill_target` / `skill_shim`) — drop the `orx` skill
//!   shim so the agent auto-discovers the CLI.
//!
//! Adding a harness is one new file with one `impl Harness` and one line
//! in the registry; the dispatch, the ID list, the detection sweep, and the
//! skill installer all pick it up with no further edits.

pub(crate) mod acp;
pub(crate) mod antigravity;
pub(crate) mod claude;
pub(crate) mod codex;
pub(crate) mod cursor;
mod detect;
pub(crate) mod managed;
pub(crate) mod opencode;
mod options;
mod plan_gate;
mod selection;
pub(crate) mod title;

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;

use crate::error::{anyhow, Error, Result};
use crate::local::chat::{
    DeliveryState, PromptAnswer, ResumeCtx, SteerMessage, SteerReceiver, TurnCtx, WirePrompt,
};
use crate::store::Store;

pub(crate) use claude::{
    question_prompt, should_synthesize_plan, synthesize_resume, SKIPPED_QUESTION_MESSAGE,
};
pub(crate) use detect::{
    detect_spawn_output_timed, probe_timing_scope, spawn_retrying_busy, unique as unique_bins,
    ProbeTiming, ProbeTimingSink,
};
pub use detect::{HarnessAuthState, HarnessInfo, ModelInfo};
pub use options::{HarnessOptions, PermissionMode, REASONING_DEFAULT_ID};
pub use plan_gate::command_is_readonly;
pub use plan_gate::decide as plan_gate_decide;
pub(crate) use selection::{
    catalog_default, catalog_entry, concrete_model, resolve_entry, resolve_model, resolve_models,
    selected_model, ModelSelectionError,
};

/// A turn with NO events for this long is treated as wedged and interrupted
/// rather than held busy forever. Known false positive: a command that is
/// legitimately silent this long (a quiet build, a training step with
/// buffered output) is indistinguishable from a hang — hence the generous
/// bound; the interruption is a clear, recoverable error either way. Shared
/// by the codex and claude adapters (each applies it to its own event wait).
pub(crate) const TURN_WATCHDOG: Duration = Duration::from_secs(30 * 60);

pub(crate) const ORX_MAX_RETRIES: u32 = 3;
pub(crate) const ORX_MAX_ATTEMPTS: u32 = ORX_MAX_RETRIES + 1;
pub(crate) const ORX_RETRY_BUDGET: Duration = Duration::from_secs(15);
pub(crate) fn native_recovery_snapshot(session_id: &str, current_turn_id: &str) -> Result<String> {
    let store = Store::open()?;
    let session = store
        .get_chat_session(session_id)?
        .ok_or_else(|| anyhow!("chat session not found"))?;
    let current_user_id = store
        .get_chat_turn(session_id, current_turn_id)?
        .and_then(|turn| turn.user_message_id);
    let window = session
        .context_usage_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
        .and_then(|usage| usage["contextWindow"].as_u64());
    crate::local::chat::handoff::context(&store, &session, current_user_id.as_deref(), None, window)
}

pub(crate) fn native_recovery_context(ctx: &TurnCtx, harness: &str) -> Result<Option<String>> {
    let snapshot = native_recovery_snapshot(&ctx.session_id, &ctx.turn_id)?;
    Ok((!snapshot.is_empty()).then(|| format!(
        "<orx-recovery-context>\nThe native {harness} session was unavailable. Continue from the saved conversation and inspect completed work before repeating actions.\n{snapshot}\n</orx-recovery-context>"
    )))
}

/// Delay before retry number 1/2/3. Jitter is deterministic per turn and
/// retry number so status rendered before sleeping always matches the sleep.
pub(crate) fn orx_retry_delay(
    turn_id: &str,
    retry_number: u32,
    explicit: Option<Duration>,
) -> Option<Duration> {
    if retry_number == 0 || retry_number > ORX_MAX_RETRIES {
        return None;
    }
    let base = Duration::from_secs(1 << (retry_number - 1));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    turn_id.hash(&mut hasher);
    retry_number.hash(&mut hasher);
    let unit = (hasher.finish() % 10_001) as f64 / 10_000.0;
    let jittered = base.mul_f64(0.75 + unit * 0.5);
    let delay = explicit.unwrap_or(jittered);
    (delay <= ORX_RETRY_BUDGET).then_some(delay)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
}

/// Attach a fixed diagnostic to a setup error without replacing its local detail.
#[derive(Debug)]
pub(crate) struct SetupFailure {
    pub reason: &'static str,
    message: String,
}

impl SetupFailure {
    pub(crate) fn tag(reason: &'static str, error: crate::error::Error) -> crate::error::Error {
        let message = error.to_string();
        error.context(Self { reason, message })
    }
}

impl std::fmt::Display for SetupFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

#[derive(Debug)]
pub struct TurnFailure {
    pub kind: &'static str,
    pub message: String,
    pub delivery: DeliveryState,
}

/// An agent or account refused a turn because it is signed out, which no retry fixes.
#[derive(Debug)]
pub struct SignedOut(pub String);

impl std::fmt::Display for SignedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SignedOut {}

/// Why a turn failed, in terms of what the user does next; the chat shows each its own way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCause {
    SignIn,
    UsageLimit,
    Credits,
    ModelUnavailable,
    ContextFull,
    Overloaded,
    Network,
}

/// What a failed turn's cause is, read from its kind, the `codexErrorInfo` Codex attaches to
/// its message, and the agents' own wording. `None` when nothing says.
pub(crate) fn failure_cause(kind: &str, message: &str) -> Option<FailureCause> {
    match kind {
        "claude_signed_out" => return Some(FailureCause::SignIn),
        "claude_usage_limit" => return Some(FailureCause::UsageLimit),
        "server_overloaded" => return Some(FailureCause::Overloaded),
        _ => {}
    }
    let text = message.to_lowercase();
    let says = |phrases: &[&str]| phrases.iter().any(|phrase| text.contains(phrase));
    // A rejected key is fixed by replacing it; signing in again would change nothing. A
    // provider behind OpenCode or ACP has no sign-in of the agent's own to offer.
    let api_key = says(&[
        "invalid api key",
        "incorrect api key",
        "invalid_api_key",
        "x-api-key",
    ]);
    let signs_in = ["claude_", "codex_", "cursor_"]
        .iter()
        .any(|agent| kind.starts_with(agent));
    if signs_in
        && !api_key
        && says(&[
            "please run /login",
            "oauth token has expired",
            "authentication_error",
            "\"httpstatuscode\":401",
            "401 unauthorized",
            "\"unauthorized\"",
            "sign in again",
            "signing in again",
            "log in again",
            "not logged in",
            "authentication required",
        ])
    {
        Some(FailureCause::SignIn)
    } else if says(&[
        "credit balance is too low",
        "insufficient_quota",
        "insufficient credits",
        "insufficient balance",
    ]) {
        Some(FailureCause::Credits)
    } else if says(&[
        "usagelimitexceeded",
        "hit your usage limit",
        "hit your session limit",
        "usage limit reached",
    ]) {
        Some(FailureCause::UsageLimit)
    } else if says(&[
        "named models unavailable",
        "zcode turn failed: 1113",
        "issue with the selected model",
        "model_not_found",
        "model is not supported",
        "does not have access to model",
    ]) {
        Some(FailureCause::ModelUnavailable)
    } else if says(&[
        "prompt is too long",
        "context_length_exceeded",
        "contextwindowexceeded",
        "maximum context length",
    ]) {
        Some(FailureCause::ContextFull)
    } else if says(&[
        "overloaded_error",
        "serveroverloaded",
        "server is overloaded",
        "currently overloaded",
        "ratelimitexceeded",
        "rate_limit_error",
        "too many requests",
    ]) {
        Some(FailureCause::Overloaded)
    } else if says(&[
        "connection to the api was lost",
        "responsestreamdisconnected",
        "httpconnectionfailed",
        "econnreset",
        "etimedout",
        "enotfound",
        "eai_again",
        "socket hang up",
        "typeerror: fetch failed",
    ]) {
        Some(FailureCause::Network)
    } else {
        None
    }
}

impl TurnFailure {
    pub fn adapter(error: crate::error::Error, delivery: DeliveryState) -> Self {
        Self {
            kind: "adapter_error",
            message: error.to_string(),
            delivery,
        }
    }
}

impl std::fmt::Display for TurnFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TurnFailure {}

pub type TurnResult = std::result::Result<TurnOutcome, TurnFailure>;

/// How an answered interactive prompt flows back into the harness. The two axes
/// a harness can live on:
///
/// * **End-turn-and-resume** (Claude Code): a prompt ends the CLI turn, and the
///   answer becomes a *new user message* that continues the native session via
///   `--resume`. These harnesses return [`ResumeAction::SendMessage`] and
///   `ChatHost` spawns a fresh turn with that text + mode.
/// * **Inline over a live protocol** (OpenCode): the turn is still running,
///   paused on a `permission.asked` / `question.asked` over the serve session;
///   the answer is POSTed back to that live process, which unblocks it. These
///   harnesses perform the reply themselves in `resume_from_prompt` (they own
///   the endpoint shape, and reach their live process through the `ResumeCtx`
///   host handle) and return [`ResumeAction::Handled`] — no new turn to spawn.
///
/// Keeping the decision behind the trait is what lets `ChatHost::respond` stay
/// harness-agnostic (mark resolved, busy-check, broadcast idle) while never
/// routing an inline-approval harness through the new-message resume path.
pub enum ResumeAction {
    /// Resume by sending `text` as a new user message under `mode` (Claude).
    SendMessage {
        text: String,
        mode: Option<PermissionMode>,
        /// Independent Plan transition (Codex); `None` preserves it.
        plan_mode: Option<bool>,
    },
    /// The harness already delivered the answer to its live process (OpenCode
    /// inline reply). `ChatHost` leaves the still-running turn alone — the
    /// paused process resumes and finishes its own turn.
    Handled {
        /// Independent Plan transition applied after the inline reply succeeds
        /// (OpenCode's native `plan_exit` question).
        plan_mode: Option<bool>,
    },
    /// No resume — e.g. a denied permission that just closes the card.
    Nothing,
}

/// One coding-agent integration. See the module docs for the capability model.
#[async_trait]
// async-trait marks its boxed futures `#[must_use]`; clippy 1.99 flags that generated code.
#[allow(clippy::double_must_use)]
pub trait Harness: Send + Sync {
    /// Canonical, stable id used on the wire and in the store
    /// (e.g. `"claude-code"`). Must be unique across the registry.
    fn id(&self) -> &str;

    /// Human-readable name for UI and prompts (e.g. `"Claude Code"`).
    fn name(&self) -> &str;

    // --- chat capability ---------------------------------------------------

    /// Whether this harness can drive chat turns. Gates it out of the chat
    /// picker and the create-session allowlist.
    fn supports_chat(&self) -> bool {
        false
    }

    /// Detect install/auth/account/model state for the `orx up` picker.
    /// `None` means this harness isn't a chat backend and shouldn't appear.
    async fn detect(&self) -> Option<HarnessInfo> {
        None
    }

    /// The fast first pass of [`Harness::detect`]: install, auth, and
    /// `agent_ready` only — no model-catalog or capability probes, which are
    /// the subprocesses that stall cold `/api/harnesses` calls for seconds.
    /// Install/auth come from filesystem and env evidence alone; one
    /// conditional exception is a single auth child where the login may live
    /// in an OS credential store the filesystem cannot see (Claude's
    /// `ProbeCli`). An installed harness's answer is provisional — `detect_one`
    /// marks it `catalog_pending` and clamps readiness — so the caller can
    /// complete it in the background.
    /// Default is the full detect, for harnesses whose auth *is* the catalog
    /// probe (Antigravity) or that have nothing expensive to defer.
    async fn detect_snapshot(&self) -> Option<HarnessInfo> {
        self.detect().await
    }

    /// Run one chat turn: spawn the CLI, parse its event stream, push wire
    /// parts onto `ctx`. Default is "not a chat harness".
    async fn run_turn(&self, _ctx: &mut TurnCtx) -> TurnResult {
        Err(TurnFailure {
            kind: "unsupported",
            message: format!("{} cannot run chat turns", self.id()),
            delivery: DeliveryState::Rejected,
        })
    }

    /// Whether this harness can take user input into a running turn. Gates
    /// whether a turn registers a steering sink at all; `detect` may narrow it
    /// per installation (codex's legacy exec path can't steer).
    fn supports_steering(&self) -> bool {
        false
    }

    /// Whether native compaction or the shared summarize-and-reseed path is supported.
    fn supports_compaction(&self) -> bool {
        false
    }

    /// Compact the native context, or request the shared fallback with `Fallback`.
    async fn compact(&self, _ctx: &CompactCtx) -> Result<CompactOutcome> {
        Ok(CompactOutcome::Fallback)
    }

    /// The permission-mode / reasoning-level vocabulary this harness supports,
    /// for the composer toggles. Default is neither control (the UI hides both).
    fn options(&self) -> HarnessOptions {
        HarnessOptions::none()
    }

    /// Generate a short (1-4 word) session title from the session's first user
    /// message with a short-lived child, using the session model for OpenCode.
    /// `None` = failed; the caller keeps the first-line placeholder.
    ///
    /// Default: the shared title one-shot, sanitized; a harness without
    /// `one_shot` gets none. OpenCode also adopts native session titles through
    /// `TurnCtx::set_title` when its server provides them.
    async fn generate_title(
        &self,
        first_message: &str,
        model: Option<&str>,
        profile: Option<&str>,
    ) -> Option<String> {
        let model = if self.id() == "opencode" {
            Some(model.filter(|model| !model.trim().is_empty())?)
        } else {
            None
        };
        let prompt = title::title_prompt(first_message);
        let request = OneShot {
            profile,
            model,
            ..title::title_request(&prompt)
        };
        let raw = self.one_shot(request).await.ok()?;
        title::sanitize_title(&raw)
    }

    /// Run one headless request on a throwaway child at the requested
    /// [`OneShotQuality`] and return the model's reply verbatim. The child
    /// cannot write or reach MCP servers (tools are disabled where the CLI
    /// allows), and nothing is recorded in the harness's own session store.
    /// The error says why it couldn't run or failed, in the CLI's own words where it gave any.
    async fn one_shot(&self, _request: OneShot<'_>) -> Result<String> {
        Err(anyhow!("{} cannot run one-shot requests", self.name()))
    }

    /// Whether `one_shot` runs on `OneShot::model`; callers caching by model
    /// should not vary the key for a harness that ignores it.
    fn one_shot_honours_model(&self) -> bool {
        true
    }

    /// Decide how an answered prompt flows back, and (for inline harnesses)
    /// deliver it. See [`ResumeAction`] for the two shapes. This runs *before*
    /// `ChatHost::respond` marks the card resolved, so returning an `Err` (e.g.
    /// an unanswerable selection, or a failed inline delivery) leaves the card
    /// actionable and retryable.
    ///
    /// * End-turn harnesses (Claude) build a [`ResumeAction::SendMessage`] and
    ///   let `ChatHost` spawn the follow-up turn.
    /// * Inline harnesses (OpenCode) POST the reply to their live process here,
    ///   reaching it through the `ctx` host handle + native session id, and
    ///   return [`ResumeAction::Handled`].
    ///
    /// The default is [`ResumeAction::Nothing`] — a harness that never emits
    /// prompts never has one to answer.
    async fn resume_from_prompt(
        &self,
        _ctx: &ResumeCtx,
        _prompt: &WirePrompt,
        _answer: &PromptAnswer,
    ) -> Result<ResumeAction> {
        Ok(ResumeAction::Nothing)
    }

    // --- skill-install capability -----------------------------------------

    /// The agent's config home (`~/.claude`, `~/.codex`, `~/.config/opencode`,
    /// `~/.cursor`). Presence of this dir is how we tell the agent is set up.
    fn config_home(&self) -> Option<PathBuf> {
        None
    }

    /// Install the shim in the first native search root. Harnesses may override it.
    fn skill_target(&self) -> Option<PathBuf> {
        Some(
            self.native_skill_roots()
                .first()?
                .join("orx")
                .join("SKILL.md"),
        )
    }

    /// The shim file contents to write at `skill_target`. `None` if not
    /// installable.
    fn skill_shim(&self) -> Option<&'static str> {
        None
    }

    /// Additional `(target, contents)` shim files beyond the primary
    /// [`skill_target`](Self::skill_target) — for a harness that must write more
    /// than one file (Codex writes both the new `~/.agents/skills/orx/SKILL.md`
    /// and the legacy `~/.codex/prompts/orx.md` for older versions). Each is
    /// written and reported alongside the primary target. Default: none.
    fn extra_skill_targets(&self) -> Vec<(PathBuf, &'static str)> {
        Vec::new()
    }

    /// True if the agent looks set up on this machine (its config home exists).
    fn is_installed_locally(&self) -> bool {
        self.config_home().map(|h| h.exists()).unwrap_or(false)
    }

    /// Native skill search roots, in precedence order, independent of session state.
    fn native_skill_roots(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    /// Installed plugin skill roots, labeled with their invocation namespace.
    fn plugin_skills_dirs(&self) -> Vec<(String, PathBuf)> {
        Vec::new()
    }

    // --- session-skills capability ----------------------------------------

    /// The worktree-relative dir this harness discovers native `SKILL.md` skill
    /// dirs under, for a **local `orx up` session** — `.claude/skills`,
    /// `.opencode/skills`, `.agents/skills`, `.cursor/skills`. The modular `orx` skills are
    /// written there (fresh every turn, beside the playbook) so the session's
    /// own agent auto-loads them. `None` for a harness with no local chat
    /// session that can host per-session skills.
    fn session_skills_dir(&self) -> Option<&'static str> {
        None
    }
}

// ACP approval is a client policy, including for chats whose launch was removed.
fn permission_options(harness_id: &str) -> Option<HarnessOptions> {
    if harness_id.starts_with("acp:") {
        Some(acp::approval_options())
    } else {
        Some(chat_harness(harness_id)?.options())
    }
}

/// Resolve a provider-owned permission id only when that harness advertises it.
/// This is the validation boundary for stored and incoming session values.
pub fn permission_mode_for(harness_id: &str, id: &str) -> Option<PermissionMode> {
    permission_options(harness_id)?
        .permission_modes
        .iter()
        .any(|choice| choice.id == id)
        .then(|| PermissionMode::from_id(id))
        .flatten()
}

/// The valid wire id to expose for a session. Unknown/stale values fall back to
/// the harness default instead of leaving the composer on an impossible mode.
pub fn effective_permission_id(harness_id: &str, stored: Option<&str>) -> Option<String> {
    let options = permission_options(harness_id)?;
    if let Some(id) = stored.filter(|id| options.permission_modes.iter().any(|c| c.id == *id)) {
        return Some(id.to_string());
    }
    options.default_permission_mode.map(str::to_string)
}

/// Validate the processing tiers ORX exposes for Codex.
pub fn service_tier_for<'a>(harness_id: &str, id: &'a str) -> Option<&'a str> {
    (harness_id == "codex" && matches!(id, "default" | "priority")).then_some(id)
}

/// One wait in a harness's turn loop: the harness's own next event, a
/// message the user steered into the turn meanwhile, or a held-back flush
/// coming due.
pub(crate) enum Waited<T> {
    Event(T),
    Steer(SteerMessage),
    Flush,
}

/// The next message the user steers into a running turn. A turn without a
/// steering sink parks here forever, so the arm is inert on harnesses that
/// can't steer. `recv` is cancel-safe: a steer that arrives while the event
/// arm wins the select survives to the next iteration.
pub(crate) async fn next_steer(steering: &mut Option<SteerReceiver>) -> SteerMessage {
    match steering {
        Some(rx) => match rx.recv().await {
            Some(message) => message,
            None => std::future::pending().await,
        },
        None => std::future::pending().await,
    }
}

pub fn supports_steering(harness_id: &str) -> bool {
    chat_harness(harness_id).is_some_and(|harness| harness.supports_steering())
}

pub fn supports_command_plan(harness_id: &str) -> bool {
    chat_harness(harness_id).and_then(|harness| harness.options().plan_activation)
        == Some(options::PlanActivation::Command)
}

pub fn permission_id_for_mode(harness_id: &str, mode: PermissionMode) -> Option<String> {
    permission_options(harness_id)?
        .permission_modes
        .into_iter()
        .find(|choice| PermissionMode::from_id(&choice.id) == Some(mode))
        .map(|choice| choice.id)
}

/// Harness registration for dispatch, the session allowlist, and skills.
pub fn registry() -> Vec<Box<dyn Harness>> {
    registry_with_acp(crate::local::acp::registered_definitions())
}

fn registry_with_acp(definitions: Vec<crate::local::acp::Definition>) -> Vec<Box<dyn Harness>> {
    let mut harnesses: Vec<Box<dyn Harness>> = vec![
        Box::new(claude::ClaudeCode),
        Box::new(codex::Codex),
        Box::new(opencode::OpenCode),
        Box::new(cursor::Cursor),
        Box::new(antigravity::Antigravity),
    ];
    harnesses.extend(
        definitions
            .into_iter()
            .map(|definition| Box::new(acp::Acp(definition)) as Box<dyn Harness>),
    );
    harnesses
}

/// A single self-contained request for [`Harness::one_shot`].
#[derive(Debug, Clone, Copy)]
pub struct OneShot<'a> {
    pub profile: Option<&'a str>,
    /// Replaces the harness's agent system prompt where the CLI allows it;
    /// otherwise it is folded into the message.
    pub system: &'a str,
    pub prompt: &'a str,
    pub quality: OneShotQuality,
    /// The model the user has picked for chat, in the harness's own id form.
    /// Codex and OpenCode run on it (their configured default may point at a
    /// provider with no working key); Claude keeps its per-quality alias.
    pub model: Option<&'a str>,
    pub timeout: Duration,
}

/// A failed one-shot child's message from the first of `outputs` it wrote to,
/// on one line so a dumped model catalog stays readable in the log.
pub(crate) fn one_shot_exit_error(status: std::process::ExitStatus, outputs: &[&[u8]]) -> Error {
    outputs
        .iter()
        .map(|bytes| one_line(&String::from_utf8_lossy(bytes), 300))
        .find(|text| !text.is_empty())
        .map_or_else(|| anyhow!("{status}"), |text| anyhow!("{text}"))
}

/// `text` with its whitespace collapsed, capped at `max_chars`.
pub(crate) fn one_line(text: &str, max_chars: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(max_chars) {
        Some((end, _)) => format!("{}…", &flat[..end]),
        None => flat,
    }
}

/// How much model to spend on a one-shot: `Cheap` is the smallest/fastest
/// configuration (titles), `Standard` a mid-tier model at modest effort
/// (anything that has to read and reason about a project). Each harness maps
/// these onto its own flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OneShotQuality {
    Cheap,
    Standard,
}

/// The active conversation and live state, without an in-flight turn.
pub(crate) fn compaction_snapshot(session_id: &str) -> Result<String> {
    native_recovery_snapshot(session_id, "")
}

/// What a session needs to compact, without the machinery of a live turn.
pub struct CompactCtx {
    pub profile: Option<String>,
    pub host: Arc<crate::local::chat::ChatHost>,
    pub session_id: String,
    pub native_session_id: Option<String>,
    pub model: Option<String>,
    pub project: crate::local::model::LocalProject,
    pub permission_mode: Option<PermissionMode>,
    pub plan_mode: bool,
    pub service_tier: Option<String>,
    pub reasoning_level: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CompactOutcome {
    /// The harness compacted its own context; the session id still resolves.
    Native,
    /// The harness has no compaction of its own to offer for this session —
    /// summarize and reseed instead.
    Fallback,
}

/// The chat-capable harness with this id, if any (used by chat dispatch).
pub fn chat_harness(id: &str) -> Option<Box<dyn Harness>> {
    registry()
        .into_iter()
        .find(|h| h.id() == id && h.supports_chat())
}

/// True when the harness's agent is a resident child of the ACP host: user ACP
/// agents and Antigravity (`agy acp`).
pub fn runs_on_acp_host(id: &str) -> bool {
    id.starts_with("acp:") || id == "antigravity"
}

pub fn session_harness(id: &str, session_id: &str) -> Option<Box<dyn Harness>> {
    if id.starts_with("acp:") {
        let state = Store::open()
            .ok()?
            .ensure_acp_session_state(session_id)
            .ok()??;
        return (state.launch.id == id)
            .then(|| Box::new(acp::Acp(state.launch)) as Box<dyn Harness>);
    }
    chat_harness(id)
}

/// True if `id` names a chat-capable harness (create-session allowlist).
pub fn is_chat_harness(id: &str) -> bool {
    registry().iter().any(|h| h.id() == id && h.supports_chat())
}

async fn detect_one(harness: &dyn Harness, snapshot: bool) -> Option<HarnessInfo> {
    let timing = detect::detect_timing();
    let start = std::time::Instant::now();
    let detected = if snapshot {
        harness.detect_snapshot().await
    } else {
        harness.detect().await
    };
    // The per-harness wall clock joins the fill's probe timings as the
    // `"total"` row — the snapshot pass is covered by its own pass event.
    if !snapshot {
        if let Some(telemetry_id) = crate::telemetry::harness::wire_id(harness.id()) {
            detect::record_probe_timing(telemetry_id, "total", start.elapsed().as_millis() as u64);
        }
    }
    if timing {
        eprintln!(
            "orx detect: {} ({}): {}ms",
            harness.id(),
            if snapshot { "snapshot" } else { "full" },
            start.elapsed().as_millis()
        );
    }
    detected.map(|mut info| {
        // A snapshot answer for an installed harness is provisional by
        // definition: its install/auth evidence is file-based and the model
        // catalog is a placeholder until the Full pass lands. Consumers
        // outside onboarding (ModelPicker, ChatPanel, SettingsPage) read
        // `agent_ready` without checking `catalog_pending`, so the
        // pending ⇒ not-ready invariant lives here rather than at every call
        // site — and `supports_steering` goes with it, since a steering claim
        // is only meaningful once the CLI's readiness is verified.
        if snapshot && (info.installed || info.catalog_pending) {
            info.catalog_pending = true;
            info.agent_ready = false;
            info.supports_steering = false;
        }
        if info.auth_state == HarnessAuthState::Unknown && info.agent_ready {
            info.auth_state = HarnessAuthState::Ready;
        }
        info.options = harness.options();
        info.supports_compaction = harness.supports_compaction();
        // The trait is the ceiling: a `detect` narrows it for an installation
        // whose run path can't steer. A steering harness whose `detect` forgets
        // to set it reports false and silently queues every send.
        info.supports_steering &= harness.supports_steering();
        let configured = info.configured_model.take();
        resolve_model(&mut info, configured.as_deref());
        info
    })
}

pub async fn detect_harness(id: &str) -> Option<HarnessInfo> {
    detect_one(chat_harness(id)?.as_ref(), false).await
}

/// The snapshot pass of [`detect_harness`] (see [`Harness::detect_snapshot`]).
pub async fn detect_harness_snapshot(id: &str) -> Option<HarnessInfo> {
    detect_one(chat_harness(id)?.as_ref(), true).await
}

/// The dashboard's detected harness catalog, in registry order.
pub async fn detect_harnesses() -> Vec<HarnessInfo> {
    detect_all(false).await
}

fn catalog_harnesses() -> impl Iterator<Item = Box<dyn Harness>> {
    registry_with_acp(crate::local::acp::catalog_definitions())
        .into_iter()
        .filter(|h| h.supports_chat())
}

/// The full pass, yielding each harness as its own probes finish instead of
/// in a batch. Callers that can commit entries individually unblock a ready
/// agent on its own clock rather than the slowest sibling's — the onboarding
/// gate is per-entry (`agentReady && !catalogPending`), so the catalog fill
/// should not be. Items carry their catalog index so the caller can restore
/// the order for the final payload.
pub fn detect_harnesses_each() -> impl futures::Stream<Item = (usize, HarnessInfo)> {
    catalog_harnesses()
        .enumerate()
        .map(|(i, h)| async move { detect_one(h.as_ref(), false).await.map(|info| (i, info)) })
        .collect::<futures::stream::FuturesUnordered<_>>()
        .filter_map(std::future::ready)
}

/// The snapshot pass of [`detect_harnesses`]: readiness without the model
/// catalogs, so a cold `/api/harnesses` answers in the time the *fastest*
/// probes take instead of the slowest catalog. Entries that still owe a
/// catalog carry `catalog_pending`.
pub async fn detect_harnesses_snapshot() -> Vec<HarnessInfo> {
    detect_all(true).await
}

async fn detect_all(snapshot: bool) -> Vec<HarnessInfo> {
    let futures =
        catalog_harnesses().map(|h| async move { detect_one(h.as_ref(), snapshot).await });
    futures::future::join_all(futures)
        .await
        .into_iter()
        .flatten()
        .collect()
}

/// `$XDG_CONFIG_HOME`, or `~/.config` as the fallback. Mirrors `config::config_dir`
/// — and notably stays XDG even on macOS (OpenCode uses `~/.config/opencode`, not
/// `~/Library/Application Support`). Shared by the harnesses keyed off XDG config.
pub(crate) fn xdg_config_home() -> PathBuf {
    // Ignore an unset *or* empty value — a set-but-empty XDG_CONFIG_HOME would
    // otherwise resolve to a relative `opencode/` path under the cwd.
    crate::config::test_contained(
        crate::local::shell_env::var("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                crate::config::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".config")
            }),
    )
}

// --- skill shims --------------------------------------------------------------
//
// The shim deliberately carries no operating instructions of its own: it just
// tells the agent to run `orx skill` to load the bundled guide. The real guidance
// stays in the CLI's SKILL.md and is fetched fresh each session, so the
// installed shim never drifts as that guide changes — which it does, often.

/// Native `SKILL.md` shim (`skills/orx/SKILL.md`) — Claude Code, OpenCode,
/// Cursor, and now Codex (`~/.agents/skills/orx/`) all read this same format.
/// The frontmatter `description` drives auto-discovery and the `/orx`
/// invocation; the body only points the agent at the bundled guide.
pub(super) const CLAUDE_SKILL: &str = r#"---
name: orx
description: Drive automated ML research on OpenResearch with the `orx` CLI — create experiments, launch and monitor runs on compute, analyze local results and logs, and search literature. Use whenever the user wants to understand, explain, explore, or work on an OpenResearch project, run experiments, do auto-research, or mentions orx or OpenResearch.
---

# OpenResearch (`orx`)

You drive OpenResearch through the `orx` command-line tool. The authoritative
operating manual is bundled inside the CLI, so **load it at the start of every
session** instead of relying on this file or prior memory.

## 1. Load the bundled guide

```bash
orx skill
```

This prints the current manual — the cardinal rules and a command
quick-reference — followed by a **bundled index of modules**. Read it before taking
any action. For the detail on a specific area, run `orx skill <name>` to print
that module (e.g. `orx skill experiment-tree`, `orx skill compute`); the same
command reads that module directly from the installed CLI.

## 2. Carry out the user's research goal

Follow the auto-research loop from the guide: create the baseline experiment
first when the project is empty, branch variants off it, fill the user's available
GPU capacity with useful parallel runs, wait on completions, and analyze each result before deciding
to repair, refill, promote, or stop.

Local research commands do not require an OpenResearch login. If a managed
compute or account command reports `Not logged in`, ask the user to run
`orx login`.
"#;

/// Legacy Codex prompt (`~/.codex/prompts/orx.md`), invoked as `/orx`. Codex now
/// reads native SKILL.md skills (`~/.agents/skills/`, gets `CLAUDE_SKILL`); this
/// prompt is still written alongside for older codex versions that don't.
/// Plain markdown for broad version compatibility; `$ARGUMENTS` is substituted
/// with whatever the user types after the command (and reads fine as-is if their
/// Codex doesn't expand it).
pub(super) const CODEX_PROMPT: &str = r#"Drive automated ML research on OpenResearch using the `orx` CLI.

Start by running `orx skill` to load the current operating manual — the cardinal
rules, a command quick-reference, and a bundled index of modules. Always read it
at the start of a session rather than relying on memory. Pull up a
module's detail with `orx skill <name>` (e.g. `orx skill experiment-tree`,
`orx skill compute`).

Then carry out the user's research goal, following the auto-research loop from that
guide: create the baseline experiment first when the project is empty, branch
variants off it, fill the available GPU capacity with useful parallel runs, wait on completions, and
analyze each result before deciding to repair, refill, promote, or stop.

Local research commands do not require an OpenResearch login. If a managed
compute or account command reports `Not logged in`, ask the user to run
`orx login`.

Research goal:
$ARGUMENTS
"#;

#[cfg(test)]
mod tests {
    use super::options::{PlanActivation, REASONING_DEFAULT_ID};
    use super::*;

    #[test]
    fn skill_install_targets_are_declared_search_roots() {
        for harness in registry() {
            if let Some(target) = harness.skill_target() {
                assert!(
                    harness
                        .native_skill_roots()
                        .iter()
                        .any(|root| target.starts_with(root)),
                    "{} skill target is not discoverable",
                    harness.id()
                );
            }
        }
    }

    #[test]
    fn compaction_support_is_explicit_for_builtins_and_disabled_for_acp() {
        let supported: [&dyn Harness; 5] = [
            &claude::ClaudeCode,
            &codex::Codex,
            &opencode::OpenCode,
            &cursor::Cursor,
            &antigravity::Antigravity,
        ];
        for harness in supported {
            assert!(harness.supports_compaction(), "{}", harness.id());
        }
        for preset in crate::local::acp::presets() {
            let harness = acp::Acp(preset.launch);
            assert!(!harness.supports_compaction(), "{}", harness.id());
        }
        struct DefaultHarness;
        #[async_trait]
        impl Harness for DefaultHarness {
            fn id(&self) -> &str {
                "default-fixture"
            }
            fn name(&self) -> &str {
                "Default fixture"
            }
        }
        assert!(!DefaultHarness.supports_compaction());
    }

    #[tokio::test]
    async fn title_uses_only_the_selected_opencode_model() {
        struct TitleHarness {
            id: &'static str,
            calls: std::sync::Mutex<Vec<Option<String>>>,
        }
        #[async_trait]
        impl Harness for TitleHarness {
            fn id(&self) -> &str {
                self.id
            }
            fn name(&self) -> &str {
                self.id
            }
            async fn one_shot(&self, request: OneShot<'_>) -> Result<String> {
                self.calls
                    .lock()
                    .unwrap()
                    .push(request.model.map(str::to_owned));
                Ok("A test title".into())
            }
        }
        let harness = TitleHarness {
            id: "opencode",
            calls: Default::default(),
        };
        for model in [None, Some(""), Some(" \t")] {
            assert!(harness.generate_title("hello", model, None).await.is_none());
        }
        assert!(harness.calls.lock().unwrap().is_empty());
        for model in [
            "orx-local-test/qwen",
            "openrouter/anthropic/claude",
            "custom/model",
        ] {
            assert_eq!(
                harness
                    .generate_title("hello", Some(model), None)
                    .await
                    .as_deref(),
                Some("A test title")
            );
            assert_eq!(
                harness.calls.lock().unwrap().last(),
                Some(&Some(model.to_owned()))
            );
        }
        for id in ["claude-code", "codex", "cursor"] {
            let harness = TitleHarness {
                id,
                calls: Default::default(),
            };
            harness
                .generate_title("hello", Some("selected/model"), None)
                .await
                .unwrap();
            assert_eq!(*harness.calls.lock().unwrap(), vec![None]);
        }
    }

    #[test]
    fn one_line_flattens_and_caps_on_a_char_boundary() {
        assert_eq!(
            one_line("  Cannot use\n this   model. ", 300),
            "Cannot use this model."
        );
        assert_eq!(one_line("ééééé", 3), "ééé…");
        assert_eq!(one_line("abc", 3), "abc");
    }

    fn options_for(id: &str) -> HarnessOptions {
        registry()
            .into_iter()
            .find(|h| h.id() == id)
            .unwrap_or_else(|| panic!("no harness {id}"))
            .options()
    }

    fn reasoning_ids(o: &HarnessOptions) -> Vec<&str> {
        o.reasoning_levels.iter().map(|c| c.id.as_str()).collect()
    }

    fn permission_ids(o: &HarnessOptions) -> Vec<&str> {
        o.permission_modes.iter().map(|c| c.id.as_str()).collect()
    }

    /// Pin each harness's native permission vocabulary and Plan activation.
    #[test]
    fn advertised_options_per_harness() {
        let claude = options_for("claude-code");
        assert_eq!(
            permission_ids(&claude),
            ["manual", "acceptEdits", "plan", "auto", "bypassPermissions"]
        );
        assert_eq!(claude.default_permission_mode, Some("auto"));
        assert_eq!(claude.plan_activation, Some(PlanActivation::Permission));
        // The harness-wide list is the *fallback* and always leads with
        // `default` (no `--effort` sent). `ultracode` is deliberately absent
        // here — it's version-gated and added per-model in `detect`, where the
        // installed CLI version is known.
        assert_eq!(
            reasoning_ids(&claude),
            ["default", "low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(
            claude.default_reasoning_level.as_deref(),
            Some(REASONING_DEFAULT_ID)
        );

        let codex = options_for("codex");
        assert_eq!(
            permission_ids(&codex),
            ["ask", "approve-for-me", "full-access"]
        );
        assert_eq!(codex.default_permission_mode, Some("approve-for-me"));
        assert_eq!(codex.plan_activation, Some(PlanActivation::Command));
        // Only the conservative fallback intersection — per-model tiers
        // (`max`/`ultra` on Sol/Terra) ride on each `ModelInfo`.
        assert_eq!(
            reasoning_ids(&codex),
            ["default", "low", "medium", "high", "xhigh"]
        );
        assert_eq!(
            codex.default_reasoning_level.as_deref(),
            Some(REASONING_DEFAULT_ID)
        );

        let opencode = options_for("opencode");
        assert_eq!(
            permission_ids(&opencode),
            ["manual", "auto", "auto-approve"]
        );
        assert_eq!(opencode.default_permission_mode, Some("auto-approve"));
        assert_eq!(opencode.plan_activation, Some(PlanActivation::Command));
        assert!(opencode.reasoning_levels.is_empty());

        let cursor = options_for("cursor");
        assert_eq!(permission_ids(&cursor), ["ask", "auto", "full-access"]);
        assert_eq!(cursor.default_permission_mode, Some("auto"));
        assert_eq!(cursor.plan_activation, Some(PlanActivation::Command));
        assert!(cursor.reasoning_levels.is_empty());

        let antigravity = options_for("antigravity");
        assert_eq!(permission_ids(&antigravity), ["default", "bypass"]);
        assert_eq!(antigravity.default_permission_mode, Some("bypass"));
        assert_eq!(antigravity.plan_activation, Some(PlanActivation::Command));
        assert!(reasoning_ids(&antigravity).is_empty());
        assert!(antigravity.default_reasoning_level.is_none());
    }

    #[test]
    fn removed_acp_launches_keep_client_approval_controls() {
        let id = format!("acp:{}", uuid::Uuid::new_v4());
        assert!(chat_harness(&id).is_none());
        assert!(!is_chat_harness(&id));
        assert_eq!(permission_mode_for(&id, "ask"), Some(PermissionMode::Ask));
        assert_eq!(
            permission_mode_for(&id, "auto-approve"),
            Some(PermissionMode::Bypass)
        );
        assert_eq!(permission_mode_for(&id, "full-access"), None);
        assert_eq!(
            effective_permission_id(&id, Some("auto-approve")).as_deref(),
            Some("auto-approve")
        );
        assert_eq!(effective_permission_id(&id, None).as_deref(), Some("ask"));
        assert_eq!(
            permission_id_for_mode(&id, PermissionMode::Ask).as_deref(),
            Some("ask")
        );
    }

    /// Every advertised permission-mode id must round-trip through
    /// `PermissionMode::from_id` — i.e. a harness never advertises an id the
    /// backend can't parse back when the session sends it.
    #[test]
    fn advertised_permission_ids_all_parse() {
        for h in registry() {
            for choice in h.options().permission_modes {
                let mode = PermissionMode::from_id(&choice.id);
                assert!(
                    mode.is_some(),
                    "{} advertises unparseable mode {:?}",
                    h.id(),
                    choice.id
                );
                assert_eq!(
                    permission_id_for_mode(h.id(), mode.unwrap()).as_deref(),
                    Some(choice.id.as_str()),
                    "{} permission id does not round-trip",
                    h.id()
                );
            }
        }
    }

    /// OpenCode's "Auto-approve" shipped as the id `auto-approve`; chats saved on it
    /// are Full access now and must not fall back to asking.
    #[test]
    fn a_chat_saved_on_opencodes_auto_approve_stays_on_full_access() {
        assert_eq!(
            effective_permission_id("opencode", Some("auto-approve")).as_deref(),
            Some("auto-approve")
        );
        assert_eq!(
            permission_mode_for("opencode", "auto-approve"),
            Some(PermissionMode::Bypass)
        );
    }

    #[test]
    fn orx_retry_policy_is_bounded_and_jittered() {
        for retry in 1..=ORX_MAX_RETRIES {
            let delay = orx_retry_delay("turn-a", retry, None).unwrap();
            let base = Duration::from_secs(1 << (retry - 1));
            assert!(delay >= base.mul_f64(0.75));
            assert!(delay <= base.mul_f64(1.25));
            assert_eq!(delay, orx_retry_delay("turn-a", retry, None).unwrap());
        }
        assert!(orx_retry_delay("turn-a", 0, None).is_none());
        assert!(orx_retry_delay("turn-a", ORX_MAX_RETRIES + 1, None).is_none());
        assert!(orx_retry_delay(
            "turn-a",
            1,
            Some(ORX_RETRY_BUDGET + Duration::from_millis(1))
        )
        .is_none());
    }

    #[test]
    fn a_failed_turn_names_the_cause_its_agent_reported() {
        use FailureCause::*;
        let codex_401 = "unexpected status 401 Unauthorized: Missing bearer or basic authentication in header, url: https://api.openai.com/v1/responses\n\ncodexErrorInfo: {\"httpConnectionFailed\":{\"httpStatusCode\":401}}";
        for (kind, message, cause) in [
            ("claude_signed_out", "Sign in with `claude auth login`, then re-check this agent.", Some(SignIn)),
            ("claude_terminal", "claude: Not logged in · Please run /login", Some(SignIn)),
            ("claude_terminal", "claude: OAuth token has expired. Please obtain a new token or refresh your existing token.", Some(SignIn)),
            ("codex_terminal", codex_401, Some(SignIn)),
            ("codex_terminal", "Your access token could not be refreshed because your refresh token was already used. Please log out and sign in again.", Some(SignIn)),
            ("cursor_terminal", "Authentication required. Please run 'agent login' first, or set CURSOR_API_KEY environment variable.", Some(SignIn)),
            ("claude_usage_limit", "You've hit your session limit · resets 3pm", Some(UsageLimit)),
            ("codex_terminal", "You've hit your usage limit. Try again at 3:00 PM.\n\ncodexErrorInfo: \"usageLimitExceeded\"", Some(UsageLimit)),
            ("claude_terminal", "claude: Credit balance is too low", Some(Credits)),
            ("opencode_terminal", "You exceeded your current quota, please check your plan and billing details. insufficient_quota", Some(Credits)),
            ("claude_terminal", "claude: There's an issue with the selected model (claude-x). It may not exist or you may not have access to it. Run /model to pick a different model.", Some(ModelUnavailable)),
            ("claude_terminal", "claude: Prompt is too long", Some(ContextFull)),
            ("codex_terminal", "Codex ran out of room in the model's context window.\n\ncodexErrorInfo: \"contextWindowExceeded\"", Some(ContextFull)),
            ("claude_terminal", "claude: API Error: 529 {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}", Some(Overloaded)),
            ("server_overloaded", "Codex is at capacity", Some(Overloaded)),
            ("codex_terminal", "stream disconnected before completion\n\ncodexErrorInfo: {\"responseStreamDisconnected\":{\"httpStatusCode\":null}}", Some(Network)),
            ("claude_terminal", "claude: Invalid API key · Please run /login", None),
            ("codex_terminal", "unexpected status 401 Unauthorized: Incorrect API key provided: sk-abc", None),
            ("opencode_terminal", "401 Unauthorized: authentication_error", None),
            ("claude_terminal", "claude: API Error: 401 {\"type\":\"error\",\"error\":{\"type\":\"authentication_error\",\"message\":\"invalid x-api-key\"}}", None),
            ("claude_terminal", "claude: The tool output was overloaded with rows, so it was truncated", None),
            ("claude_setup", "Claude Code 2.1.211 or newer is required", None),
            ("adapter_error", "claude exited mid-turn; see /root/.local/share/openresearch/agent-claude.log", None),
        ] {
            assert_eq!(failure_cause(kind, message), cause, "{kind}: {message}");
        }
    }
}
