//! The harness compatibility layer: one `Harness` trait that every coding-agent
//! integration (Claude Code, Codex, OpenCode, Cursor) implements, plus the
//! single `registry()` that every consumer iterates.
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
//!   shim so the agent auto-discovers the CLI. Cursor offers only this.
//!
//! Adding a fourth harness is one new file with one `impl Harness` and one line
//! in `registry()`; the dispatch, the ID list, the detection sweep, and the
//! skill installer all pick it up with no further edits.

pub(crate) mod claude;
pub(crate) mod codex;
mod cursor;
mod detect;
pub(crate) mod opencode;
mod options;
mod plan_gate;
pub(crate) mod title;

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;

use crate::error::Result;
use crate::local::chat::{
    stored_to_wire, DeliveryState, PromptAnswer, ResumeCtx, SteerMessage, SteerReceiver, TurnCtx,
    WirePart, WirePrompt,
};
use crate::store::Store;

pub(crate) use claude::{question_prompt, should_synthesize_plan, synthesize_resume};
pub use detect::{HarnessAuthState, HarnessInfo, ModelInfo};
pub use options::{HarnessOptions, PermissionMode};
pub use plan_gate::command_is_readonly;
pub use plan_gate::decide as plan_gate_decide;

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
const RECOVERY_SNAPSHOT_BYTES: usize = 32 * 1024;

fn newest_utf8_tail(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut start = value.len() - max_bytes;
    while !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
}

fn recovery_part_lines(parts: &[WirePart], lines: &mut Vec<String>) {
    for part in parts {
        match part.kind.as_str() {
            "text" => {
                if let Some(text) = part.text.as_deref().filter(|text| !text.trim().is_empty()) {
                    lines.push(text.trim().to_string());
                }
            }
            "tool" => {
                let state = part.state.as_ref();
                let label = state
                    .and_then(|state| state.title.as_deref())
                    .or(part.tool.as_deref())
                    .unwrap_or("tool");
                let status = state
                    .map(|state| state.status.as_str())
                    .unwrap_or("unknown");
                lines.push(format!("[tool: {label} — {status}]"));
            }
            "prompt" => {
                if let Some(prompt) = part.prompt.as_ref() {
                    let outcome = if prompt.resolved {
                        "resolved"
                    } else {
                        "unresolved"
                    };
                    lines.push(format!("[{} prompt — {outcome}]", prompt.kind));
                }
            }
            _ => {}
        }
        recovery_part_lines(&part.children, lines);
    }
}

pub(crate) fn native_recovery_snapshot(session_id: &str, current_turn_id: &str) -> String {
    let Ok(store) = Store::open() else {
        return String::new();
    };
    let current_user_id = store
        .get_chat_turn(session_id, current_turn_id)
        .ok()
        .flatten()
        .and_then(|turn| turn.user_message_id);
    let Ok(messages) = store.list_chat_messages(session_id) else {
        return String::new();
    };
    let mut entries = Vec::new();
    for message in messages {
        if current_user_id.as_deref() == Some(message.id.as_str()) {
            continue;
        }
        let wire = stored_to_wire(&message);
        let mut lines = Vec::new();
        recovery_part_lines(&wire.parts, &mut lines);
        if !lines.is_empty() {
            entries.push(format!("{}:\n{}", wire.role, lines.join("\n")));
        }
    }
    let mut selected = Vec::new();
    let mut bytes = 0;
    for entry in entries.into_iter().rev() {
        let cost = entry.len() + 2;
        if bytes + cost > RECOVERY_SNAPSHOT_BYTES {
            if selected.is_empty() {
                selected.push(newest_utf8_tail(&entry, RECOVERY_SNAPSHOT_BYTES).to_string());
            }
            break;
        }
        bytes += cost;
        selected.push(entry);
    }
    selected.reverse();
    selected.join("\n\n")
}

pub(crate) fn native_recovery_context(ctx: &TurnCtx, harness: &str) -> Option<String> {
    let snapshot = native_recovery_snapshot(&ctx.session_id, &ctx.turn_id);
    (!snapshot.is_empty()).then(|| {
        format!(
            "<orx-recovery-context>\nThe persisted native {harness} session was unavailable. Use this ORX transcript snapshot as prior context; do not repeat completed tool actions.\n{snapshot}\n</orx-recovery-context>"
        )
    })
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

#[derive(Debug)]
pub struct TurnFailure {
    pub kind: &'static str,
    pub message: String,
    pub delivery: DeliveryState,
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
pub trait Harness: Send + Sync {
    /// Canonical, stable id used on the wire and in the store
    /// (e.g. `"claude-code"`). Must be unique across the registry.
    fn id(&self) -> &'static str;

    /// Human-readable name for UI and prompts (e.g. `"Claude Code"`).
    fn name(&self) -> &'static str;

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

    /// The permission-mode / reasoning-level vocabulary this harness supports,
    /// for the composer toggles. Default is neither control (the UI hides both).
    fn options(&self) -> HarnessOptions {
        HarnessOptions::none()
    }

    /// Generate a short (≤6 words) session title from the session's first user
    /// message by spawning a short-lived headless child pinned to a cheap
    /// configuration. `None` = can't or failed — the caller keeps the
    /// first-line placeholder.
    ///
    /// Default: the shared title one-shot, sanitized; a harness without
    /// `one_shot` (Cursor) gets none. OpenCode also still adopts a native `session.updated` title (minus its creation
    /// seed) through `TurnCtx::set_title` if its server ever offers one, but
    /// runs its own one-shot child since the server stopped titling parent
    /// sessions.
    async fn generate_title(&self, first_message: &str) -> Option<String> {
        let prompt = title::title_prompt(first_message);
        let raw = self.one_shot(title::title_request(&prompt)).await?;
        title::sanitize_title(&raw)
    }

    /// Run one headless request on a throwaway child at the requested
    /// [`OneShotQuality`] and return the model's reply verbatim. The child
    /// cannot write or reach MCP servers (tools are disabled where the CLI
    /// allows), and nothing is recorded in the harness's own session store.
    /// `None` = can't or failed.
    async fn one_shot(&self, _request: OneShot<'_>) -> Option<String> {
        None
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
    /// `None` if this harness has no installable skill.
    fn config_home(&self) -> Option<PathBuf> {
        None
    }

    /// Where the skill shim file lands. `None` if not installable.
    fn skill_target(&self) -> Option<PathBuf> {
        None
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

    /// The agent's global native-skills dir (`~/.claude/skills`,
    /// `~/.agents/skills`, `<xdg>/opencode/skills`) — where the user's own
    /// installed skills live. Derived from the `skills/orx/SKILL.md` shim
    /// target every installable harness shares. `None` if not installable.
    fn global_skills_dir(&self) -> Option<PathBuf> {
        Some(self.skill_target()?.parent()?.parent()?.to_path_buf())
    }

    /// Further skills dirs this agent loads from, each labeled by where it came
    /// from — the plugins installed into this agent. Same shape as
    /// [`global_skills_dir`](Self::global_skills_dir): one skill folder per
    /// entry. Default: none.
    fn plugin_skills_dirs(&self) -> Vec<(String, PathBuf)> {
        Vec::new()
    }

    // --- session-skills capability ----------------------------------------

    /// The worktree-relative dir this harness discovers native `SKILL.md` skill
    /// dirs under, for a **local `orx up` session** — `.claude/skills`,
    /// `.opencode/skills`, `.agents/skills`. The modular `orx` skills are
    /// written there (fresh every turn, beside the playbook) so the session's
    /// own agent auto-loads them. `None` for a harness with no local chat
    /// session that can host per-session skills (Cursor).
    fn session_skills_dir(&self) -> Option<&'static str> {
        None
    }
}

/// Resolve a provider-owned permission id only when that harness advertises it.
/// This is the validation boundary for stored and incoming session values.
pub fn permission_mode_for(harness_id: &str, id: &str) -> Option<PermissionMode> {
    let harness = chat_harness(harness_id)?;
    harness
        .options()
        .permission_modes
        .iter()
        .any(|choice| choice.id == id)
        .then(|| PermissionMode::from_id(id))
        .flatten()
}

/// The valid wire id to expose for a session. Unknown/stale values fall back to
/// the harness default instead of leaving the composer on an impossible mode.
pub fn effective_permission_id(harness_id: &str, stored: Option<&str>) -> Option<String> {
    let harness = chat_harness(harness_id)?;
    let options = harness.options();
    if let Some(id) = stored.filter(|id| options.permission_modes.iter().any(|c| c.id == *id)) {
        return Some(id.to_string());
    }
    options.default_permission_mode.map(str::to_string)
}

/// Validate the processing tiers ORX exposes for Codex.
pub fn service_tier_for<'a>(harness_id: &str, id: &'a str) -> Option<&'a str> {
    (harness_id == "codex" && matches!(id, "default" | "priority")).then_some(id)
}

/// One wait in a harness's turn loop: the harness's own next event, or a
/// message the user steered into the turn meanwhile.
pub(crate) enum Waited<T> {
    Event(T),
    Steer(SteerMessage),
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
    chat_harness(harness_id)?
        .options()
        .permission_modes
        .into_iter()
        .find(|choice| PermissionMode::from_id(&choice.id) == Some(mode))
        .map(|choice| choice.id)
}

/// The one registry. Every consumer — chat dispatch, detection sweep, the
/// create-session allowlist, and the skill installer — iterates this.
pub fn registry() -> Vec<Box<dyn Harness>> {
    vec![
        Box::new(claude::ClaudeCode),
        Box::new(codex::Codex),
        Box::new(opencode::OpenCode),
        Box::new(cursor::Cursor),
    ]
}

/// A single self-contained request for [`Harness::one_shot`].
#[derive(Debug, Clone, Copy)]
pub struct OneShot<'a> {
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

/// How much model to spend on a one-shot: `Cheap` is the smallest/fastest
/// configuration (titles), `Standard` a mid-tier model at modest effort
/// (anything that has to read and reason about a project). Each harness maps
/// these onto its own flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OneShotQuality {
    Cheap,
    Standard,
}

/// The chat-capable harness with this id, if any (used by chat dispatch).
pub fn chat_harness(id: &str) -> Option<Box<dyn Harness>> {
    registry()
        .into_iter()
        .find(|h| h.id() == id && h.supports_chat())
}

/// True if `id` names a chat-capable harness (create-session allowlist).
pub fn is_chat_harness(id: &str) -> bool {
    registry().iter().any(|h| h.id() == id && h.supports_chat())
}

async fn detect_one(harness: &dyn Harness) -> Option<HarnessInfo> {
    harness.detect().await.map(|mut info| {
        if info.auth_state == HarnessAuthState::Unknown {
            info.auth_state = if info.agent_ready {
                HarnessAuthState::Ready
            } else if info.installed && !info.install_broken && info.id != "claude-code" {
                HarnessAuthState::NeedsLogin
            } else {
                HarnessAuthState::Unknown
            };
        }
        info.options = harness.options();
        // The trait is the ceiling: a `detect` narrows it for an installation
        // whose run path can't steer. A steering harness whose `detect` forgets
        // to set it reports false and silently queues every send.
        info.supports_steering &= harness.supports_steering();
        info
    })
}

pub async fn detect_harness(id: &str) -> Option<HarnessInfo> {
    let harness = registry()
        .into_iter()
        .find(|h| h.id() == id && h.supports_chat())?;
    detect_one(harness.as_ref()).await
}

/// Detect every chat-capable harness, in registry order. This is what the
/// `orx up` dashboard renders in its harness picker.
pub async fn detect_harnesses() -> Vec<HarnessInfo> {
    let harnesses: Vec<Box<dyn Harness>> = registry()
        .into_iter()
        .filter(|h| h.supports_chat())
        .collect();
    let futures = harnesses.iter().map(|h| detect_one(h.as_ref()));
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
    crate::local::shell_env::var("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config")
        })
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

    fn permission_contract(o: &HarnessOptions) -> Vec<(&str, &str, &str)> {
        o.permission_modes
            .iter()
            .map(|choice| {
                (
                    choice.id.as_str(),
                    choice.label.as_str(),
                    choice.description.as_deref().unwrap_or_default(),
                )
            })
            .collect()
    }

    #[test]
    fn recovery_snapshot_tail_respects_utf8_byte_limit() {
        let value = format!("old{}new", "🦀".repeat(10));
        let tail = newest_utf8_tail(&value, 12);
        assert!(tail.len() <= 12);
        assert!(tail.ends_with("new"));
        assert!(value.ends_with(tail));
    }

    /// Pin each harness's native permission vocabulary and Plan activation.
    #[test]
    fn advertised_options_per_harness() {
        let claude = options_for("claude-code");
        assert_eq!(
            permission_contract(&claude),
            [
                ("manual", "Manual", "Always ask before making changes"),
                (
                    "acceptEdits",
                    "Accept edits",
                    "Automatically accept all file edits"
                ),
                ("plan", "Plan", "Create a plan before making changes"),
                ("auto", "Auto", "Claude handles permission decisions"),
                (
                    "bypassPermissions",
                    "Bypass permissions",
                    "Accepts all permissions"
                ),
            ]
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
            permission_contract(&codex),
            [
                (
                    "ask",
                    "Ask for approval",
                    "Ask before commands that need elevated access"
                ),
                (
                    "approve-for-me",
                    "Approve for me",
                    "Codex reviews approval requests automatically"
                ),
                (
                    "full-access",
                    "Full access",
                    "Run without sandbox or approval prompts"
                ),
            ]
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
            permission_contract(&opencode),
            [
                (
                    "default",
                    "Default",
                    "Ask before actions that need your approval"
                ),
                (
                    "auto-approve",
                    "Auto-approve",
                    "Approve requests automatically, except actions you have denied"
                ),
            ]
        );
        assert_eq!(opencode.default_permission_mode, Some("default"));
        assert_eq!(opencode.plan_activation, Some(PlanActivation::Command));
        assert!(opencode.reasoning_levels.is_empty());
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
}
