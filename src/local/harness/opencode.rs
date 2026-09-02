//! OpenCode harness.
//!
//! Chat: talks to a lazily spawned `opencode serve` child (the `AgentHost` the
//! up server shares). serve is opencode's first-party embedding surface; HTTP
//! on loopback is just this adapter's transport, never exposed to the browser.
//! A turn = subscribe to the global `/event` SSE stream, POST the message
//! (which resolves when the turn ends), and translate this session's part
//! events into wire parts as they stream.
//!
//! Interactive prompts: unlike Claude (which ends its turn and resumes with a
//! new message), opencode approves *inline*. Its serve stream emits
//! `permission.asked` / `question.asked` while the `session.prompt` POST is
//! still open — the turn is paused, not finished. We surface those as
//! `permission` / `question` cards and reply over the live session
//! (`resume_from_prompt` → [`ResumeAction::Handled`]), which unblocks the same
//! POST. Auto-approve resolves native `ask` requests without a card; Default
//! surfaces them. Questions always need a human, so they always surface.
//!
//! Detection: opencode's `auth.json` is `{provider: {type}}`; the signed-in
//! providers are its account line, and `opencode models --verbose` is the model
//! list plus each model's reasoning `variants` (plain `opencode models` is the
//! fallback for a CLI too old for `--verbose`).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};

use super::detect::{probe_bin, read_json, HarnessInfo};
use super::options::{
    HarnessOptions, OptionChoice, PermissionMode, PlanActivation, REASONING_DEFAULT_ID,
};
use super::{
    Harness, OneShot, ResumeAction, TurnFailure, TurnOutcome, TurnResult, ORX_MAX_ATTEMPTS,
};
use crate::error::{anyhow, Result};
use crate::local::chat::{
    ContextUsage, DeliveryState, PromptAnswer, ResumeCtx, TurnCtx, WirePart, WirePrompt,
    WireQuestionOption, WireToolState,
};
use crate::local::native_store::{self, NativeStore};
use crate::local::opencode::find_opencode;

const OPENCODE_REINSTALL: &str =
    "Reinstall opencode (curl -fsSL https://opencode.ai/install | bash)";

pub struct OpenCode;

#[async_trait]
impl Harness for OpenCode {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn name(&self) -> &'static str {
        "OpenCode"
    }

    fn supports_chat(&self) -> bool {
        true
    }

    async fn one_shot(&self, request: OneShot<'_>) -> Option<String> {
        opencode_one_shot(&find_opencode().ok()?, request).await
    }

    async fn detect(&self) -> Option<HarnessInfo> {
        let mut info = HarnessInfo::new(self.id(), self.name());
        let mut models = Vec::new();
        if let Ok(bin) = find_opencode() {
            info.record_bin(&bin, probe_bin(&bin).await);
            // A binary that failed `--version` has no catalog to give either.
            if !info.install_broken {
                models = opencode_models(&bin).await;
            }
        }
        let providers = opencode_providers();
        if !providers.is_empty() {
            info.authenticated = true;
            info.auth_method = Some("oauth");
            info.account = Some(providers.join(", "));
        }
        // opencode also takes provider keys straight from the environment,
        // writing no auth.json — same fallback claude.rs has. Checked against
        // orx's synced env too, since that's a source the harness child gets
        // but this process may not. Measured, not assumed: `opencode models`
        // still lists free/bundled models when signed out, so a non-empty
        // model list can't stand in for a credential.
        const PROVIDER_KEYS: &[&str] = &[
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "OPENROUTER_API_KEY",
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
            "GROQ_API_KEY",
            "XAI_API_KEY",
            "DEEPSEEK_API_KEY",
        ];
        if !info.authenticated
            && PROVIDER_KEYS
                .iter()
                .any(|k| super::detect::api_key(k).is_some())
        {
            info.authenticated = true;
            info.auth_method = Some("apiKey");
        }

        // Tightened from `installed` alone — opencode with no credential can't
        // actually run a turn, and it was the one harness reporting Connected
        // regardless. Behaviour change on upgrade: an install with neither
        // auth.json nor a provider key above now reads "Not signed in", and
        // since step 1 of onboarding gates on this, an opencode-only user is
        // asked to sign in before continuing.
        info.agent_ready = info.ready();
        if info.agent_ready {
            // A stored key can be revoked or expired: opencode still lists the
            // provider's models and would fail every turn on them. Probe each
            // signed-in provider once (per auth.json version) and hide the
            // models of any that reject its key.
            let dead = if let Some(bin) = info.bin_path.as_deref().map(PathBuf::from) {
                dead_providers(&bin, &providers, &models).await
            } else {
                Vec::new()
            };
            if !dead.is_empty() {
                models.retain(|model| !dead.iter().any(|p| model_provider(&model.id) == p));
                info.account = Some(
                    providers
                        .iter()
                        .map(|p| {
                            if dead.contains(p) {
                                format!("{p} (key rejected)")
                            } else {
                                p.clone()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                info.agent_note = Some(format!(
                    "{} rejected the stored API key, so its models are hidden. Re-add it with `opencode auth login`.",
                    dead.join(", ")
                ));
            }
            info.models = models;
        } else if info.install_broken {
            info.agent_note = Some(info.broken_note(OPENCODE_REINSTALL));
        } else if info.installed {
            info.agent_note =
                Some("Sign in with `opencode auth login` to chat with it here.".to_string());
        } else {
            info.agent_note = Some(
                "Install opencode (curl -fsSL https://opencode.ai/install | bash), then sign in with `opencode auth login`."
                    .to_string(),
            );
        }
        Some(info)
    }

    async fn run_turn(&self, ctx: &mut TurnCtx) -> TurnResult {
        run_turn(ctx)
            .await
            .map(|()| TurnOutcome::Completed)
            .map_err(|error| TurnFailure::adapter(error, ctx.delivery_state()))
    }

    fn options(&self) -> HarnessOptions {
        // OpenCode's agent (plan/build) is independent of permission handling.
        // Default honors configured allow/ask/deny rules; Auto-approve answers
        // only native `ask` requests and never overrides explicit denies.
        // Reasoning IS a model property in opencode, so there is no meaningful
        // harness-wide list: the real choices are each model's `variants`, read
        // from `opencode models --verbose` in `detect` and attached per-model.
        // Leaving this axis empty means a model with no variants shows no
        // picker at all, rather than falling back to a bogus union.
        HarnessOptions::none().with_permission_choices(
            vec![
                OptionChoice::described(
                    "default",
                    "Default",
                    "Ask before actions that need your approval",
                ),
                OptionChoice::described(
                    "auto-approve",
                    "Auto-approve",
                    "Approve requests automatically, except actions you have denied",
                ),
            ],
            "default",
            PlanActivation::Command,
        )
    }

    /// opencode is paused mid-turn on a `permission.asked` / `question.asked`;
    /// the answer is replied over the live serve session, which unblocks the
    /// still-open `session.prompt` POST. So this delivers the reply inline and
    /// returns [`ResumeAction::Handled`] — never the new-message path.
    async fn resume_from_prompt(
        &self,
        ctx: &ResumeCtx,
        prompt: &WirePrompt,
        answer: &PromptAnswer,
    ) -> Result<ResumeAction> {
        let plan_mode = plan_exit_transition(prompt, answer);
        if let Some(plan_mode) = plan_mode {
            // Persist the native answer's Plan transition before OpenCode
            // consumes it. If delivery fails, restore the active Plan state so
            // the still-actionable card and ORX continue to agree.
            ctx.host.set_plan_mode(&ctx.session_id, plan_mode).await?;
            if let Err(err) = reply_inline(ctx, prompt, answer).await {
                let _ = ctx.host.set_plan_mode(&ctx.session_id, true).await;
                return Err(err);
            }
            return Ok(ResumeAction::Handled { plan_mode: None });
        }
        reply_inline(ctx, prompt, answer).await?;
        Ok(ResumeAction::Handled { plan_mode: None })
    }

    fn config_home(&self) -> Option<PathBuf> {
        // OpenCode discovers skills under XDG config, staying XDG even on macOS.
        Some(super::xdg_config_home().join("opencode"))
    }

    fn skill_target(&self) -> Option<PathBuf> {
        Some(
            self.config_home()?
                .join("skills")
                .join("orx")
                .join("SKILL.md"),
        )
    }

    fn skill_shim(&self) -> Option<&'static str> {
        // OpenCode reads the same SKILL.md format as Claude Code.
        Some(super::CLAUDE_SKILL)
    }

    fn session_skills_dir(&self) -> Option<&'static str> {
        Some(".opencode/skills")
    }
}

fn opencode_auth_path() -> Option<PathBuf> {
    let base = crate::local::shell_env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("share")))?;
    Some(base.join("opencode").join("auth.json"))
}

/// Providers opencode is signed into (its auth.json is `{provider: {type}}`).
fn opencode_providers() -> Vec<String> {
    let Some(auth) = opencode_auth_path().and_then(read_json) else {
        return Vec::new();
    };
    match auth.as_object() {
        Some(map) => map.keys().cloned().collect(),
        None => Vec::new(),
    }
}

/// Providers whose stored key a live request rejects. One tiny read-only
/// request per provider on its first catalogued model, remembered per
/// auth.json version so a paid probe runs once, not once per detection.
/// Failures that are not an auth rejection (network, timeout, no model)
/// leave the provider alone.
async fn dead_providers(
    bin: &PathBuf,
    providers: &[String],
    models: &[super::ModelInfo],
) -> Vec<String> {
    static VERDICTS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, bool>>> =
        std::sync::OnceLock::new();
    let verdicts = VERDICTS.get_or_init(Default::default);
    let version = opencode_auth_path()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let probes = providers.iter().filter_map(|provider| {
        let key = format!("{version}:{provider}");
        let cached = verdicts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .copied();
        let model = models
            .iter()
            .find(|model| model_provider(&model.id) == provider)?
            .id
            .clone();
        Some(async move {
            let dead = match cached {
                Some(dead) => dead,
                None => {
                    let dead = probe_rejects_key(bin, &model).await;
                    verdicts
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(key, dead);
                    dead
                }
            };
            dead.then(|| provider.clone())
        })
    });
    futures::future::join_all(probes)
        .await
        .into_iter()
        .flatten()
        .collect()
}

/// True only when the provider answered with an authentication error; any
/// other outcome (a reply, a timeout, a network failure) is not a dead key.
async fn probe_rejects_key(bin: &PathBuf, model: &str) -> bool {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args([
        "run",
        "--agent",
        "plan",
        "--pure",
        "--model",
        model,
        "Reply with the single word ok",
    ])
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .kill_on_drop(true)
    .current_dir(std::env::temp_dir());
    crate::local::chat::prepare_env(&mut cmd);
    if let Ok(db) = native_store::prepare_opencode(NativeStore::Isolated) {
        cmd.env("OPENCODE_DB", db);
    }
    cmd.env("NO_COLOR", "1");
    let Ok(Ok(out)) = tokio::time::timeout(KEY_PROBE_TIMEOUT, cmd.output()).await else {
        return false;
    };
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    !out.status.success() && is_auth_rejection(&text)
}

const KEY_PROBE_TIMEOUT: Duration = Duration::from_secs(25);

/// Phrases the major providers use when a key is invalid, revoked, or expired.
const AUTH_REJECTION_MARKERS: &[&str] = &[
    "api key not valid",
    "invalid api key",
    "incorrect api key",
    "invalid x-api-key",
    "invalid_api_key",
    "authentication_error",
    "authentication failed",
    "unauthorized",
    "401",
    "key has expired",
    "api key expired",
    "no auth credentials",
];

fn is_auth_rejection(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    AUTH_REJECTION_MARKERS.iter().any(|m| lower.contains(m))
}

/// The `provider` half of an opencode `provider/model` id.
fn model_provider(id: &str) -> &str {
    id.split_once('/').map(|(p, _)| p).unwrap_or(id)
}

/// `opencode models --verbose` — the ground truth for what the agent can run
/// *and* for each model's reasoning `variants`.
///
/// `--verbose` prints, per model, a `provider/model` header line followed by a
/// pretty-printed JSON object. We parse it for the `variants` map because
/// reasoning in opencode is a genuine per-model property (issue #123):
/// `gemini-3-flash` offers `minimal…high`, `deepseek-v4-flash` offers
/// `low…max`, and plenty of models offer none at all.
///
/// Falls back to the plain `opencode models` id list if `--verbose` is
/// unavailable or unparseable, so an older/newer opencode still yields models
/// (just without per-model variants).
async fn opencode_models(bin: &PathBuf) -> Vec<super::ModelInfo> {
    let verbose = run_models(bin, &["models", "--verbose"]).await;
    if let Some(out) = &verbose {
        let parsed = parse_verbose_models(out);
        if !parsed.is_empty() {
            return parsed;
        }
    }
    let Some(plain) = run_models(bin, &["models"]).await else {
        return Vec::new();
    };
    model_id_lines(&plain).map(super::ModelInfo::new).collect()
}

/// One headless request on a throwaway `opencode run` child on
/// `request.model`, else the user's default model. opencode's server no longer retitles parent sessions itself
/// (only sub-agent child sessions get task-description titles), so titles run
/// through here like the claude/codex one-shot children. opencode has no
/// system-prompt flag, so `system` leads the message. Any failure lands on
/// `None` and the caller keeps its fallback.
async fn opencode_one_shot(bin: &PathBuf, request: OneShot<'_>) -> Option<String> {
    let message = format!("{}\n\n{}", request.system, request.prompt);
    let mut cmd = tokio::process::Command::new(bin);
    // The request embeds user-controlled text, so the child must not be able
    // to act on it: the built-in read-only `plan` agent denies writes,
    // `--pure` skips external plugins, and the temp cwd keeps any residual
    // reads away from real repos. A tool call that still asks for permission
    // just blocks the unattended child until the timeout kills it.
    cmd.args(["run", "--agent", "plan", "--pure"])
        .args(request.model.iter().flat_map(|model| ["--model", model]))
        .arg(&message)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        // Outside any repository, so the child doesn't ingest the server cwd's
        // AGENTS.md into a request that carries its own context.
        .current_dir(std::env::temp_dir());
    crate::local::chat::prepare_env(&mut cmd);
    cmd.env(
        "OPENCODE_DB",
        native_store::prepare_opencode(NativeStore::Isolated).ok()?,
    );
    // Plain text only — an ANSI-colorizing CLI (or a synced FORCE_COLOR) would
    // otherwise write escape codes straight into the reply.
    cmd.env("NO_COLOR", "1");
    let out = tokio::time::timeout(request.timeout, cmd.output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run `opencode <args>` in the home dir, returning stdout on success.
async fn run_models(bin: &PathBuf, args: &[&str]) -> Option<String> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args)
        .current_dir(dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .stdin(std::process::Stdio::null());
    crate::local::chat::prepare_env(&mut cmd);
    let fut = cmd.output();
    let Ok(Ok(out)) = tokio::time::timeout(Duration::from_secs(20), fut).await else {
        return None;
    };
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The bare `provider/model` id lines of plain `opencode models` output.
fn model_id_lines(out: &str) -> impl Iterator<Item = &str> {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && l.contains('/'))
}

/// Parse `opencode models --verbose` into models + their variant ids.
///
/// The format is a repeating `header line` + `{ … }` JSON block. We walk lines,
/// treat any non-`{`-starting line containing `/` as a header, and accumulate
/// the following block until braces balance — brace counting (rather than
/// "next header") keeps a `}` inside a nested object from ending the block
/// early.
///
/// The counter skips braces inside JSON string literals. That is not
/// hypothetical tidiness: a single `{` in any free-text field (a model `name`
/// or description) would otherwise desynchronize the depth, and since it can
/// never balance again the loop would swallow the entire rest of the output —
/// dropping every later model, and quietly, because a partial parse doesn't
/// trigger the plain-list fallback.
fn parse_verbose_models(out: &str) -> Vec<super::ModelInfo> {
    let mut models = Vec::new();
    let mut lines = out.lines().peekable();
    while let Some(line) = lines.next() {
        let header = line.trim();
        if header.is_empty() || !header.contains('/') || header.starts_with('{') {
            continue;
        }
        if !lines
            .peek()
            .is_some_and(|l| l.trim_start().starts_with('{'))
        {
            continue;
        }
        let mut block = String::new();
        let mut depth = 0usize;
        let mut in_str = false;
        let mut esc = false;
        for body in lines.by_ref() {
            for ch in body.chars() {
                match ch {
                    _ if esc => esc = false,
                    '\\' if in_str => esc = true,
                    '"' => in_str = !in_str,
                    '{' if !in_str => depth += 1,
                    '}' if !in_str => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
            // Neither a string literal nor an escape spans lines in this
            // output, so reset both: an unterminated quote would otherwise
            // invert `in_str` for every following line, stop brace counting
            // entirely, and swallow the rest of the output — the same silent
            // model-dropping failure the string tracking exists to prevent.
            esc = false;
            in_str = false;
            block.push_str(body);
            block.push('\n');
            if depth == 0 {
                break;
            }
        }
        // An unparseable block still yields the model, just without variants —
        // never drop a model the CLI reported.
        let parsed = serde_json::from_str::<Value>(&block).ok();
        let variants = parsed.as_ref().and_then(variant_ids);
        let name = parsed
            .as_ref()
            .and_then(|v| v.get("name"))
            .and_then(Value::as_str);
        let model = match variants {
            Some(ids) => {
                let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
                super::ModelInfo::new(header).with_reasoning(&refs)
            }
            None => super::ModelInfo::new(header),
        };
        models.push(model.with_label(name, None));
    }
    models
}

/// The variant ids of one model's verbose JSON, ordered weakest → strongest.
///
/// `Some(vec![])` (an empty `variants` map) is distinct from `None` (no
/// `variants` key at all): the former hides the picker, the latter falls back.
///
/// Ordering is imposed here rather than taken from the JSON: `serde_json`'s
/// default `Map` is a `BTreeMap`, so object keys arrive alphabetically
/// (`high, low, max, medium, xhigh`) and a picker in that order is nonsense.
/// Sorting by `OPENCODE_VARIANTS` restores the intended ramp.
fn variant_ids(model: &Value) -> Option<Vec<String>> {
    let variants = model.get("variants")?;
    let mut ids: Vec<String> = if let Some(map) = variants.as_object() {
        map.keys().cloned().collect()
    } else {
        // Tolerate an array form (`[]` is what an empty map serializes to in
        // some opencode builds — observed locally).
        variants
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    };
    // Known ids ramp in canonical order; anything unrecognized sorts after
    // them, alphabetically, so a new opencode variant still shows up.
    ids.sort_by_key(|id| {
        let rank = OPENCODE_VARIANTS
            .iter()
            .position(|v| v == id)
            .unwrap_or(OPENCODE_VARIANTS.len());
        (rank, id.clone())
    });
    Some(ids)
}

/// The variant ids opencode's catalog is known to use, weakest → strongest.
/// This ORDERS a model's variants for display (see `variant_ids`); it is not an
/// allowlist — opencode's catalog is the authority on what exists.
const OPENCODE_VARIANTS: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Session reasoning id → opencode's top-level `variant` value.
///
/// Only the `default` sentinel (and an absent level) send nothing; every other
/// value is forwarded as-is. Deliberately NOT filtered against
/// `OPENCODE_VARIANTS`: the ids come from opencode's own catalog, and
/// `variant_ids` goes out of its way to keep ones this build doesn't recognize
/// so a new variant still reaches the picker. Filtering here would offer such a
/// choice and then silently ignore it. `run_turn` has only the model id and
/// must not re-shell `opencode models` (a 20s subprocess) per turn, so opencode
/// itself is the validator of last resort.
fn opencode_variant(level: Option<&str>) -> Option<&str> {
    level.filter(|l| *l != REASONING_DEFAULT_ID)
}

/// opencode part → wire part (the shapes are already close).
fn to_wire_part(part: &Value) -> Option<WirePart> {
    let id = part.get("id")?.as_str()?.to_string();
    let kind = part.get("type")?.as_str()?;
    match kind {
        "text" | "reasoning" => Some(WirePart {
            id,
            kind: kind.into(),
            text: part.get("text").and_then(Value::as_str).map(str::to_string),
            tool: None,
            state: None,
            prompt: None,
            children: Vec::new(),
        }),
        "tool" => {
            let state = part.get("state");
            Some(WirePart {
                id,
                kind: "tool".into(),
                text: None,
                tool: part.get("tool").and_then(Value::as_str).map(str::to_string),
                state: Some(WireToolState {
                    status: state
                        .and_then(|s| s.get("status"))
                        .and_then(Value::as_str)
                        .unwrap_or("running")
                        .into(),
                    input: state.and_then(|s| s.get("input")).cloned(),
                    output: state
                        .and_then(|s| s.get("output"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    error: state
                        .and_then(|s| s.get("error"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    title: state
                        .and_then(|s| s.get("title"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                }),
                prompt: None,
                children: Vec::new(),
            })
        }
        _ => None,
    }
}

/// The id of the most-recent top-level `task` tool part not yet linked to a
/// child session — the row a freshly-spawned sub-agent session belongs to.
/// opencode's `session.created` carries the child's `parentID` (our session) but
/// not the spawning tool call, so we attribute to the latest unclaimed `task`
/// row; in the common single-task case this is exact.
///
/// Only top-level `task` rows are candidates, so nesting is one level deep: a
/// sub-agent that spawns its *own* sub-agent emits a `session.created` whose
/// `parentID` is the child session (not ours), so the grandchild isn't
/// registered and its events fall through to the foreign-session drop.
fn newest_task_part_id(parts: &[WirePart], claimed: &HashMap<String, String>) -> Option<String> {
    let taken: HashSet<&str> = claimed.values().map(String::as_str).collect();
    parts
        .iter()
        .rev()
        .find(|p| p.tool.as_deref() == Some("task") && !taken.contains(p.id.as_str()))
        .map(|p| p.id.clone())
}

/// opencode `permission.asked` payload → a `permission` card. The permission
/// request id rides on `native_id` so the reply can address
/// `POST /session/{sid}/permissions/{id}`. `permission` is opencode's tool
/// group (e.g. `bash`, `edit`); the metadata carries the concrete call detail.
fn permission_card(props: &Value) -> Option<WirePrompt> {
    let id = props.get("id").and_then(Value::as_str)?.to_string();
    Some(WirePrompt {
        kind: "permission".into(),
        tool: props
            .get("permission")
            .and_then(Value::as_str)
            .map(str::to_string),
        // The event's `metadata` is the closest thing to a tool input summary
        // the UI can render (command / file / etc., shape varies by tool).
        tool_input: props.get("metadata").filter(|m| !m.is_null()).cloned(),
        native_id: Some(id),
        ..Default::default()
    })
}

/// opencode `question.asked` payload → a `question` card. opencode's
/// `QuestionInfo` (`{question, header, options:[{label,description}], multiple}`)
/// is the same shape as Claude's AskUserQuestion, so it maps 1:1. Only the first
/// question is surfaced (the composer answers one at a time); its request id
/// rides on `native_id` for `POST /question/{id}/reply`.
fn question_card(props: &Value, plan_exit_calls: &HashSet<String>) -> Option<WirePrompt> {
    let id = props.get("id").and_then(Value::as_str)?.to_string();
    let q = props
        .get("questions")
        .and_then(Value::as_array)
        .and_then(|qs| qs.first())?;
    let options = q
        .get("options")
        .and_then(Value::as_array)
        .map(|opts| {
            opts.iter()
                .filter_map(|o| {
                    Some(WireQuestionOption {
                        label: o.get("label").and_then(Value::as_str)?.to_string(),
                        description: o
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(WirePrompt {
        kind: "question".into(),
        question: q
            .get("question")
            .and_then(Value::as_str)
            .map(str::to_string),
        header: q.get("header").and_then(Value::as_str).map(str::to_string),
        options,
        multi_select: q.get("multiple").and_then(Value::as_bool).unwrap_or(false),
        plan_exit: props
            .get("tool")
            .and_then(|tool| tool.get("callID"))
            .and_then(Value::as_str)
            .is_some_and(|call_id| plan_exit_calls.contains(call_id)),
        native_id: Some(id),
        ..Default::default()
    })
}

/// POST a permission decision to the live serve session (v1 API). `response` is
/// `once` | `always` | `reject`.
async fn post_permission(
    http: &reqwest::Client,
    base: &str,
    native_session: &str,
    permission_id: &str,
    response: &str,
) -> Result<()> {
    http.post(format!(
        "{base}/session/{native_session}/permissions/{permission_id}"
    ))
    .json(&json!({ "response": response }))
    .send()
    .await?
    .error_for_status()?;
    Ok(())
}

/// Deliver an answered card's reply to the live serve session, unblocking the
/// paused `session.prompt` POST. Permission → `{response: once|always|reject}`;
/// question → `{answers: [[label,...]]}` (or reject). The reply target is the
/// card's `native_id` (the opencode permission/question request id).
async fn reply_inline(ctx: &ResumeCtx, prompt: &WirePrompt, answer: &PromptAnswer) -> Result<()> {
    let request_id = prompt
        .native_id
        .as_deref()
        .ok_or_else(|| anyhow!("opencode prompt has no reply id"))?;
    // The reply only lands if the turn is still paused waiting for it. If the
    // turn already ended (errored / interrupted), serve may still accept the
    // POST but no one is consuming the resumed stream, so the reply would be
    // lost and the card would falsely mark resolved. Reject it instead — the
    // card stays actionable and the user sees the turn is no longer live.
    if !ctx.is_busy().await {
        return Err(anyhow!(
            "this turn is no longer running — its prompt can't be answered"
        ));
    }
    // Reach this session's live serve child through the shared host, exactly
    // as `ChatHost::interrupt` does — the reply goes to the same loopback
    // serve whose `session.prompt` POST is paused on this prompt.
    let port = ctx
        .host
        .opencode
        .port_for(&ctx.session_id)
        .await
        .ok_or_else(|| anyhow!("opencode serve is not running — cannot deliver the reply"))?;
    let base = format!("http://127.0.0.1:{port}");
    let http = ctx.http();

    match prompt.kind.as_str() {
        "permission" => {
            // approve → "always" (so the same tool won't re-prompt this turn);
            // reject closes it. The reply is session-scoped in opencode's v1 API.
            let native_session = ctx.native_session_id.as_deref().ok_or_else(|| {
                anyhow!("opencode session has no native id — cannot deliver the reply")
            })?;
            let response = if answer.approve { "always" } else { "reject" };
            post_permission(http, &base, native_session, request_id, response).await?;
        }
        "question" => {
            if answer.answers.is_empty() {
                // No selection: reject the question rather than reply empty, so
                // opencode surfaces the model's fallback path.
                http.post(format!("{base}/question/{request_id}/reject"))
                    .json(&json!({}))
                    .send()
                    .await?
                    .error_for_status()?;
            } else {
                // opencode takes an array of answers, one per question; we only
                // surface the first question, so send a single answer array.
                http.post(format!("{base}/question/{request_id}/reply"))
                    .json(&json!({ "answers": [&answer.answers] }))
                    .send()
                    .await?
                    .error_for_status()?;
            }
        }
        other => {
            return Err(anyhow!(
                "opencode cannot reply to a `{other}` prompt inline"
            ))
        }
    }
    Ok(())
}

/// Session mode → opencode built-in agent name. `Plan` runs the read-only
/// `plan` agent (denies edits, allows inspection); everything else runs the
/// default `build` agent. The permission-reply behavior (surface vs auto-reply)
/// is a separate axis handled in `handle_prompt_event`.
fn opencode_agent(plan_mode: bool) -> &'static str {
    if plan_mode {
        "plan"
    } else {
        "build"
    }
}

fn opencode_auto_approve(mode: Option<PermissionMode>) -> bool {
    matches!(mode, Some(PermissionMode::Auto))
}

fn plan_exit_transition(prompt: &WirePrompt, answer: &PromptAnswer) -> Option<bool> {
    prompt.plan_exit.then(|| {
        !answer
            .answers
            .iter()
            .any(|choice| choice.eq_ignore_ascii_case("yes"))
    })
}

#[derive(Debug)]
struct OpenCodeSetupHttpError {
    status: reqwest::StatusCode,
    retry_after: Option<Duration>,
}

impl std::fmt::Display for OpenCodeSetupHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OpenCode setup returned HTTP {}", self.status)
    }
}

impl std::error::Error for OpenCodeSetupHttpError {}

#[derive(Debug)]
struct OpenCodeSetupProtocolError(&'static str);

impl std::fmt::Display for OpenCodeSetupProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for OpenCodeSetupProtocolError {}

fn opencode_setup_response(response: reqwest::Response) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs);
    Err(OpenCodeSetupHttpError {
        status: response.status(),
        retry_after,
    }
    .into())
}

async fn opencode_setup_attempt(
    ctx: &mut TurnCtx,
    store: NativeStore,
) -> Result<(String, String, reqwest::Response)> {
    let status = ctx
        .host
        .opencode
        .ensure(&ctx.project, &ctx.session_id, store)
        .await?;
    let port = status
        .port
        .ok_or(OpenCodeSetupProtocolError("opencode agent has no port"))?;
    let base = format!("http://127.0.0.1:{port}");
    let native_id = match &ctx.native_session_id {
        Some(id) => id.clone(),
        None => {
            let response = ctx
                .http()
                .post(format!("{base}/session"))
                .header("content-type", "application/json")
                .body("{}")
                .send()
                .await?;
            let session: Value = opencode_setup_response(response)?.json().await?;
            let id = session
                .get("id")
                .and_then(Value::as_str)
                .ok_or(OpenCodeSetupProtocolError(
                    "opencode session response had no id",
                ))?
                .to_string();
            ctx.set_native_session_id(&id);
            id
        }
    };
    let events = ctx.http().get(format!("{base}/event")).send().await?;
    let events = opencode_setup_response(events)?;
    Ok((native_id, base, events))
}

async fn opencode_pre_accept_setup(
    ctx: &mut TurnCtx,
    store: NativeStore,
) -> Result<(String, String, reqwest::Response)> {
    loop {
        let remaining = ctx.orx_retry_remaining();
        let attempt = opencode_setup_attempt(ctx, store);
        let result = match remaining {
            Some(remaining) => tokio::time::timeout(remaining, attempt)
                .await
                .map_err(|_| anyhow!("OpenCode setup exceeded the ORX retry budget"))?,
            None => attempt.await,
        };
        match result {
            Ok(setup) => {
                ctx.clear_retry_status();
                return Ok(setup);
            }
            Err(error) => {
                let (retryable, explicit) =
                    if let Some(http) = error.downcast_ref::<OpenCodeSetupHttpError>() {
                        (
                            http.status.as_u16() == 408
                                || http.status.as_u16() == 429
                                || http.status.is_server_error(),
                            http.retry_after,
                        )
                    } else if let Some(request) = error.downcast_ref::<reqwest::Error>() {
                        (
                            request.is_connect() || request.is_timeout() || request.is_request(),
                            None,
                        )
                    } else {
                        (
                            error.downcast_ref::<OpenCodeSetupProtocolError>().is_none(),
                            None,
                        )
                    };
                let retry = retryable
                    .then(|| ctx.schedule_orx_retry(explicit))
                    .flatten();
                let Some((retry_number, delay)) = retry else {
                    ctx.mark_delivery(DeliveryState::NotSent);
                    ctx.mark_terminal_failure("opencode_setup", error.to_string());
                    return Err(error);
                };
                ctx.show_retry_status(
                    "orx",
                    "Reconnecting to OpenCode",
                    retry_number as i64 + 1,
                    Some(ORX_MAX_ATTEMPTS as i64),
                    Some(crate::store::now_ms() + delay.as_millis() as i64),
                );
                tokio::time::sleep(delay).await;
            }
        }
    }
}

async fn run_turn(ctx: &mut TurnCtx) -> Result<()> {
    // Native permission and question requests die with their turn. Clear any
    // crash/restart leftovers before a new live request can be surfaced.
    ctx.host
        .resolve_stale_prompts(&ctx.session_id, true)
        .await?;

    let native_session = match ctx.native_session_id.clone() {
        Some(id) => tokio::task::spawn_blocking(move || native_store::opencode_session(&id))
            .await
            .map_err(|error| anyhow!("OpenCode session lookup failed: {error}"))??,
        None => None,
    };
    let store = native_session
        .as_ref()
        .map(|session| session.store)
        .unwrap_or(NativeStore::Isolated);
    if ctx.native_session_id.is_some() && native_session.is_none() {
        if let Some(recovery) = super::native_recovery_context(ctx, "OpenCode") {
            ctx.text = format!("{recovery}\n\n{}", ctx.text);
        }
        ctx.native_session_id = None;
    }

    let (native_id, base, events) = opencode_pre_accept_setup(ctx, store).await?;
    let mut stream = events.bytes_stream();

    let mut body = json!({
        "parts": [{ "type": "text", "text": ctx.text }],
        // Select opencode's built-in agent from the session's mode: `plan` (the
        // read-only planning agent — allows inspection, denies edits) vs `build`
        // (the default). The message endpoint takes `agent` directly (verified),
        // so no separate switch call is needed.
        "agent": opencode_agent(ctx.plan_mode),
    });
    if let Some(model) = &ctx.model {
        if let Some((provider, model_id)) = model.split_once('/') {
            body["model"] = json!({ "providerID": provider, "modelID": model_id });
        }
    }
    // Reasoning → opencode's provider-specific `variant` (the serve API's
    // session-message field, mirroring `opencode run --variant`). Omitted for
    // `Default`, so the model's own reasoning default stands (issue #123).
    if let Some(variant) = opencode_variant(ctx.reasoning_level.as_deref()) {
        body["variant"] = json!(variant);
    }
    let turn_started_at = crate::store::now_ms();
    let send = ctx
        .http()
        .post(format!("{base}/session/{native_id}/message"))
        .json(&body)
        .send();
    ctx.persist_delivery(DeliveryState::Unknown)?;
    tokio::pin!(send);

    // Parts are attributed via message.updated role info; a part arriving
    // before its message would be misfiled, and assistant messages are always
    // announced before their parts stream.
    let mut assistant_msgs: HashSet<String> = HashSet::new();
    // Sub-agent child sessions spawned by a `task` tool this turn: child
    // sessionID → the task spawn part's id. Their events (a foreign sessionID)
    // route into that part's `children` instead of being dropped.
    let mut sub_sessions: HashMap<String, String> = HashMap::new();
    // Native plan exit is an ordinary `question.asked`; connect it to the
    // preceding `plan_exit` tool through the question's `tool.callID`.
    let mut plan_exit_calls: HashSet<String> = HashSet::new();
    let mut buf = String::new();

    loop {
        tokio::select! {
            chunk = stream.next() => {
                let Some(chunk) = chunk else {
                    return Err(anyhow!("opencode event stream ended mid-turn"));
                };
                buf.push_str(&String::from_utf8_lossy(&chunk?));
                while let Some(pos) = buf.find('\n') {
                    let line = buf[..pos].trim().to_string();
                    buf.drain(..=pos);
                    let Some(data) = line.strip_prefix("data: ") else { continue };
                    let Ok(event) = serde_json::from_str::<Value>(data) else { continue };
                    if let Some(part) = event
                        .get("properties")
                        .and_then(|props| props.get("part"))
                        .filter(|part| {
                            part.get("sessionID").and_then(Value::as_str) == Some(native_id.as_str())
                                && part.get("type").and_then(Value::as_str) == Some("tool")
                                && part.get("tool").and_then(Value::as_str) == Some("plan_exit")
                        })
                    {
                        if let Some(call_id) = part.get("callID").and_then(Value::as_str) {
                            plan_exit_calls.insert(call_id.to_string());
                        }
                    }
                    // Interactive prompts (permission/question) pause the turn and
                    // are handled async (emit a card, or auto-reply per mode); all
                    // other events are message/part updates handled synchronously.
                    if !handle_prompt_event(
                        ctx,
                        &native_id,
                        &base,
                        &event,
                        &plan_exit_calls,
                    )
                    .await?
                    {
                        handle_event(ctx, &native_id, &event, &mut assistant_msgs, &mut sub_sessions);
                    }
                }
            }
            resp = &mut send => {
                // Turn done — the response body is the final assistant message;
                // merge its parts as the authoritative versions.
                let resp = resp?.error_for_status()?;
                ctx.mark_delivery(DeliveryState::Accepted);
                ctx.clear_retry_status();
                if let Ok(message) = resp.json::<Value>().await {
                    if !opencode_response_is_current(&message, turn_started_at) {
                        let message = "OpenCode returned an earlier assistant message instead of replying to this turn. Update OpenCode or start a new chat.";
                        ctx.mark_terminal_failure("opencode_stale_response", message);
                        return Err(anyhow!(message));
                    }
                    if let Some(parts) = message.get("parts").and_then(Value::as_array) {
                        for part in parts {
                            if let Some(wire) = to_wire_part(part) {
                                // Preserve children: the final `task` part carries
                                // none, but its row already streamed the sub-agent
                                // transcript into `children`.
                                ctx.upsert_part_preserving_children(wire);
                            }
                        }
                    }
                }
                return Ok(());
            }
        }
    }
}

fn opencode_response_is_current(message: &Value, turn_started_at: i64) -> bool {
    message
        .pointer("/info/time/created")
        .and_then(Value::as_i64)
        .is_some_and(|created| created >= turn_started_at)
}

/// OpenCode assistant `tokens` occupying the context window:
/// `input + output + reasoning + cache.read + cache.write`. Returns `None` when
/// the object is absent, and `None` (not `Some(0)`) when every field is zero —
/// the early `message.updated` events carry an all-zero placeholder.
fn opencode_used_tokens(tokens: Option<&Value>) -> Option<u64> {
    let tokens = tokens?;
    let field = |v: &Value, name: &str| v.get(name).and_then(Value::as_u64).unwrap_or(0);
    let cache = tokens.get("cache").unwrap_or(&Value::Null);
    let total = field(tokens, "input")
        + field(tokens, "output")
        + field(tokens, "reasoning")
        + field(cache, "read")
        + field(cache, "write");
    (total > 0).then_some(total)
}

/// Whether a `session.updated` title is opencode's placeholder rather than a
/// real summary. The server seeds every session with `New session - <ISO
/// timestamp>` at creation and overwrites it once its own summarizer answers,
/// so the seed is a title to skip, not adopt.
fn is_opencode_seed_title(title: &str) -> bool {
    title.trim_start().starts_with("New session - ")
}

fn handle_event(
    ctx: &mut TurnCtx,
    native_id: &str,
    event: &Value,
    assistant_msgs: &mut HashSet<String>,
    sub_sessions: &mut HashMap<String, String>,
) {
    let props = event.get("properties").unwrap_or(&Value::Null);
    match event.get("type").and_then(Value::as_str) {
        Some("session.status") => {
            if props.get("sessionID").and_then(Value::as_str) != Some(native_id) {
                return;
            }
            let status = props.get("status").unwrap_or(&Value::Null);
            let status_type = status.get("type").and_then(Value::as_str);
            if status_type == Some("retry") {
                ctx.mark_delivery(DeliveryState::Accepted);
                let attempt = status.get("attempt").and_then(Value::as_i64).unwrap_or(1) + 1;
                let next = status.get("next").and_then(Value::as_i64).map(|next| {
                    if next > 1_000_000_000_000 {
                        next
                    } else {
                        crate::store::now_ms() + next
                    }
                });
                let message = status
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("OpenCode is retrying");
                ctx.show_retry_status("native", message, attempt, None, next);
            } else {
                if status_type == Some("busy") {
                    ctx.mark_delivery(DeliveryState::Accepted);
                }
                ctx.clear_retry_status();
            }
        }
        Some("session.error") => {
            if props.get("sessionID").and_then(Value::as_str) != Some(native_id) {
                return;
            }
            let error = props.get("error").unwrap_or(props);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("OpenCode reported an error")
                .to_string();
            ctx.mark_native_retry_exhausted();
            ctx.mark_delivery(DeliveryState::Accepted);
            ctx.mark_terminal_failure("opencode_terminal", message.clone());
            ctx.push_error(message);
        }
        // A `task` tool spawns a sub-agent in a child session; opencode announces
        // it with `session.created` carrying the child's `parentID` = our
        // session. Link that child session to the spawning `task` tool row so its
        // events stream into that row's `children`.
        Some("session.created") => {
            let info = props.get("info").unwrap_or(&Value::Null);
            if info.get("parentID").and_then(Value::as_str) == Some(native_id) {
                if let Some(child_id) = info.get("id").and_then(Value::as_str) {
                    if let Some(spawn) = newest_task_part_id(&ctx.assistant.parts, sub_sessions) {
                        sub_sessions.insert(child_id.to_string(), spawn);
                    }
                }
            }
        }
        Some("message.updated") => {
            let info = props.get("info").unwrap_or(&Value::Null);
            let session = info.get("sessionID").and_then(Value::as_str);
            let is_assistant = info.get("role").and_then(Value::as_str) == Some("assistant");
            if session == Some(native_id)
                && info.get("role").and_then(Value::as_str) == Some("user")
            {
                ctx.mark_delivery(DeliveryState::Accepted);
            }
            // Record assistant message ids for the main session AND registered
            // sub-sessions, so a session's user parts (e.g. the task prompt echo)
            // can be filtered out — for both the transcript and sub-agent nesting.
            let ours =
                session == Some(native_id) || session.is_some_and(|s| sub_sessions.contains_key(s));
            if ours && is_assistant {
                if let Some(id) = info.get("id").and_then(Value::as_str) {
                    assistant_msgs.insert(id.to_string());
                }
            }
            // Only the MAIN session's tokens drive the context meter; a
            // sub-agent's smaller counts must not overwrite it.
            if session == Some(native_id) && is_assistant {
                // Several `message.updated` fire per message; the early ones have
                // no tokens yet, so skip a report until real numbers land. The
                // context window isn't in this event (provider config only), so
                // report the token count without one.
                if let Some(used) = opencode_used_tokens(info.get("tokens")) {
                    ctx.report_usage(ContextUsage {
                        used_tokens: used,
                        context_window: None,
                    });
                }
            }
        }
        Some("message.part.updated") => {
            let part = props.get("part").unwrap_or(&Value::Null);
            let session = part.get("sessionID").and_then(Value::as_str);
            let owned_by_assistant = part
                .get("messageID")
                .and_then(Value::as_str)
                .is_some_and(|mid| assistant_msgs.contains(mid));
            // A sub-agent's part (foreign sessionID we've registered) streams
            // into its owning `task` row's children, with a namespaced id — but
            // only assistant-owned parts (skip the child's user prompt echo).
            if let Some(spawn) = session.and_then(|s| sub_sessions.get(s)).cloned() {
                if owned_by_assistant {
                    if let Some(mut wire) = to_wire_part(part) {
                        wire.id = format!("{spawn}:{}", wire.id);
                        ctx.upsert_child(&spawn, wire);
                        ctx.maybe_flush();
                    }
                }
                return;
            }
            if session != Some(native_id) || !owned_by_assistant {
                return;
            }
            if let Some(wire) = to_wire_part(part) {
                ctx.upsert_part(wire);
                ctx.maybe_flush();
            }
        }
        Some("message.part.delta") => {
            if props.get("field").and_then(Value::as_str) != Some("text") {
                return;
            }
            let session = props.get("sessionID").and_then(Value::as_str);
            let (Some(part_id), Some(delta)) = (
                props.get("partID").and_then(Value::as_str),
                props.get("delta").and_then(Value::as_str),
            ) else {
                return;
            };
            // Route a sub-agent's text delta into the owning task row's child.
            if let Some(spawn) = session.and_then(|s| sub_sessions.get(s)).cloned() {
                let child_id = format!("{spawn}:{part_id}");
                ctx.append_child_text(&spawn, &child_id, delta, || {
                    WirePart::text(child_id.clone(), "")
                });
                ctx.maybe_flush();
                return;
            }
            if session != Some(native_id) {
                return;
            }
            ctx.append_part_text(part_id, delta);
            ctx.maybe_flush();
        }
        Some("session.updated") => {
            // Adopt opencode's auto-generated titles. The creation seed arrives
            // in the first `session.updated` and the real title in a later one;
            // adopting the seed would latch it as 'generated' and permanently
            // reject the real one.
            let info = props.get("info").unwrap_or(&Value::Null);
            if info.get("id").and_then(Value::as_str) == Some(native_id) {
                if let Some(title) = info
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|t| !is_opencode_seed_title(t))
                {
                    ctx.set_title(title);
                }
            }
        }
        _ => {}
    }
}

/// Surface a prompt card and flush it so it renders immediately (before the
/// turn resumes). The card's `native_id` (the reply target) is also its
/// `WirePart` id, so the user's answer round-trips back to the right request.
fn surface_card(ctx: &mut TurnCtx, card: WirePrompt) {
    // `native_id` is always set by permission_card/question_card (opencode
    // requires the request id); the fallback id only guards a malformed payload.
    let part_id = card
        .native_id
        .clone()
        .unwrap_or_else(|| format!("prompt-{}", ctx.assistant.parts.len()));
    ctx.upsert_part(WirePart::prompt(part_id, card));
    let _ = ctx.flush();
}

/// Handle an interactive-prompt SSE event (`permission.asked` / `question.asked`)
/// for this session. Returns `true` if it consumed the event (so the caller
/// skips `handle_event`), `false` otherwise.
///
/// Permissions honor the session's policy: Auto-approve replies `always` to an
/// `ask` request, while Default surfaces a card. Explicit denies never emit an
/// approval request, so neither policy overrides them. Questions always surface — there's no
/// sensible auto-answer. A single flaky auto-reply must not lose the whole turn,
/// so on POST failure we fall back to surfacing the card rather than erroring.
async fn handle_prompt_event(
    ctx: &mut TurnCtx,
    native_id: &str,
    base: &str,
    event: &Value,
    plan_exit_calls: &HashSet<String>,
) -> Result<bool> {
    let props = event.get("properties").unwrap_or(&Value::Null);
    // Only this session's prompts (the /event stream is global across sessions).
    if props.get("sessionID").and_then(Value::as_str) != Some(native_id) {
        // Not a match — but if it *is* a prompt event for another session, still
        // report "not consumed" so handle_event ignores it too (it will, by id).
        return Ok(false);
    }
    match event.get("type").and_then(Value::as_str) {
        Some("permission.asked") => {
            let Some(card) = permission_card(props) else {
                // No request id to reply to — surface it as an error so the turn
                // isn't silently wedged waiting on an answer no one can give.
                ctx.push_error("opencode asked for a permission we couldn't parse".into());
                let _ = ctx.flush();
                return Ok(true);
            };
            // Auto-approve handles native `ask` requests; Default surfaces them.
            let auto_approve = opencode_auto_approve(ctx.permission_mode);
            match (auto_approve, card.native_id.as_deref()) {
                (true, Some(id)) => {
                    // Reply without surfacing a card — keep the turn flowing. If
                    // the reply POST fails, don't kill the turn: fall back to a
                    // card so the user can decide.
                    if let Err(err) =
                        post_permission(ctx.http(), base, native_id, id, "always").await
                    {
                        eprintln!("orx up: opencode auto-approve failed, surfacing card: {err}");
                        surface_card(ctx, card);
                    }
                }
                _ => surface_card(ctx, card),
            }
            Ok(true)
        }
        Some("question.asked") => {
            match question_card(props, plan_exit_calls) {
                Some(card) => surface_card(ctx, card),
                None => {
                    ctx.push_error("opencode asked a question we couldn't parse".into());
                    let _ = ctx.flush();
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed-down real `opencode models --verbose` output (1.17.15): a header
    /// line per model followed by its pretty-printed JSON. Covers the three
    /// cases that matter — a rich variants map, a *different* one on another
    /// model, and an empty one.
    const VERBOSE_SAMPLE: &str = r#"opencode/claude-fable-5
{
  "id": "claude-fable-5",
  "providerID": "opencode",
  "capabilities": {
    "reasoning": true,
    "input": { "text": true }
  },
  "variants": {
    "low": { "effort": "low" },
    "medium": { "effort": "medium" },
    "high": { "effort": "high" },
    "xhigh": { "effort": "xhigh" },
    "max": { "effort": "max" }
  }
}
opencode/gemini-3-flash
{
  "id": "gemini-3-flash",
  "providerID": "opencode",
  "variants": {
    "minimal": { "effort": "minimal" },
    "low": { "effort": "low" },
    "medium": { "effort": "medium" },
    "high": { "effort": "high" }
  }
}
opencode/glm-5
{
  "id": "glm-5",
  "providerID": "opencode",
  "variants": {}
}
"#;

    fn ids(m: &super::super::ModelInfo) -> Option<Vec<&str>> {
        m.reasoning_levels
            .as_ref()
            .map(|c| c.iter().map(|c| c.id.as_str()).collect())
    }

    /// The core of issue #123 for opencode: variants are genuinely per-model,
    /// so each model gets its own list rather than a hard-coded union.
    #[test]
    fn verbose_models_parse_per_model_variants() {
        let models = parse_verbose_models(VERBOSE_SAMPLE);
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            [
                "opencode/claude-fable-5",
                "opencode/gemini-3-flash",
                "opencode/glm-5"
            ]
        );
        // Nested `{ … }` inside the variants map must not end the block early.
        assert_eq!(
            ids(&models[0]),
            Some(vec!["default", "low", "medium", "high", "xhigh", "max"])
        );
        // A different model, a genuinely different set (note `minimal`, and no
        // `xhigh`/`max`) — the whole point of being model-aware.
        assert_eq!(
            ids(&models[1]),
            Some(vec!["default", "minimal", "low", "medium", "high"])
        );
    }

    /// Regression: `serde_json`'s default map is a `BTreeMap`, so raw key order
    /// is alphabetical (`high, low, max, medium, xhigh`) — a meaningless ramp
    /// in the picker. Variants must come out weakest → strongest regardless of
    /// the order they appear in the JSON.
    #[test]
    fn variants_are_ordered_weakest_to_strongest() {
        let model = serde_json::json!({
            "variants": { "max": {}, "low": {}, "xhigh": {}, "high": {}, "medium": {} }
        });
        assert_eq!(
            variant_ids(&model).unwrap(),
            ["low", "medium", "high", "xhigh", "max"]
        );
        // Unknown ids still survive, sorted after the known ramp.
        let odd = serde_json::json!({ "variants": { "zzz": {}, "high": {}, "aaa": {} } });
        assert_eq!(variant_ids(&odd).unwrap(), ["high", "aaa", "zzz"]);
    }

    /// A native variant literally named `default` must not produce a second
    /// row identical to the sentinel — that row would read as "no override" and
    /// make the real variant unselectable.
    #[test]
    fn a_native_default_variant_does_not_duplicate_the_sentinel() {
        let out = "prov/a\n{\n  \"variants\": { \"default\": {}, \"high\": {} }\n}\n";
        let models = parse_verbose_models(out);
        assert_eq!(ids(&models[0]), Some(vec!["default", "high"]));
    }

    /// An empty `variants` map means "checked, none supported" → an empty list,
    /// which hides the picker. It must NOT be `None`, which would fall back to
    /// the harness-wide list.
    #[test]
    fn empty_variants_map_hides_the_picker() {
        let models = parse_verbose_models(VERBOSE_SAMPLE);
        assert_eq!(ids(&models[2]), Some(vec![]));
        assert!(models[2].reasoning_levels.is_some());
    }

    /// Garbage or a `--verbose` flag the installed CLI doesn't support yields
    /// no models, which sends `opencode_models` to the plain-list fallback.
    #[test]
    fn unparseable_verbose_output_yields_nothing() {
        assert!(parse_verbose_models("").is_empty());
        assert!(parse_verbose_models("error: unknown flag --verbose").is_empty());
        // Header with no JSON block is skipped, not half-parsed.
        assert!(parse_verbose_models("opencode/foo\nnot json\n").is_empty());
    }

    /// The plain-list fallback still yields models, just without variants.
    #[test]
    fn plain_model_lines_have_no_variants() {
        let list: Vec<_> = model_id_lines("opencode/a\n\n  github-copilot/b  \njunk\n").collect();
        assert_eq!(list, ["opencode/a", "github-copilot/b"]);
        assert!(super::super::ModelInfo::new("opencode/a")
            .reasoning_levels
            .is_none());
    }

    /// A `{` inside a JSON string value must not desynchronize the brace
    /// counter. Before this was handled, one such brace consumed the rest of
    /// the output and every later model vanished — silently, since a partial
    /// parse is non-empty and so never reaches the plain-list fallback.
    #[test]
    fn brace_inside_a_string_does_not_swallow_later_models() {
        let out = concat!(
            "prov/a\n{\n  \"name\": \"Weird { name\",\n  \"variants\": { \"high\": {} }\n}\n",
            "prov/b\n{\n  \"name\": \"esc \\\" and } brace\",\n  \"variants\": {}\n}\n",
            "prov/c\n{\n  \"variants\": { \"low\": {}, \"max\": {} }\n}\n",
        );
        let models = parse_verbose_models(out);
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["prov/a", "prov/b", "prov/c"]
        );
        assert_eq!(ids(&models[0]), Some(vec!["default", "high"]));
        assert_eq!(ids(&models[1]), Some(vec![]));
        assert_eq!(ids(&models[2]), Some(vec!["default", "low", "max"]));
    }

    /// Only the sentinel is withheld. An unrecognized id is forwarded, because
    /// `variant_ids` deliberately keeps unknown variants so a new one still
    /// reaches the picker — offering it and then dropping it here would ignore
    /// the user's selection.
    #[test]
    fn variant_is_sent_unless_it_is_the_default_sentinel() {
        assert_eq!(opencode_variant(Some("high")), Some("high"));
        assert_eq!(opencode_variant(Some("minimal")), Some("minimal"));
        assert_eq!(opencode_variant(Some("none")), Some("none"));
        assert_eq!(opencode_variant(Some("brand-new")), Some("brand-new"));
        assert_eq!(opencode_variant(Some(REASONING_DEFAULT_ID)), None);
        assert_eq!(opencode_variant(None), None);
    }

    /// Every variant id detection advertises must survive the mapper — the
    /// picker can never offer a value `run_turn` would silently drop. Includes
    /// an unknown id, which is exactly the case a mapper-side allowlist broke.
    #[test]
    fn advertised_variants_all_map_back() {
        let unknown = "prov/x\n{\n  \"variants\": { \"high\": {}, \"turbo\": {} }\n}\n";
        for model in parse_verbose_models(VERBOSE_SAMPLE)
            .into_iter()
            .chain(parse_verbose_models(unknown))
        {
            for choice in model.reasoning_levels.into_iter().flatten() {
                if choice.id == REASONING_DEFAULT_ID {
                    continue;
                }
                assert_eq!(
                    opencode_variant(Some(&choice.id)),
                    Some(choice.id.as_str()),
                    "{} advertises {} but the mapper drops it",
                    model.id,
                    choice.id
                );
            }
        }
    }

    #[test]
    fn plan_mode_uses_the_plan_agent_others_build() {
        assert_eq!(opencode_agent(true), "plan");
        assert_eq!(opencode_agent(false), "build");
    }

    #[test]
    fn plan_and_permissions_form_four_independent_combinations() {
        for (plan_mode, permission_mode, agent, auto_approve) in [
            (false, Some(PermissionMode::Ask), "build", false),
            (false, Some(PermissionMode::Auto), "build", true),
            (true, Some(PermissionMode::Ask), "plan", false),
            (true, Some(PermissionMode::Auto), "plan", true),
        ] {
            assert_eq!(opencode_agent(plan_mode), agent);
            assert_eq!(opencode_auto_approve(permission_mode), auto_approve);
        }
    }

    // `properties` payloads shaped exactly like the live `permission.asked` /
    // `question.asked` events (verified against opencode serve). These pin the
    // field names the parsers read — the kind that silently yields a `None` card
    // at runtime if opencode ever renames one.
    #[test]
    fn permission_card_reads_id_permission_metadata() {
        let props = json!({
            "id": "per_abc123",
            "sessionID": "ses_x",
            "permission": "bash",
            "patterns": [],
            "metadata": { "command": "orx runs r1" },
            "always": [],
            "tool": { "messageID": "m1", "callID": "c1" }
        });
        let card = permission_card(&props).expect("should parse");
        assert_eq!(card.kind, "permission");
        assert_eq!(card.tool.as_deref(), Some("bash"));
        assert_eq!(card.native_id.as_deref(), Some("per_abc123")); // the reply target
        assert_eq!(
            card.tool_input
                .as_ref()
                .and_then(|m| m.get("command"))
                .and_then(|c| c.as_str()),
            Some("orx runs r1")
        );
        // No id → no reply target → no card.
        assert!(permission_card(&json!({ "permission": "bash" })).is_none());
    }

    #[test]
    fn question_card_reads_first_question_and_opencode_multiple_field() {
        let props = json!({
            "id": "que_xyz",
            "sessionID": "ses_x",
            "questions": [{
                "question": "Which backend?",
                "header": "Backend",
                "options": [
                    { "label": "modal", "description": "per-second" },
                    { "label": "k8s", "description": "your cluster" }
                ],
                "multiple": true
            }]
        });
        let card = question_card(&props, &HashSet::new()).expect("should parse");
        assert_eq!(card.kind, "question");
        assert_eq!(card.native_id.as_deref(), Some("que_xyz"));
        assert_eq!(card.question.as_deref(), Some("Which backend?"));
        assert_eq!(card.header.as_deref(), Some("Backend"));
        assert_eq!(card.options.len(), 2);
        assert_eq!(card.options[0].label, "modal");
        assert_eq!(card.options[0].description.as_deref(), Some("per-second"));
        // opencode's field is `multiple`, NOT Claude's `multiSelect`.
        assert!(card.multi_select);
        // A `multiSelect` (Claude's name) is NOT read → defaults to false.
        let claude_shaped = json!({
            "id": "que_1",
            "questions": [{ "question": "q", "header": "h", "options": [], "multiSelect": true }]
        });
        assert!(
            !question_card(&claude_shaped, &HashSet::new())
                .unwrap()
                .multi_select
        );
        // No questions → no card.
        assert!(question_card(&json!({ "id": "que_1" }), &HashSet::new()).is_none());
    }

    #[test]
    fn question_card_recognizes_plan_exit_by_tool_call_id() {
        let props = json!({
            "id": "que_exit",
            "tool": { "messageID": "m1", "callID": "call_plan" },
            "questions": [{
                "question": "Switch to build?",
                "header": "Build Agent",
                "options": [{ "label": "Yes" }, { "label": "No" }],
                "multiple": false
            }]
        });
        let calls = HashSet::from(["call_plan".to_string()]);
        assert!(question_card(&props, &calls).unwrap().plan_exit);
        assert!(!question_card(&props, &HashSet::new()).unwrap().plan_exit);
    }

    #[test]
    fn native_plan_exit_yes_leaves_plan_no_keeps_it() {
        let mut prompt = WirePrompt {
            plan_exit: true,
            ..Default::default()
        };
        let response = |choice: &str| PromptAnswer {
            session_id: "session".into(),
            prompt_id: "question".into(),
            approve: true,
            resume_mode: None,
            answers: vec![choice.into()],
            note: None,
            annotations: Vec::new(),
        };
        assert_eq!(plan_exit_transition(&prompt, &response("Yes")), Some(false));
        assert_eq!(plan_exit_transition(&prompt, &response("No")), Some(true));
        prompt.plan_exit = false;
        assert_eq!(plan_exit_transition(&prompt, &response("Yes")), None);
    }

    #[test]
    fn message_updated_reports_summed_tokens_without_window() {
        let mut ctx = TurnCtx::test_stub();
        let mut msgs = HashSet::new();
        let event = json!({
            "type": "message.updated",
            "properties": { "info": {
                "id": "msg_1",
                "sessionID": "ses_x",
                "role": "assistant",
                "tokens": { "input": 1200, "output": 340, "reasoning": 50, "cache": { "read": 8000, "write": 200 } }
            }}
        });
        handle_event(&mut ctx, "ses_x", &event, &mut msgs, &mut HashMap::new());
        let usage = ctx.context_usage.expect("usage reported");
        assert_eq!(usage.used_tokens, 1200 + 340 + 50 + 8000 + 200);
        assert_eq!(usage.context_window, None);
    }

    #[test]
    fn message_updated_without_tokens_reports_nothing() {
        let mut ctx = TurnCtx::test_stub();
        let mut msgs = HashSet::new();
        // Early message.updated: assistant role, but no tokens yet.
        let no_tokens = json!({
            "type": "message.updated",
            "properties": { "info": { "id": "msg_1", "sessionID": "ses_x", "role": "assistant" }}
        });
        handle_event(
            &mut ctx,
            "ses_x",
            &no_tokens,
            &mut msgs,
            &mut HashMap::new(),
        );
        assert!(ctx.context_usage.is_none());
        // All-zero placeholder tokens must also be ignored.
        let zero_tokens = json!({
            "type": "message.updated",
            "properties": { "info": { "id": "msg_1", "sessionID": "ses_x", "role": "assistant",
                "tokens": { "input": 0, "output": 0, "reasoning": 0, "cache": { "read": 0, "write": 0 } }}}
        });
        handle_event(
            &mut ctx,
            "ses_x",
            &zero_tokens,
            &mut msgs,
            &mut HashMap::new(),
        );
        assert!(ctx.context_usage.is_none());
    }

    #[test]
    fn stale_final_response_is_rejected() {
        assert!(opencode_response_is_current(
            &json!({"info":{"time":{"created":100}}}),
            100
        ));
        assert!(!opencode_response_is_current(
            &json!({"info":{"time":{"created":99}}}),
            100
        ));
    }

    #[test]
    fn auth_rejections_are_recognized_but_other_failures_are_not() {
        assert!(is_auth_rejection(
            "Error: API key not valid. Please pass a valid API key."
        ));
        assert!(is_auth_rejection("Incorrect API key provided: sk-abc"));
        assert!(is_auth_rejection("{\"type\":\"authentication_error\"}"));
        assert!(is_auth_rejection("HTTP 401 Unauthorized"));
        assert!(!is_auth_rejection("Error: fetch failed (ENOTFOUND)"));
        assert!(!is_auth_rejection("ok"));
        assert!(!is_auth_rejection("model not found: google/nope"));
    }

    #[test]
    fn model_provider_is_the_prefix() {
        assert_eq!(model_provider("google/gemini-2.5-flash"), "google");
        assert_eq!(model_provider("opencode/big-pickle"), "opencode");
        assert_eq!(model_provider("bare"), "bare");
    }

    #[test]
    fn seed_title_is_recognized_but_real_titles_pass() {
        // The exact shape opencode stamps at session creation.
        assert!(is_opencode_seed_title(
            "New session - 2026-07-09T23:50:40.501Z"
        ));
        assert!(is_opencode_seed_title(
            "  New session - 2026-07-09T23:50:40.501Z"
        ));
        // What the summarizer actually produces — must reach `set_title`.
        assert!(!is_opencode_seed_title("Fix the login redirect"));
        assert!(!is_opencode_seed_title("New session handling in the store"));
        assert!(!is_opencode_seed_title(""));
    }

    #[test]
    fn only_active_session_statuses_confirm_turn_acceptance() {
        let mut ctx = TurnCtx::test_stub();
        ctx.mark_delivery(DeliveryState::Unknown);
        let mut messages = HashSet::new();
        let mut sessions = HashMap::new();
        handle_event(
            &mut ctx,
            "ses_x",
            &json!({"type":"session.status","properties":{"sessionID":"ses_x","status":{"type":"idle"}}}),
            &mut messages,
            &mut sessions,
        );
        assert_eq!(ctx.delivery_state(), DeliveryState::Unknown);
        handle_event(
            &mut ctx,
            "ses_x",
            &json!({"type":"session.status","properties":{"sessionID":"ses_x","status":{"type":"retry","attempt":1}}}),
            &mut messages,
            &mut sessions,
        );
        assert_eq!(ctx.delivery_state(), DeliveryState::Accepted);
    }

    /// A `task` tool spawns a child session (announced via `session.created` with
    /// `parentID` = our session); the sub-agent's parts stream into the task
    /// row's `children`, not the top-level transcript.
    #[test]
    fn subagent_parts_stream_into_the_task_row_children() {
        let mut ctx = TurnCtx::test_stub();
        let mut msgs: HashSet<String> = HashSet::new();
        let mut subs: HashMap<String, String> = HashMap::new();
        // The main assistant message + its `task` tool call (top-level).
        handle_event(
            &mut ctx,
            "ses_main",
            &json!({"type":"message.updated","properties":{"info":{"id":"msg_1","sessionID":"ses_main","role":"assistant"}}}),
            &mut msgs,
            &mut subs,
        );
        handle_event(
            &mut ctx,
            "ses_main",
            &json!({"type":"message.part.updated","properties":{"part":{
                "id":"prt_task","type":"tool","tool":"task","sessionID":"ses_main","messageID":"msg_1",
                "state":{"status":"running","input":{"description":"analyze"}}}}}),
            &mut msgs,
            &mut subs,
        );
        // opencode announces the spawned child session (parentID = our session).
        handle_event(
            &mut ctx,
            "ses_main",
            &json!({"type":"session.created","properties":{"info":{"id":"ses_child","parentID":"ses_main"}}}),
            &mut msgs,
            &mut subs,
        );
        assert_eq!(subs.get("ses_child").map(String::as_str), Some("prt_task"));
        // The child session's assistant message + a tool part → nests under task.
        handle_event(
            &mut ctx,
            "ses_main",
            &json!({"type":"message.updated","properties":{"info":{"id":"msg_c","sessionID":"ses_child","role":"assistant"}}}),
            &mut msgs,
            &mut subs,
        );
        handle_event(
            &mut ctx,
            "ses_main",
            &json!({"type":"message.part.updated","properties":{"part":{
                "id":"prt_bash","type":"tool","tool":"bash","sessionID":"ses_child","messageID":"msg_c",
                "state":{"status":"completed","input":{"command":"ls"},"output":"a.rs"}}}}),
            &mut msgs,
            &mut subs,
        );
        // Only the task row is top-level; the sub bash nested under it (namespaced).
        assert_eq!(ctx.assistant.parts.len(), 1, "{:?}", ctx.assistant.parts);
        let task = &ctx.assistant.parts[0];
        assert_eq!(task.id, "prt_task");
        assert_eq!(task.tool.as_deref(), Some("task"));
        let bash = task
            .children
            .iter()
            .find(|p| p.id == "prt_task:prt_bash")
            .expect("sub bash nested under the task row");
        assert_eq!(bash.state.as_ref().unwrap().output.as_deref(), Some("a.rs"));

        // The turn-end merge re-upserts the main message's parts (incl. the task
        // row, rebuilt with empty children) authoritatively. It MUST preserve the
        // accrued children — a plain upsert would wipe the sub-agent transcript.
        let final_task = to_wire_part(&json!({
            "id":"prt_task","type":"tool","tool":"task","sessionID":"ses_main","messageID":"msg_1",
            "state":{"status":"completed","input":{"description":"analyze"},"output":"done"}
        }))
        .unwrap();
        assert!(
            final_task.children.is_empty(),
            "rebuilt part has no children"
        );
        ctx.upsert_part_preserving_children(final_task);
        let task = &ctx.assistant.parts[0];
        assert_eq!(task.state.as_ref().unwrap().status, "completed");
        assert_eq!(task.children.len(), 1, "children survive the final merge");
    }
}
