//! Codex harness.
//!
//! Chat rides Codex's **app-server** protocol (codex ≥ 0.144): one long-lived
//! `codex app-server` child per session (see `local::codex`), a thread per
//! session (`thread/start` / `thread/resume` — the thread id persists as the
//! session's `native_session_id`), one `turn/start` per message, events
//! streamed as JSON-RPC notifications. The playbook rides
//! `developerInstructions` (a real instruction channel — no more first-turn
//! `<system-context>` text wrapping), and the sandbox policy travels per turn
//! (`sandboxPolicy` with writable roots + network). Auto uses Codex's built-in
//! approval reviewer for first-party providers; requests left to the client are
//! surfaced as permission cards and answered inline over the same connection
//! (`resume_from_prompt` → `{"decision": accept|decline}`). Verified against
//! codex-cli 0.144.0 via
//! `codex app-server generate-json-schema` plus a live spike; the fixture
//! transcript in the tests pins the wire shapes.
//!
//! Older codex (< 0.144) falls back to the legacy exec path for one release:
//! one `codex exec --json` process per turn, JSONL events on stdout,
//! multi-turn via `codex exec resume <session>`, playbook injected as tagged
//! context on the first turn. `ORX_CODEX_EXEC=1` forces the fallback.
//!
//! Detection follows the active provider in `$CODEX_HOME/config.toml`. A custom
//! provider authenticates with its declared `env_key`; otherwise
//! `auth.json` holds either an `OPENAI_API_KEY` or an OAuth `id_token` JWT we
//! decode (unverified) for the account email and plan.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::detect::{
    bin_version, jwt_payload, nonempty_str, parse_version, probe_bin, read_json, resolve_symlinks,
    title_case, HarnessInfo, ModelInfo,
};
use super::options::{
    resolve_reasoning, HarnessOptions, OptionChoice, PermissionMode, PlanActivation,
    REASONING_DEFAULT_ID,
};
use super::{
    should_synthesize_plan, synthesize_resume, Harness, OneShot, OneShotQuality, ResumeAction,
    TurnFailure, TurnOutcome, TurnResult, Waited, ORX_MAX_ATTEMPTS, TURN_WATCHDOG,
};
use crate::error::{anyhow, Result};
use crate::local::chat::{
    find_part_mut, prepare_env, set_chat_session_env, upsert_preserving_children, ContextUsage,
    DeliveryState, PromptAnswer, ResumeCtx, SteerMessage, TurnCtx, WireMessage, WirePart,
    WirePrompt, WireQuestionOption, WireToolState,
};
use crate::local::codex::{CodexClient, JsonRpcError, ServerReqKind, TurnEvent};
use crate::local::native_store::{self, NativeStore};
use crate::local::opencode::ensure_playbook;
use crate::local::shell_env::find_on_path;
use crate::store::{Store, StoredChatMessage};

// FALLBACK model table, used only when the app-server catalog is unreachable
// (codex < 0.144's legacy exec path, or a failed/timed-out `model/list`). The
// primary source is `codex_model_list`: the app-server's `model/list` reports
// every model with its `supportedReasoningEfforts`, exactly like opencode's
// `models --verbose` — so models and tiers are normally *queried*, not curated.
//
// Each entry is `(model id, the `model_reasoning_effort` values it accepts)`,
// mirroring the catalog as of codex-cli 0.144. Sol/Terra reach `ultra`; Luna
// stops at `max`; 5.5 stops at `xhigh`. (A live `codex exec` turn on Luna
// tolerated `ultra`, but the catalog is what codex's own picker offers — the
// catalog wins for what WE offer.) Getting a tier wrong is not cosmetic: codex
// forwards the value unvalidated and an unsupported one comes back as a 400
// that kills the turn (observed: 5.5 + `max`).
const CODEX_MODELS: [(&str, &[&str]); 4] = [
    (
        "gpt-5.6-sol",
        &["low", "medium", "high", "xhigh", "max", "ultra"],
    ),
    (
        "gpt-5.6-terra",
        &["low", "medium", "high", "xhigh", "max", "ultra"],
    ),
    ("gpt-5.6-luna", &["low", "medium", "high", "xhigh", "max"]),
    ("gpt-5.5", &["low", "medium", "high", "xhigh"]),
];

/// Codex usage occupying the context window: `input_tokens + output_tokens`
/// (`cached_input_tokens` is a subset of `input_tokens`, not additive). Returns
/// `None` when the object is absent, or when the sum is zero (an all-zero
/// payload isn't real occupancy and must not render "0%").
fn codex_used_tokens(usage: Option<&Value>) -> Option<u64> {
    let usage = usage?;
    let field = |name: &str| usage.get(name).and_then(Value::as_u64).unwrap_or(0);
    let total = field("input_tokens") + field("output_tokens");
    (total > 0).then_some(total)
}

/// Read a legacy-exec `token_count` `info` object into (occupancy, window).
/// `last_token_usage` is the most recent request, whose `input_tokens` already
/// contains the full resent context — that IS the context occupancy (what the
/// codex TUI shows), and it matches the app-server's per-turn `turn.usage`.
/// `total_token_usage` is a running sum across every request in the session (it
/// only grows), so it's the fallback, not the preference.
fn token_count_usage(info: &Value) -> (Option<u64>, Option<u64>) {
    let usage = info
        .get("last_token_usage")
        .filter(|v| !v.is_null())
        .or_else(|| info.get("total_token_usage"));
    let window = info.get("model_context_window").and_then(Value::as_u64);
    (codex_used_tokens(usage), window)
}

/// The harness-wide fallback list — the conservative intersection, used for a
/// model that isn't in `CODEX_MODELS` (a `-c model=…` override, or a newer id
/// this build doesn't know).
const CODEX_REASONING_LEVELS: [&str; 4] = ["low", "medium", "high", "xhigh"];

/// Deliberately channel-neutral: `find_codex` takes whatever is on PATH, and
/// naming one installer would send a brew or standalone install to npm.
const CODEX_REINSTALL: &str = "Reinstall Codex (developers.openai.com/codex)";

/// The effort ids a given codex model accepts per the FALLBACK table, or the
/// conservative intersection. Send-time validation only — detection prefers
/// the live catalog (`codex_model_list`).
fn codex_model_reasoning(model: &str) -> Option<&'static [&'static str]> {
    CODEX_MODELS
        .iter()
        .find(|(id, _)| *id == model)
        .map(|(_, levels)| *levels)
}

/// Query the app-server's `model/list` — codex's own catalog, the same data its
/// TUI picker renders: every model with its `supportedReasoningEfforts` and
/// default. This is the primary model source for first-party accounts and for
/// custom providers that declare an explicit model catalog (the static table is
/// only the fallback), for the same reason opencode parses `models --verbose`:
/// the installed CLI knows its catalog and we don't — a curated table here
/// shipped missing three models and a wrong Luna tier before this existed.
///
/// Protocol: spawn `codex app-server`, `initialize` → `initialized` (the same
/// handshake `local::codex` uses, incl. `experimentalApi` — `model/list` is
/// part of the v2 surface), then one `model/list` request. Any failure —
/// spawn, timeout, old codex without the method — returns `None` and the
/// caller falls back to the static table. Hidden catalog entries are skipped
/// (the server already filters them by default; the guard is belt-and-braces).
async fn codex_model_list(bin: &Path, configured_effort: Option<&str>) -> Option<Vec<ModelInfo>> {
    let fut = async {
        let mut cmd = Command::new(bin);
        cmd.arg("app-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        prepare_env(&mut cmd);
        let mut child = cmd.spawn().ok()?;
        let mut stdin = child.stdin.take()?;
        let mut lines = BufReader::new(child.stdout.take()?).lines();

        use tokio::io::AsyncWriteExt;
        async fn send(stdin: &mut tokio::process::ChildStdin, v: Value) -> Option<()> {
            let mut line = v.to_string();
            line.push('\n');
            stdin.write_all(line.as_bytes()).await.ok()
        }
        // Read until the response with this id (skipping notifications).
        async fn recv(
            lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
            id: u64,
        ) -> Option<Value> {
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    if v.get("id").and_then(Value::as_u64) == Some(id) {
                        return Some(v);
                    }
                }
            }
            None
        }

        send(
            &mut stdin,
            serde_json::json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "clientInfo": {
                        "name": "orx",
                        "title": "OpenResearch",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "capabilities": { "experimentalApi": true },
                },
            }),
        )
        .await?;
        recv(&mut lines, 1).await?;
        send(&mut stdin, serde_json::json!({ "method": "initialized" })).await?;
        send(
            &mut stdin,
            serde_json::json!({ "id": 2, "method": "model/list", "params": {} }),
        )
        .await?;
        let resp = recv(&mut lines, 2).await?;
        let models = parse_model_list(resp.get("result")?, configured_effort);
        (!models.is_empty()).then_some(models)
    };
    tokio::time::timeout(Duration::from_secs(15), fut)
        .await
        .ok()
        .flatten()
}

/// `model/list` result → per-model `ModelInfo`, efforts attached in catalog
/// order. Split from the transport for testability.
///
/// Each model gets a *concrete* preselected tier rather than a "no override"
/// sentinel: the tier that actually runs when the user picks nothing, which
/// codex resolves as the `config.toml` `model_reasoning_effort` override when
/// that's set (and supported by the model), else the catalog's own
/// `defaultReasoningEffort`. Preselecting-and-sending that tier is equivalent
/// to sending nothing, and the picker shows a real value instead of "Default".
fn parse_model_list(result: &Value, configured_effort: Option<&str>) -> Vec<ModelInfo> {
    let Some(data) = result.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    data.iter()
        .filter(|m| !m.get("hidden").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|m| {
            // `model` is the slug the turn passes as `-m`/`model`; `id` equals
            // it in practice but `model` is the documented carrier.
            let id = m
                .get("model")
                .or_else(|| m.get("id"))
                .and_then(Value::as_str)?;
            let efforts: Vec<&str> = m
                .get("supportedReasoningEfforts")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|e| e.get("reasoningEffort").and_then(Value::as_str))
                        .collect()
                })
                .unwrap_or_default();
            let catalog_default = m.get("defaultReasoningEffort").and_then(Value::as_str);
            let default = configured_effort
                .filter(|e| efforts.contains(e))
                .or(catalog_default);
            let info = ModelInfo::new(id).with_label(
                m.get("displayName").and_then(Value::as_str),
                m.get("description").and_then(Value::as_str),
            );
            let info = info.with_service_tiers(
                m.get("serviceTiers")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|tier| tier.get("id").and_then(Value::as_str) == Some("priority"))
                    .filter_map(|tier| {
                        Some(OptionChoice {
                            id: tier.get("id")?.as_str()?.to_string(),
                            label: tier.get("name")?.as_str()?.to_string(),
                            description: tier
                                .get("description")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                        })
                    })
                    .collect(),
            );
            Some(match default {
                Some(default) => info.with_reasoning_default(&efforts, default),
                // No reported default (an older catalog shape) → keep the
                // sentinel-led list, where "no override" is the safe lead.
                None => info.with_reasoning(&efforts),
            })
        })
        .collect()
}

pub struct Codex;

/// Only the fields detection needs off `config.toml`; codex has many more.
#[derive(Deserialize)]
struct CodexConfig {
    model: Option<String>,
    model_provider: Option<String>,
    /// The user's configured effort override. Codex resolves it above the
    /// catalog's per-model `defaultReasoningEffort`, so the picker's
    /// preselected tier must too.
    model_reasoning_effort: Option<String>,
    /// An explicit provider model catalog (`model_catalog_json`). When a custom
    /// provider declares one, the app-server's `model/list` reflects it and the
    /// picker can offer every model the provider exposes; without one the CLI
    /// falls back to its bundled first-party catalog, which says nothing about
    /// a custom endpoint.
    model_catalog_json: Option<String>,
    #[serde(default)]
    model_providers: HashMap<String, CodexProvider>,
}

/// The `model_reasoning_effort` the user configured in `config.toml`, if any.
fn parse_configured_effort(raw: &str) -> Option<String> {
    toml::from_str::<CodexConfig>(raw)
        .ok()?
        .model_reasoning_effort
}

/// Keep the configured model first without discarding catalog metadata.
/// With either input absent, preserve the other as-is.
fn custom_provider_models(
    configured_model: Option<&str>,
    catalog: Option<Vec<ModelInfo>>,
) -> Vec<ModelInfo> {
    let Some(configured_model) = configured_model else {
        return catalog.unwrap_or_default();
    };
    let Some(mut models) = catalog else {
        return vec![ModelInfo::new(configured_model)];
    };
    match models.iter().position(|model| model.id == configured_model) {
        Some(0) => {}
        Some(index) => {
            let configured = models.remove(index);
            models.insert(0, configured);
        }
        None => {
            // Unknown catalog metadata keeps "no override", so Codex applies configured effort.
            models.insert(0, ModelInfo::new(configured_model));
        }
    }
    models
}

#[derive(Deserialize)]
struct CodexProvider {
    env_key: Option<String>,
    #[serde(default)]
    requires_openai_auth: bool,
}

struct CustomProvider {
    model: Option<String>,
    env_key: Option<String>,
    has_model_catalog: bool,
}

impl CustomProvider {
    /// A provider that declares no `env_key` carries its credential elsewhere
    /// (or needs none), so treat it as usable rather than blocking on a var we
    /// were never told the name of.
    fn is_ready(&self) -> bool {
        match self.env_key.as_deref() {
            Some(key) => super::detect::api_key(key).is_some(),
            None => true,
        }
    }
}

/// The active provider, when it is a custom one that bypasses OpenAI auth.
/// `None` means first-party detection (auth.json) applies.
fn parse_custom_provider(raw: &str) -> Option<CustomProvider> {
    let cfg: CodexConfig = toml::from_str(raw).ok()?;
    let provider = cfg.model_providers.get(cfg.model_provider.as_deref()?)?;
    if provider.requires_openai_auth {
        return None;
    }
    Some(CustomProvider {
        model: cfg.model.filter(|model| !model.trim().is_empty()),
        env_key: provider.env_key.clone(),
        has_model_catalog: cfg
            .model_catalog_json
            .is_some_and(|path| !path.trim().is_empty()),
    })
}

/// `codex` on PATH, symlinks resolved (see `resolve_symlinks` — codex needs to
/// find its `codex-code-mode-host` helper next to the real binary).
pub fn find_codex() -> Option<PathBuf> {
    find_on_path("codex").map(resolve_symlinks)
}

/// `find_codex` with the install hint baked in (the `find_opencode` precedent)
/// — shared by both transports' spawn paths.
pub(crate) fn find_codex_required() -> Result<PathBuf> {
    find_codex().ok_or_else(|| {
        anyhow!("codex not found on PATH — install Codex and run `codex login` first")
    })
}

/// One headless request on a throwaway `codex exec` thread. Deliberately
/// *not* the session's own thread — a request there would pollute the real
/// conversation history.
///
/// Runs on `request.model` when given, else the user's default model;
/// `Cheap` drops reasoning effort to `low`. `--ephemeral` keeps the throwaway thread out of
/// every session store. Codex has no system-prompt flag, so `system` leads the
/// message.
///
/// Any failure — spawn, non-zero exit, timeout, garbage output — returns `None`
/// and the caller keeps its fallback.
async fn codex_one_shot(bin: &Path, request: OneShot<'_>) -> Option<String> {
    let effort = match request.quality {
        OneShotQuality::Cheap => "low",
        OneShotQuality::Standard => "medium",
    };
    let message = format!("{}\n\n{}", request.system, request.prompt);
    let fut = async {
        let mut cmd = Command::new(bin);
        cmd.args(["exec", "--ephemeral", "--json", "--skip-git-repo-check"])
            .args(["-c", "sandbox_mode=\"read-only\""])
            .args(["-c", "approval_policy=\"never\""])
            .args(["-c", &format!("model_reasoning_effort=\"{effort}\"")])
            .args(request.model.iter().flat_map(|model| ["-m", model]))
            // A one-shot needs no MCP: booting the user's servers for a
            // single request would cost far more than the request itself.
            .args(["-c", "mcp_servers={}"])
            // Nor skills: the plugin catalog alone injected ~6k prompt tokens
            // (24.3k → 18k input measured) into a request that ignores it.
            .args(["-c", "features.plugins=false"])
            .arg(&message)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            // Hermetic: run outside any repo so the child doesn't ingest the
            // server cwd's AGENTS.md into a request that carries its own context.
            .current_dir(std::env::temp_dir());
        prepare_env(&mut cmd);
        cmd.env(
            "CODEX_HOME",
            native_store::prepare_codex(NativeStore::Isolated).ok()?,
        );
        // Plain text only — an ANSI-colorizing CLI (or a synced FORCE_COLOR)
        // would otherwise write escape codes straight into the reply.
        cmd.env("NO_COLOR", "1");
        let mut child = cmd.spawn().ok()?;
        let mut lines = BufReader::new(child.stdout.take()?).lines();
        // Keep the last agent message: a chatty run may narrate before it
        // answers, and the reply is what it settled on.
        let mut last = None;
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(text) = exec_line_agent_message(&line) {
                last = Some(text);
            }
        }
        if !child.wait().await.ok()?.success() {
            return None;
        }
        last
    };
    tokio::time::timeout(request.timeout, fut).await.ok()?
}

/// One `codex exec --json` stdout line → its agent message text, if it carries
/// one. Handles both JSONL shapes the turn parser already covers: the legacy
/// `msg.type == "agent_message"` event and the item-style `item.completed`
/// wrapper. Split from the transport so it can be tested without a CLI.
fn exec_line_agent_message(line: &str) -> Option<String> {
    let event = serde_json::from_str::<Value>(line).ok()?;
    let msg = event.get("msg").unwrap_or(&event);
    match msg.get("type").and_then(Value::as_str)? {
        "agent_message" => msg
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string),
        "item.completed" => {
            let item = msg.get("item")?;
            if item.get("type").and_then(Value::as_str) != Some("agent_message") {
                return None;
            }
            item.get("text").and_then(Value::as_str).map(str::to_string)
        }
        _ => None,
    }
}

#[async_trait]
impl Harness for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn name(&self) -> &'static str {
        "Codex"
    }

    fn supports_chat(&self) -> bool {
        true
    }

    /// The app-server takes `turn/steer` against the active turn; `detect`
    /// withholds it from installations that fall back to the exec path.
    fn supports_steering(&self) -> bool {
        true
    }

    async fn detect(&self) -> Option<HarnessInfo> {
        let mut info = HarnessInfo::new(self.id(), self.name());
        if let Some(bin) = find_codex() {
            info.record_bin(&bin, probe_bin(&bin).await);
        }
        let home = native_store::codex_home(NativeStore::Legacy);
        let config_raw = std::fs::read_to_string(home.join("config.toml")).ok();
        let custom_provider = config_raw.as_deref().and_then(parse_custom_provider);
        let configured_effort = config_raw.as_deref().and_then(parse_configured_effort);

        if let Some(provider) = custom_provider.as_ref() {
            // A provider with `requires_openai_auth = false` never writes
            // auth.json; its declared `env_key` is the credential.
            if provider.is_ready() {
                info.authenticated = true;
                info.auth_method = provider.env_key.as_ref().map(|_| "apiKey");
            } else if let Some(key) = provider.env_key.as_deref() {
                info.agent_note = Some(format!(
                    "Set `{key}` for the configured Codex model provider."
                ));
            }
        } else if let Some(auth) = read_json(home.join("auth.json")) {
            if nonempty_str(&auth, "OPENAI_API_KEY").is_some() {
                info.authenticated = true;
                info.auth_method = Some("apiKey");
            }
            if let Some(claims) = auth
                .get("tokens")
                .and_then(|t| t.get("id_token"))
                .and_then(Value::as_str)
                .and_then(jwt_payload)
            {
                info.authenticated = true;
                info.auth_method = Some("oauth");
                info.account = nonempty_str(&claims, "email");
                if let Some(oa) = claims.get("https://api.openai.com/auth") {
                    info.plan = nonempty_str(oa, "chatgpt_plan_type").map(|p| title_case(&p));
                }
            }
        }

        info.agent_ready = info.ready();
        if info.agent_ready {
            // A custom provider's bundled first-party catalog is meaningless,
            // so probe only when its config declares an explicit catalog.
            let custom_catalog = match (
                custom_provider.as_ref(),
                info.bin_path.as_deref().map(Path::new),
            ) {
                (Some(provider), Some(bin)) if provider.has_model_catalog => {
                    codex_model_list(bin, configured_effort.as_deref()).await
                }
                _ => None,
            };
            match custom_provider
                .as_ref()
                .map(|provider| provider.model.as_deref())
            {
                Some(configured_model) => {
                    info =
                        info.with_models(custom_provider_models(configured_model, custom_catalog))
                }
                None => {
                    // First-party account: ask the installed CLI for its own
                    // catalog (models + per-model efforts, the data codex's TUI
                    // picker renders). The static table only covers a codex too
                    // old to answer `model/list`.
                    let bin = info.bin_path.as_deref().map(Path::new);
                    let models = match bin {
                        Some(bin) => codex_model_list(bin, configured_effort.as_deref()).await,
                        None => None,
                    };
                    info = info.with_models(models.unwrap_or_else(|| {
                        CODEX_MODELS
                            .iter()
                            .map(|(id, levels)| ModelInfo::new(*id).with_reasoning(levels))
                            .collect()
                    }));
                }
            }
            // Old CLIs still work via the legacy exec path, but miss the
            // app-server wins (permission prompts on sandbox escalations;
            // thread resume).
            // `turn/steer` is an app-server method, so this must follow the
            // dispatch predicate rather than the version alone.
            info.supports_steering = runs_app_server().await;
            let too_old = info
                .version
                .as_deref()
                .and_then(parse_version)
                .is_some_and(|v| v < MIN_APP_SERVER_VERSION);
            if too_old {
                info.agent_note = Some(
                    "This Codex version chats via the legacy exec path — update to 0.144+ for plan mode & permission prompts.".to_string(),
                );
            }
        } else if info.install_broken {
            // Outranks both notes below: neither signing in nor a provider key
            // helps a codex that can't start.
            info.agent_note = Some(info.broken_note(CODEX_REINSTALL));
        } else if info.agent_note.is_some() {
            // A configured custom provider already said which env var to set;
            // `codex login` is the wrong instruction for it.
        } else if info.installed {
            info.agent_note = Some("Sign in with `codex login` to chat with it here.".to_string());
        } else {
            info.agent_note = Some(
                "Install Codex (developers.openai.com/codex), then sign in with `codex login`."
                    .to_string(),
            );
        }
        Some(info)
    }

    async fn run_turn(&self, ctx: &mut TurnCtx) -> TurnResult {
        if !runs_app_server().await {
            let result = run_turn_exec(ctx).await;
            // Still `Unknown` after a failure means codex died before its first
            // event — nothing was delivered, so the card must offer Retry, not a
            // Continue that re-runs the same doomed spawn.
            if result.is_err() && ctx.delivery_state() == DeliveryState::Unknown {
                ctx.mark_delivery(DeliveryState::NotSent);
            }
            return result
                .map(|()| TurnOutcome::Completed)
                .map_err(|error| TurnFailure::adapter(error, ctx.delivery_state()));
        }
        run_turn_app_server(ctx)
            .await
            .map(|()| TurnOutcome::Completed)
            .map_err(|error| TurnFailure::adapter(error, ctx.delivery_state()))
    }

    async fn one_shot(&self, request: OneShot<'_>) -> Option<String> {
        codex_one_shot(&find_codex()?, request).await
    }

    fn options(&self) -> HarnessOptions {
        // Codex keeps planning on its independent collaboration-mode axis; the
        // dropdown mirrors the desktop app's three permission choices.
        HarnessOptions::none()
            .with_permission_choices(
                vec![
                    OptionChoice::described(
                        "ask",
                        "Ask for approval",
                        "Ask before commands that need elevated access",
                    ),
                    OptionChoice::described(
                        "approve-for-me",
                        "Approve for me",
                        "Codex reviews approval requests automatically",
                    ),
                    OptionChoice::described(
                        "full-access",
                        "Full access",
                        "Run without sandbox or approval prompts",
                    ),
                ],
                "approve-for-me",
                PlanActivation::Command,
            )
            // Harness-wide fallback only — the real per-model lists ride on each
            // `ModelInfo` (see `CODEX_MODELS`). The default is
            // `Default`, so a configured `model_reasoning_effort` in
            // `~/.codex/config.toml` is no longer overridden by an implicit
            // per-turn `high` (issue #123).
            .with_reasoning_levels(&CODEX_REASONING_LEVELS)
    }

    /// Three prompt kinds resume differently:
    ///
    /// * `permission` (native, held mid-turn): the answer is the JSON-RPC
    ///   `{decision}` reply, delivered inline over the live app-server child —
    ///   the still-running turn keeps streaming once codex unblocks
    ///   ([`ResumeAction::Handled`], never the new-message path).
    /// * `question` (native, held mid-turn): a `request_user_input` reply,
    ///   delivered inline the same way (`user_input_reply`).
    /// * `plan` (end-turn card, no `native_id`): resumes by a NEW user message
    ///   ([`ResumeAction::SendMessage`]) — approve sends the implementation
    ///   prompt with Plan cleared while preserving the selected permission;
    ///   the `default` collaborationMode mask exits native Plan. Revise stays in
    ///   Plan (shared `synthesize_resume`); a note-less reject just closes the
    ///   card ([`ResumeAction::Nothing`]).
    async fn resume_from_prompt(
        &self,
        ctx: &ResumeCtx,
        prompt: &WirePrompt,
        answer: &PromptAnswer,
    ) -> Result<ResumeAction> {
        match prompt.kind.as_str() {
            // End-turn plan card (no native_id): resume by message, exactly
            // like Claude's plan card. The fresh turn's collaborationMode mask
            // (`default` on approve/leave, `plan` on revise) is what un-sticks
            // or keeps plan mode — no inline reply, so no busy-check here.
            "plan" => {
                // Note-less reject on an end-turn card: the turn is already over
                // and there's nothing to un-stick with a message — resuming just
                // to say "stop" would end in fresh text that becomes ANOTHER
                // plan card, so it could never dismiss the strip. Close it.
                if !answer.approve && answer.note.as_deref().is_none_or(|s| s.trim().is_empty()) {
                    return Ok(ResumeAction::Nothing);
                }
                let note = answer.note.as_deref().filter(|s| !s.trim().is_empty());
                let (text, plan_mode) = if answer.approve {
                    // Codex's plan template primes the model for "Implement the
                    // plan." — its own proven approval phrasing (the TUI uses
                    // it). Approving leaves Plan without changing permissions;
                    // the fresh turn attaches the `default` mask that un-sticks.
                    let mut text = "Implement the plan.".to_string();
                    if let Some(note) = note {
                        text.push_str(&format!("\n\nAdditional guidance: {note}"));
                    }
                    (text, false)
                } else {
                    // Revise (a note-carrying reject): stay in Plan. Reuse the
                    // shared plan-deny wording so the phrasing matches Claude.
                    (synthesize_resume("plan", answer).0, true)
                };
                Ok(ResumeAction::SendMessage {
                    text,
                    mode: None,
                    plan_mode: Some(plan_mode),
                })
            }
            // Native held cards (permission / question): reply inline over the
            // live child. A reply only lands if the turn is still paused on it —
            // after an interrupt/error the request was already settled and a
            // late reply would be a stale answer into the void. Mirror Claude's
            // zombie collapse so a card left by a crashed turn stops swallowing
            // answers.
            "permission" | "question" => {
                if !ctx.is_busy().await {
                    ctx.host
                        .resolve_zombie_prompt(&ctx.session_id, &answer.prompt_id);
                    return Err(anyhow!(
                        "this turn is no longer running — its prompt can't be answered"
                    ));
                }
                let native = prompt
                    .native_id
                    .as_deref()
                    .ok_or_else(|| anyhow!("codex prompt has no reply id"))?;
                // native_id is the JSON-RPC request id's raw text.
                let rpc_id: Value = serde_json::from_str(native)
                    .map_err(|_| anyhow!("codex prompt reply id is invalid"))?;
                // Build the reply BEFORE reaching the client, so a bad answer
                // (a question with no selection/note) errs before delivery and
                // leaves the card actionable.
                let reply = if prompt.kind == "permission" {
                    serde_json::json!({ "decision": approval_decision(answer.approve) })
                } else {
                    user_input_reply(prompt, answer)?
                };
                let client = ctx
                    .host
                    .codex
                    .client_for(&ctx.session_id)
                    .await
                    .ok_or_else(|| {
                        anyhow!("codex app-server is not running — cannot deliver the reply")
                    })?;
                client.respond(&rpc_id, reply).await?;
                Ok(ResumeAction::Handled { plan_mode: None })
            }
            other => Err(anyhow!("codex cannot reply to a `{other}` prompt")),
        }
    }

    fn config_home(&self) -> Option<PathBuf> {
        Some(native_store::codex_home(NativeStore::Legacy))
    }

    fn skill_target(&self) -> Option<PathBuf> {
        // Codex now speaks native SKILL.md skills (`~/.agents/skills/`); the
        // legacy `~/.codex/prompts/` path is deprecated and model-invisible. The
        // primary target is the real skill; the legacy prompt still rides along
        // via `extra_skill_targets` for older codex versions.
        Some(
            dirs::home_dir()?
                .join(".agents")
                .join("skills")
                .join("orx")
                .join("SKILL.md"),
        )
    }

    fn skill_shim(&self) -> Option<&'static str> {
        // Native SKILL.md format, same body as Claude Code / OpenCode / Cursor.
        Some(super::CLAUDE_SKILL)
    }

    fn extra_skill_targets(&self) -> Vec<(PathBuf, &'static str)> {
        // Keep the legacy `/orx` prompt for codex versions that don't yet read
        // `~/.agents/skills/`.
        vec![(
            native_store::codex_home(NativeStore::Legacy)
                .join("prompts")
                .join("orx.md"),
            super::CODEX_PROMPT,
        )]
    }

    fn session_skills_dir(&self) -> Option<&'static str> {
        Some(".agents/skills")
    }
}

// --- app-server path (codex ≥ 0.144) -----------------------------------------

/// First protocol version the harness was validated against (schema dump +
/// live spike). Older CLIs take the exec fallback below.
const MIN_APP_SERVER_VERSION: (u64, u64, u64) = (0, 144, 0);

/// Whether a turn will run over the app-server: a supported codex, unless
/// ORX_CODEX_EXEC forces the legacy exec path ("0"/empty don't count).
/// Capability reporting reads the same answer, so the composer can't offer
/// app-server-only features the exec path lacks.
async fn runs_app_server() -> bool {
    let force_exec = std::env::var("ORX_CODEX_EXEC").is_ok_and(|v| !v.is_empty() && v != "0");
    !force_exec && app_server_supported().await
}

/// Whether the installed codex speaks the validated app-server protocol.
/// Probed once per process (a codex upgrade mid-run takes an `orx up` restart
/// to notice — acceptable).
async fn app_server_supported() -> bool {
    static SUPPORTED: tokio::sync::OnceCell<bool> = tokio::sync::OnceCell::const_new();
    *SUPPORTED
        .get_or_init(|| async {
            let Some(bin) = find_codex() else {
                return false;
            };
            bin_version(&bin)
                .await
                .as_deref()
                .and_then(parse_version)
                .is_some_and(|v| v >= MIN_APP_SERVER_VERSION)
        })
        .await
}

/// Session mode → thread sandbox, approval policy, and approval reviewer.
/// Custom providers stay human-reviewed because Codex's hidden auto-review
/// model is not generally available through third-party model endpoints.
fn codex_policies(
    mode: Option<PermissionMode>,
    auto_review_supported: bool,
) -> (&'static str, &'static str, &'static str) {
    match mode.unwrap_or(PermissionMode::Auto) {
        PermissionMode::Bypass => ("danger-full-access", "never", "user"),
        // Plan runs the SAME sandbox as Auto (workspace-write + on-request).
        // Native plan mode restricts *at the prompt level* — codex's built-in
        // plan.md template tells the model to propose without editing — not at
        // the sandbox level, so this is the parity gap vs Claude's hook-gated
        // plan mode: an off-script write inside the workspace would not prompt
        // (user-accepted). This arm is the variation point if we ever want a
        // harder read-only floor: change this arm's sandbox to `read-only`.
        // AcceptEdits/Ask still collapse to the balanced
        // default (mirrors `codex_sandbox` on the exec path).
        PermissionMode::Plan => ("workspace-write", "on-request", "user"),
        PermissionMode::Auto if auto_review_supported => {
            ("workspace-write", "on-request", "auto_review")
        }
        _ => ("workspace-write", "on-request", "user"),
    }
}

fn config_supports_auto_review(response: &Value) -> bool {
    let Some(config) = response.get("config").and_then(Value::as_object) else {
        return false;
    };
    let first_party_provider = match config.get("model_provider") {
        Some(Value::Null) => true,
        Some(Value::String(provider)) => provider == "openai",
        _ => false,
    };
    let first_party_url = match config.get("openai_base_url") {
        Some(Value::Null) => true,
        Some(Value::String(url)) => url.trim().is_empty(),
        _ => false,
    };
    first_party_provider && first_party_url
}

async fn codex_auto_review_supported(client: &CodexClient, workspace: &Path) -> bool {
    let Ok(Ok(response)) = client
        .try_request(
            "config/read",
            serde_json::json!({
                "cwd": workspace.to_string_lossy(),
                "includeLayers": false,
            }),
        )
        .await
    else {
        return false;
    };
    config_supports_auto_review(&response) && super::detect::api_key("OPENAI_BASE_URL").is_none()
}

/// The per-turn `sandboxPolicy` object. workspace-write carries the same
/// grants the exec path passed via `-c`: the orx data dir, its lifecycle lock,
/// and the hub clone's `.git` as writable roots (see the helpers below), plus
/// network (the agent's job is driving the orx API and git). Like the exec `-c`
/// override, this is a full policy replacement for the turn — a user's own
/// config.toml `sandbox_workspace_write.writable_roots` don't survive it (no
/// append form exists on either transport).
async fn sandbox_policy_json(mode: Option<PermissionMode>, workspace: &Path) -> Value {
    match mode.unwrap_or(PermissionMode::Auto) {
        PermissionMode::Bypass => serde_json::json!({ "type": "dangerFullAccess" }),
        _ => {
            let mut roots: Vec<String> = Vec::new();
            roots.extend(ensure_orx_data_dir().map(|p| p.to_string_lossy().into_owned()));
            roots.extend(ensure_orx_lifecycle_lock().map(|p| p.to_string_lossy().into_owned()));
            roots.extend(
                shared_git_dir(workspace)
                    .await
                    .map(|p| p.to_string_lossy().into_owned()),
            );
            serde_json::json!({
                "type": "workspaceWrite",
                "writableRoots": roots,
                "networkAccess": true,
            })
        }
    }
}

/// The per-turn `collaborationMode` mask (experimental API). Codex's native
/// plan mode is a *collaboration mode*, not a sandbox setting: `plan` injects
/// codex's built-in plan.md template and enables `request_user_input`; `default`
/// injects the Default template. Attaching a mask is never free — even
/// `{mode:"default"}` on a fresh (template-less) thread INJECTS the Default
/// template (verified in the 0.144 spike) — so the caller attaches this only
/// when it actually wants a template (see `run_turn_app_server`).
///
/// Envelope keys are camelCase (`collaborationMode`), `settings` keys snake_case
/// (`reasoning_effort`, `developer_instructions`). `model` is REQUIRED. The
/// built-in template rides `developer_instructions: null`; it's an independent
/// channel from the thread-level `developerInstructions` playbook, so the
/// playbook is never disturbed by leaving this null.
fn collaboration_mode_json(mode: &str, model: &str, effort: Option<&str>) -> Value {
    let mut settings = serde_json::Map::new();
    settings.insert("model".to_string(), Value::String(model.to_string()));
    if let Some(effort) = effort {
        settings.insert(
            "reasoning_effort".to_string(),
            Value::String(effort.to_string()),
        );
    }
    settings.insert("developer_instructions".to_string(), Value::Null);
    serde_json::json!({ "mode": mode, "settings": Value::Object(settings) })
}

fn collaboration_mask_mode(
    plan_mode: bool,
    reset_pending: bool,
    last_mode: Option<&str>,
) -> Option<&'static str> {
    if plan_mode {
        Some("plan")
    } else if reset_pending || last_mode == Some("plan") {
        Some("default")
    } else {
        None
    }
}

/// How a turn ended, from `turn/completed`.
enum TurnEnd {
    /// Completed or interrupted. `interrupted` drives whether an end-turn plan
    /// card is synthesized — an interrupted plan turn has no finished plan.
    Done {
        interrupted: bool,
    },
    Failed(String),
}

/// One app-server notification → transcript state. Pure (fixture-tested):
/// touches only `ctx.assistant.parts` via the TurnCtx helpers. Returns the
/// turn's terminal state when this event ends it.
fn apply_notification(ctx: &mut TurnCtx, method: &str, params: &Value) -> Option<TurnEnd> {
    if method != "error" {
        ctx.clear_retry_status();
    }
    match method {
        "item/started" | "item/completed" => {
            if let Some(item) = params.get("item") {
                apply_item(ctx, item, method == "item/completed");
            }
        }
        "item/agentMessage/delta" => {
            append_delta(ctx, params, |id| WirePart::text(id, ""));
        }
        // GPT-5 reasoning streams summaries; raw content deltas are the
        // fallback shape. Only one of the two fires per item in practice.
        "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
            append_delta(ctx, params, |id| WirePart::reasoning(id, ""));
        }
        // Plan mode streams the finished plan token-by-token before the
        // completed `plan` item lands. Rendered as a plain markdown text part
        // (WirePart kinds are text|reasoning|tool|prompt) under a derived id so
        // the completed item upserts the same part. The end-turn plan card then
        // reads this part's text as the authoritative plan.
        "item/plan/delta" => {
            let plan_delta = |id: String| WirePart::text(id, "");
            if let Some(item_id) = params.get("itemId").and_then(Value::as_str) {
                let part_id = plan_part_id(item_id);
                if !part_exists(ctx, &part_id) {
                    ctx.upsert_part(plan_delta(part_id.clone()));
                }
                if let Some(delta) = params.get("delta").and_then(Value::as_str) {
                    ctx.append_part_text(&part_id, delta);
                }
            }
        }
        "item/commandExecution/outputDelta" => {
            let (Some(item_id), Some(delta)) = (
                params.get("itemId").and_then(Value::as_str),
                params.get("delta").and_then(Value::as_str),
            ) else {
                return None;
            };
            // Deltas can beat `item/started`; a placeholder part (command
            // unknown yet) is corrected by the later item events.
            if !part_exists(ctx, item_id) {
                ctx.upsert_part(tool_part(
                    item_id.to_string(),
                    "bash",
                    "running",
                    Some(serde_json::json!({ "command": "" })),
                    None,
                ));
            }
            if let Some(part) = ctx.assistant.parts.iter_mut().find(|p| p.id == item_id) {
                if let Some(state) = part.state.as_mut() {
                    let output = state.output.get_or_insert_with(String::new);
                    output.push_str(delta);
                }
            }
        }
        "error" => {
            // Transient errors are retried by codex itself (willRetry); only
            // terminal ones reach the transcript.
            let will_retry = params
                .get("willRetry")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if will_retry {
                ctx.show_retry_status("native", "Codex CLI is retrying", 1, None, None);
            } else {
                ctx.mark_native_retry_exhausted();
                ctx.clear_retry_status();
                let mut message = error_message(params.get("error"));
                if let Some(info) = params.get("codexErrorInfo").or_else(|| {
                    params
                        .get("error")
                        .and_then(|error| error.get("codexErrorInfo"))
                }) {
                    message.push_str(&format!("\n\ncodexErrorInfo: {info}"));
                }
                ctx.mark_terminal_failure("codex_terminal", message.clone());
                ctx.push_error(message);
            }
        }
        // Codex 0.144 emits this before the typed review event; ignoring it avoids duplicate rows.
        "guardianWarning" => {}
        "item/autoApprovalReview/started" | "item/autoApprovalReview/completed" => {
            if let Some(message) = guardian_review_failure(params) {
                ctx.push_error(message);
            }
        }
        "turn/completed" => {
            let turn = params.get("turn").unwrap_or(&Value::Null);
            // Usage may sit under `turn.usage` or top-level `params.usage`
            // depending on the app-server version; probe both.
            let usage = turn
                .get("usage")
                .filter(|v| !v.is_null())
                .or_else(|| params.get("usage"));
            if let Some(used) = codex_used_tokens(usage) {
                let context_window = turn
                    .get("model_context_window")
                    .or_else(|| params.get("model_context_window"))
                    .and_then(Value::as_u64);
                ctx.report_usage(ContextUsage {
                    used_tokens: used,
                    context_window,
                });
            }
            let status = turn.get("status").and_then(Value::as_str).unwrap_or("");
            if status == "failed" {
                ctx.mark_native_retry_exhausted();
                let mut message = error_message(turn.get("error"));
                if let Some(info) = turn.get("codexErrorInfo").or_else(|| {
                    turn.get("error")
                        .and_then(|error| error.get("codexErrorInfo"))
                }) {
                    message.push_str(&format!("\n\ncodexErrorInfo: {info}"));
                }
                ctx.mark_terminal_failure("codex_terminal", message.clone());
                return Some(TurnEnd::Failed(message));
            }
            // Defensive: the pins say turn/completed carries a final status,
            // but a non-final one must not truncate the turn if codex ever
            // regresses.
            if status == "inProgress" {
                return None;
            }
            return Some(TurnEnd::Done {
                interrupted: status == "interrupted",
            });
        }
        _ => {}
    }
    None
}

/// Append a streamed delta to its part, creating the (empty) part on the
/// first delta — deltas can arrive before we see `item/started`.
fn append_delta(ctx: &mut TurnCtx, params: &Value, make: impl FnOnce(String) -> WirePart) {
    let (Some(item_id), Some(delta)) = (
        params.get("itemId").and_then(Value::as_str),
        params.get("delta").and_then(Value::as_str),
    ) else {
        return;
    };
    if !part_exists(ctx, item_id) {
        ctx.upsert_part(make(item_id.to_string()));
    }
    ctx.append_part_text(item_id, delta);
}

/// Whether the assistant message already carries a part with this id.
fn part_exists(ctx: &TurnCtx, id: &str) -> bool {
    ctx.assistant.parts.iter().any(|p| p.id == id)
}

/// A tool-flavored WirePart (bash / edit) in one of the three statuses.
fn tool_part(
    id: String,
    tool: &str,
    status: &str,
    input: Option<Value>,
    output: Option<String>,
) -> WirePart {
    WirePart {
        id,
        kind: "tool".into(),
        text: None,
        tool: Some(tool.into()),
        state: Some(WireToolState {
            status: status.into(),
            input,
            output,
            error: None,
            title: None,
        }),
        prompt: None,
        children: Vec::new(),
    }
}

/// running / error / completed for a (possibly still-open) tool item.
fn tool_status(completed: bool, failed: bool) -> &'static str {
    if !completed {
        "running"
    } else if failed {
        "error"
    } else {
        "completed"
    }
}

/// A ThreadItem (from `item/started` / `item/completed`) → WirePart, applied to
/// the parent transcript. Thin wrapper over the pure [`item_to_part`]: it owns
/// the streaming-merge guards that need `ctx` (never wipe a streamed part with
/// an empty final text), then upserts. The sub-agent path calls `item_to_part`
/// directly against its own bucket (see the turn loop's routing).
fn apply_item(ctx: &mut TurnCtx, item: &Value, completed: bool) {
    let Some(part) = item_to_part(item, completed, &ctx.assistant.parts) else {
        return;
    };
    // agentMessage / reasoning / plan stream via deltas before the completed
    // item lands; a completed item with empty text must not wipe what the
    // deltas built. `item_to_part` produces the part with its final id (plan
    // uses `plan_part_id`), so the guard keys off that id.
    if completed
        && streamed_text_kind(item)
        && part_text_is_empty(&part)
        && part_exists(ctx, &part.id)
    {
        return;
    }
    upsert_preserving_children(&mut ctx.assistant.parts, part);
}

async fn reconcile_turn_items(
    ctx: &mut TurnCtx,
    client: &CodexClient,
    thread_id: &str,
    turn_id: Option<&str>,
) {
    let Some(turn_id) = turn_id else { return };
    let Some(items) = client.read_turn_items(thread_id, turn_id).await else {
        return;
    };
    reconcile_items(&mut ctx.assistant.parts, &items);
}

pub(crate) fn reconcile_interrupted_items(
    session_id: &str,
    message_id: &str,
    items: &[Value],
) -> Option<WireMessage> {
    let store = Store::open().ok()?;
    let stored = store
        .list_chat_messages(session_id)
        .ok()?
        .into_iter()
        .find(|message| message.id == message_id)?;
    let mut message = crate::local::chat::stored_to_wire(&stored);
    reconcile_items(&mut message.parts, items);
    settle_interrupted_parts(&mut message.parts);
    store
        .upsert_chat_message(&StoredChatMessage {
            parts_json: serde_json::to_string(&message.parts).ok()?,
            ..stored.clone()
        })
        .ok()?;
    Some(message)
}

fn reconcile_items(parts: &mut Vec<WirePart>, items: &[Value]) {
    // History can repeat identical text; consume-once pairing keeps the Nth
    // repeat restorable instead of collapsing every copy onto one part.
    let mut claimed: Vec<String> = Vec::new();
    for item in items {
        let completed = item.get("status").and_then(Value::as_str) != Some("inProgress");
        let Some(part) = item_to_part(item, completed, parts) else {
            continue;
        };
        let id_exists = parts.iter().any(|prior| prior.id == part.id);
        if completed && streamed_text_kind(item) && part_text_is_empty(&part) && id_exists {
            continue;
        }
        // `thread/read` re-mints ids (`item-N`) for content the live stream
        // keyed by its Responses id (`msg_…`/`rs_…`), so an unknown id whose
        // text continues a non-empty same-kind part is that part's item
        // renamed, not new content (OR-181) — adopt the authoritative text in
        // place. An empty part carries no evidence of which item it is.
        if !id_exists && renamable_text_kind(item) {
            let incoming = part.text.as_deref().unwrap_or("");
            let renamed = parts.iter().position(|prior| {
                let prior_text = prior.text.as_deref().unwrap_or("");
                !claimed.contains(&prior.id)
                    && !prior.id.starts_with("plan-item-")
                    && prior.kind == part.kind
                    && !prior_text.is_empty()
                    && incoming.starts_with(prior_text)
            });
            if let Some(i) = renamed {
                claimed.push(parts[i].id.clone());
                parts[i].text = part.text;
                continue;
            }
        }
        upsert_preserving_children(parts, part);
    }
}

/// Item types whose ids `thread/read` re-mints (see `reconcile_items`). Plan
/// streams text too, but its part id is derived (`plan_part_id`), so history
/// never renames it — and the `plan-item-` prior check is the other half of
/// that exemption.
fn renamable_text_kind(item: &Value) -> bool {
    matches!(
        item.get("type").and_then(Value::as_str),
        Some("agentMessage") | Some("reasoning")
    )
}

fn settle_interrupted_parts(parts: &mut [WirePart]) {
    for part in parts {
        settle_interrupted_parts(&mut part.children);
        if let Some(state) = part.state.as_mut() {
            if state.status == "running" {
                state.status = "interrupted".into();
            }
        }
    }
}

/// The three item types whose text streams token-by-token via `item/*/delta`
/// before the completed item arrives (agentMessage, reasoning, plan). For these,
/// a completed item carrying empty text must not clobber the streamed part.
fn streamed_text_kind(item: &Value) -> bool {
    matches!(
        item.get("type").and_then(Value::as_str),
        Some("agentMessage") | Some("reasoning") | Some("plan")
    )
}

/// Whether a built part carries no display text (its `text` is absent/empty).
fn part_text_is_empty(part: &WirePart) -> bool {
    part.text.as_deref().unwrap_or("").is_empty()
}

/// A ThreadItem → WirePart, **pure** (no `ctx`, no streaming merge). Returns
/// `None` for items that render nothing (userMessage / hookPrompt). `prior` is
/// the parts the result will land among — only `commandExecution` reads it, to
/// preserve streamed `outputDelta` text a completed item without
/// `aggregatedOutput` would otherwise drop; callers with no prior pass `&[]`.
///
/// The returned part carries its **final** id: plain item id for most types,
/// the derived `plan_part_id` for `plan`. Callers namespacing sub-agent ids
/// prefix `part.id` after the fact.
fn item_to_part(item: &Value, completed: bool, prior: &[WirePart]) -> Option<WirePart> {
    let id = item.get("id").and_then(Value::as_str).map(str::to_string)?;
    match item.get("type").and_then(Value::as_str) {
        Some("agentMessage") => {
            let text = item.get("text").and_then(Value::as_str).unwrap_or("");
            Some(WirePart::text(id, text))
        }
        Some("reasoning") => {
            let text = reasoning_text(item);
            Some(WirePart::reasoning(id, &text))
        }
        Some("commandExecution") => {
            let failed = completed
                && (!matches!(
                    item.get("status").and_then(Value::as_str),
                    Some("completed")
                ) || item
                    .get("exitCode")
                    .and_then(Value::as_i64)
                    .is_some_and(|c| c != 0));
            // Streamed output (outputDelta) survives a completed item without
            // aggregatedOutput; when present, aggregatedOutput is authoritative.
            let output = item
                .get("aggregatedOutput")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    prior
                        .iter()
                        .find(|p| p.id == id)
                        .and_then(|p| p.state.as_ref())
                        .and_then(|s| s.output.clone())
                });
            let mut input = serde_json::json!({
                "command": item.get("command").map(command_string).unwrap_or_default(),
            });
            if let Some(argv) = item.get("command").and_then(command_argv) {
                input["commandArgv"] = serde_json::json!(argv);
            }
            if let Some(cwd) = item.get("cwd").and_then(Value::as_str) {
                input["cwd"] = Value::String(cwd.to_string());
            }
            if let Some(prior_input) = prior
                .iter()
                .find(|part| part.id == id)
                .and_then(|part| part.state.as_ref())
                .and_then(|state| state.input.as_ref())
            {
                for key in [
                    "runTargetIds",
                    "runTargetIdsAuthoritative",
                    "experimentTargetIds",
                    "experimentTargetIdsAuthoritative",
                    "targetIds",
                ] {
                    if let Some(value) = prior_input.get(key) {
                        input[key] = value.clone();
                    }
                }
            }
            Some(tool_part(
                id,
                "bash",
                tool_status(completed, failed),
                Some(input),
                output,
            ))
        }
        Some("fileChange") => {
            let failed = completed
                && !matches!(
                    item.get("status").and_then(Value::as_str),
                    Some("completed")
                );
            let input = item
                .get("changes")
                .cloned()
                .map(|c| serde_json::json!({ "changes": c }));
            Some(tool_part(
                id,
                "edit",
                tool_status(completed, failed),
                input,
                None,
            ))
        }
        Some("plan") => {
            // Keyed on the derived plan part id so the streamed
            // `item/plan/delta` parts and this completed item upsert the same
            // part (and `plan_card` can find the authoritative plan text).
            let text = item.get("text").and_then(Value::as_str).unwrap_or("");
            Some(WirePart::text(plan_part_id(&id), text))
        }
        Some("webSearch") => {
            // No status field on webSearch — it only fails if the whole turn
            // errors. The tool name "WebSearch" matches the UI's case.
            // `query` is empty for non-search actions (openPage, findInPage);
            // the `action` union carries the url/pattern, so its fields are
            // merged into the input for the UI to label the row.
            let query = item.get("query").and_then(Value::as_str).unwrap_or("");
            let mut input = item
                .get("action")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            if !query.is_empty() || !input.contains_key("query") {
                input.insert("query".into(), Value::String(query.to_string()));
            }
            Some(tool_part(
                id,
                "WebSearch",
                tool_status(completed, false),
                Some(Value::Object(input)),
                None,
            ))
        }
        Some("mcpToolCall") => {
            let status = item.get("status").and_then(Value::as_str);
            let failed = completed && status != Some("completed");
            let server = item.get("server").and_then(Value::as_str).unwrap_or("");
            let tool = item.get("tool").and_then(Value::as_str).unwrap_or("");
            let name = if server.is_empty() && tool.is_empty() {
                "mcp".to_string()
            } else {
                format!("{server}:{tool}")
            };
            let input = serde_json::json!({
                "arguments": item.get("arguments").cloned().unwrap_or(Value::Null),
            });
            // Prefer the error (when failed) then the result (when completed).
            let output = if failed {
                item.get("error").map(value_to_pretty)
            } else if completed {
                item.get("result").map(value_to_pretty)
            } else {
                None
            };
            Some(tool_part(
                id,
                &name,
                tool_status(completed, failed),
                Some(input),
                output,
            ))
        }
        Some("dynamicToolCall") => {
            let status = item.get("status").and_then(Value::as_str);
            let success = item.get("success").and_then(Value::as_bool);
            let failed = completed && (status != Some("completed") || success == Some(false));
            let tool = item.get("tool").and_then(Value::as_str).unwrap_or("tool");
            let name = match item.get("namespace").and_then(Value::as_str) {
                Some(ns) if !ns.is_empty() => format!("{ns}:{tool}"),
                _ => tool.to_string(),
            };
            let input = serde_json::json!({
                "arguments": item.get("arguments").cloned().unwrap_or(Value::Null),
            });
            let output = item.get("contentItems").map(value_to_pretty);
            Some(tool_part(
                id,
                &name,
                tool_status(completed, failed),
                Some(input),
                output,
            ))
        }
        // The Codex collaboration items that spawn / drive a sub-agent. Rendered
        // as a first-class "subagent" spawn part; the turn loop hangs the
        // sub-agent's own streamed transcript under its `children`, and the UI
        // labels the row from `state.input` (tool/prompt/kind).
        Some("collabAgentToolCall") | Some("subAgentActivity") => {
            Some(subagent_spawn_part(&id, item, completed))
        }
        // userMessage / hookPrompt echo *input* (the user's own message / the
        // hook-injected prompt fragments), not model activity — rendering them
        // would duplicate the user bubble.
        Some("userMessage") | Some("hookPrompt") => None,
        // Generic fallback so nothing is silently swallowed: any other item
        // type (imageView, sleep, imageGeneration, review mode,
        // contextCompaction, or a future protocol addition) renders as a tool
        // part named after its raw type.
        other => {
            let tool = other.unwrap_or("item");
            let status = item.get("status").and_then(Value::as_str);
            // Positive `== failed` (not `!= completed`) so status-less types
            // like contextCompaction render completed, not error.
            let failed = status == Some("failed");
            // Input = the item object minus `id`/`type`; None when nothing left.
            let input = item
                .as_object()
                .map(|obj| {
                    let mut map = obj.clone();
                    map.remove("id");
                    map.remove("type");
                    map
                })
                .filter(|m| !m.is_empty())
                .map(Value::Object);
            Some(tool_part(
                id,
                tool,
                tool_status(completed, failed),
                input,
                None,
            ))
        }
    }
}

/// Thread ids of the sub-agents a `collabAgentToolCall` / `subAgentActivity`
/// item references. Spawn/send/etc. carry the target(s) in `receiverThreadIds`;
/// `subAgentActivity` carries the single `agentThreadId`.
fn subagent_thread_ids(item: &Value) -> Vec<String> {
    if let Some(arr) = item.get("receiverThreadIds").and_then(Value::as_array) {
        return arr
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
    }
    item.get("agentThreadId")
        .and_then(Value::as_str)
        .map(|t| vec![t.to_string()])
        .unwrap_or_default()
}

/// Build the "subagent" spawn part for a collab/sub-activity item. The UI reads
/// `state.input` to label the row ("Spawned agent", "Sub-agent started", …); the
/// sub-agent's streamed transcript is hung under `children` by the turn loop,
/// and the UI locates it via this part's id, not any thread id in the payload.
fn subagent_spawn_part(id: &str, item: &Value, completed: bool) -> WirePart {
    // collabAgentToolCall carries a `status` (inProgress|completed|failed);
    // subAgentActivity has no status — treat it as a completed marker row.
    let status = item.get("status").and_then(Value::as_str);
    let failed = status == Some("failed");
    let running = status == Some("inProgress")
        || (!completed && status.is_none() && item.get("kind").is_none());
    let wire_status = if running {
        "running"
    } else if failed {
        "error"
    } else {
        "completed"
    };
    // Surface only what the UI labels the row from (`toolLine`'s subagent arm
    // reads `tool` / `prompt` / `kind` / `nickname`) — the transcript is
    // located via the spawn part id + `children`, not via any thread id in the
    // payload.
    let mut input = serde_json::Map::new();
    for key in ["tool", "prompt", "kind"] {
        if let Some(v) = item.get(key) {
            input.insert(key.into(), v.clone());
        }
    }
    // The model-assigned agent identity — the row's best label. Collab items
    // carry it on the first receiver agent; activity items on the agent path,
    // whose last segment is the agent name.
    let nickname = item
        .get("receiverAgents")
        .and_then(Value::as_array)
        .and_then(|agents| agents.first())
        .and_then(|agent| {
            agent
                .get("agentNickname")
                .or_else(|| agent.get("agentRole"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            item.get("agentPath")
                .and_then(Value::as_str)
                .and_then(|path| path.rsplit('/').next())
                .filter(|name| !name.is_empty() && *name != "root")
        });
    if let Some(nickname) = nickname {
        input.insert("nickname".into(), nickname.into());
    }
    tool_part(
        id.to_string(),
        "subagent",
        wire_status,
        Some(Value::Object(input)),
        None,
    )
    // NB: `children` starts empty here. When this spawn part is re-upserted
    // (item/started → item/completed), the upsert must carry forward any
    // children the sub-agent transcript accrued — see `upsert_preserving_children`.
}

// --- sub-agent event routing ---------------------------------------------------
//
// A Codex sub-agent runs as its own thread but streams over the same app-server
// connection, during the parent turn, with its own `turnId`. The parent turn
// loop drops foreign-turn events (see `event_turn_mismatch`) — which is correct
// for an aborted *predecessor parent* turn, but would also drop a live
// sub-agent's transcript. We keep that drop for the predecessor case and, for a
// thread we know is a sub-agent spawned this turn, route its items/deltas into
// the spawning part's `children` instead.

/// Shared tail of every successfully-completed turn: sweep still-open request
/// cards, reconcile final item states, settle orphaned spawn rows, synthesize
/// the plan card when applicable, and flush. Kept in one place so the three
/// exit paths (plain completion, drain completion, drain quiet-settle) can't
/// drift.
async fn finish_completed_turn(
    ctx: &mut TurnCtx,
    client: &CodexClient,
    thread_id: &str,
    turn_id: Option<&str>,
    plan_card_wanted: bool,
    open_requests: &mut HashMap<String, (Value, ServerReqKind)>,
) {
    sweep_open_requests(ctx, client, open_requests).await;
    reconcile_turn_items(ctx, client, thread_id, turn_id).await;
    // A sub-agent whose `turn/completed` never arrived before the turn ended
    // would otherwise spin forever.
    settle_running_subagents(&mut ctx.assistant.parts);
    if plan_card_wanted {
        if let Some(part) = plan_card(&ctx.assistant.parts, &ctx.assistant.id) {
            ctx.upsert_part(part);
        }
    }
    let _ = ctx.flush();
}

/// How long a quiet post-parent drain waits before settling the turn. The
/// parent's turn is already complete, so only actively-streaming agents
/// justify waiting; settling early degrades gracefully because codex delivers
/// a finished agent's report on the next turn either way.
const DRAIN_QUIET_SETTLE: Duration = Duration::from_secs(180);

/// How long a turn may stay quiet before the loop acts on it. Held absolute by
/// the caller so steering can't push either bound back.
fn turn_phase_quiet(parent_done: bool) -> Duration {
    if parent_done {
        DRAIN_QUIET_SETTLE
    } else {
        TURN_WATCHDOG
    }
}

/// A sub-agent thread discovered this parent turn, keyed by its threadId.
struct SubThread {
    /// The `subagent` spawn part (anywhere in the tree) that owns this thread's
    /// transcript. Its `children` is the bucket the thread's parts stream into.
    spawn_part_id: String,
    /// Still running: no terminal `turn/completed` seen for this thread yet.
    /// Spawned agents outlive the parent turn (codex delivers their reports on
    /// the NEXT turn), so the parent's `turn/completed` doesn't end our turn
    /// while any of these are live — see the drain in `run_turn_app_server`.
    live: bool,
}

/// Where an incoming notification/request should be routed.
enum EventScope {
    /// Belongs to the parent turn — the existing path.
    Parent,
    /// Belongs to a known sub-agent thread — route into its bucket.
    SubAgent(String),
    /// Foreign turn we don't track (an aborted predecessor's tail) — drop.
    Stale,
}

/// Classify an event by its `threadId`/`turnId`. Parent-turn events are
/// `Parent`; events on a registered sub-agent thread are `SubAgent`; everything
/// else is `Stale` (dropped, exactly as before this feature).
fn classify_event_thread(
    parent_turn: Option<&str>,
    sub_threads: &HashMap<String, SubThread>,
    params: &Value,
) -> EventScope {
    // Fast path: same turn as the parent → Parent (unchanged behavior, and it
    // also covers events with no turnId, which `event_turn_mismatch` passed).
    if !event_turn_mismatch(parent_turn, params) {
        return EventScope::Parent;
    }
    // Foreign turn: a sub-agent we spawned, or a stale predecessor?
    match params.get("threadId").and_then(Value::as_str) {
        Some(tid) if sub_threads.contains_key(tid) => EventScope::SubAgent(tid.to_string()),
        _ => EventScope::Stale,
    }
}

/// The sub-agent equivalent of `apply_notification`, routing into `bucket` (the
/// spawning part's `children`). Returns the discovered grandchild thread ids (a
/// sub-agent spawning its own sub-agents) and their owning spawn part id, so the
/// caller can register them. Never ends the parent turn, never reports usage.
fn apply_sub_notification(
    bucket: &mut Vec<WirePart>,
    tid: &str,
    method: &str,
    params: &Value,
) -> Vec<DiscoveredSubThread> {
    let mut discovered = Vec::new();
    match method {
        "item/started" | "item/completed" => {
            if let Some(item) = params.get("item") {
                let completed = method == "item/completed";
                let mut scoped_item = item.clone();
                if let Some(id) = item.get("id").and_then(Value::as_str) {
                    scoped_item["id"] = Value::String(namespaced_part_id(tid, id));
                }
                if let Some(part) = item_to_part(&scoped_item, completed, bucket) {
                    // A grandchild spawn: register its threads under this part.
                    if part.tool.as_deref() == Some("subagent") {
                        for gtid in subagent_thread_ids(item) {
                            discovered.push(DiscoveredSubThread {
                                thread_id: gtid,
                                spawn_part_id: part.id.clone(),
                                arms: item_arms_thread(item),
                            });
                        }
                    }
                    let completed_streamed =
                        completed && streamed_text_kind(item) && part_text_is_empty(&part);
                    if completed_streamed && bucket.iter().any(|p| p.id == part.id) {
                        // Don't wipe streamed deltas with an empty final.
                    } else {
                        upsert_preserving_children(bucket, part);
                    }
                }
            }
        }
        "item/agentMessage/delta" => {
            append_delta_into(bucket, tid, params, |id| WirePart::text(id, ""));
        }
        "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
            append_delta_into(bucket, tid, params, |id| WirePart::reasoning(id, ""));
        }
        "item/commandExecution/outputDelta" => {
            if let (Some(item_id), Some(delta)) = (
                params.get("itemId").and_then(Value::as_str),
                params.get("delta").and_then(Value::as_str),
            ) {
                let pid = namespaced_part_id(tid, item_id);
                if !bucket.iter().any(|p| p.id == pid) {
                    bucket.push(tool_part(
                        pid.clone(),
                        "bash",
                        "running",
                        Some(serde_json::json!({ "command": "" })),
                        None,
                    ));
                }
                if let Some(part) = bucket.iter_mut().find(|p| p.id == pid) {
                    if let Some(state) = part.state.as_mut() {
                        state.output.get_or_insert_with(String::new).push_str(delta);
                    }
                }
            }
        }
        // A sub-agent's own turn/completed / error / other notifications don't
        // add transcript parts here (`route_sub_event` handles the thread's
        // terminal turn notifications and mirrors liveness onto the spawn
        // part), and crucially never end the parent turn.
        _ => {}
    }
    discovered
}

/// Namespace a sub-agent's part id by its threadId — codex item ids restart per
/// thread, so a bare id could collide with a parent-thread part.
fn namespaced_part_id(thread_id: &str, item_id: &str) -> String {
    format!("{thread_id}:{item_id}")
}

/// Register any sub-agent threads a `collabAgentToolCall`/`subAgentActivity`
/// item references, keyed to the spawn part (the item's own id). Idempotent —
/// re-seeing the item (started→completed) just re-points to the same part.
/// `spawn_part_id` is namespaced when the collab item itself belongs to a
/// sub-agent (a grandchild spawn), plain for a top-level parent spawn.
fn register_sub_threads_from(
    parent_thread: &str,
    method: &str,
    params: &Value,
    sub_threads: &mut HashMap<String, SubThread>,
) {
    if method != "item/started" && method != "item/completed" {
        return;
    }
    let Some(item) = params.get("item") else {
        return;
    };
    if !matches!(
        item.get("type").and_then(Value::as_str),
        Some("collabAgentToolCall") | Some("subAgentActivity")
    ) {
        return;
    }
    let Some(spawn_id) = item.get("id").and_then(Value::as_str) else {
        return;
    };
    // Re-point (not just first-write): a later collab item on the same thread —
    // `sendInput`/`resumeAgent` after the initial spawn — should own the
    // thread's continued transcript, so its activity streams under the new row
    // rather than the original (already-completed) spawn row. Re-firing the same
    // spawn item (started→completed) re-points to the same id: a harmless no-op.
    for tid in subagent_thread_ids(item) {
        // NEVER the parent's own thread: a handoff/interaction item can
        // reference it, and registering it would add a "sub-agent" that can
        // never retire — the post-parent drain would wait on it forever.
        if tid == parent_thread {
            continue;
        }
        register_sub_thread(
            sub_threads,
            tid,
            spawn_id.to_string(),
            item_arms_thread(item),
        );
    }
}

/// A sub-agent thread referenced by a collab item, with whether that item
/// drives it (see `item_arms_thread`).
struct DiscoveredSubThread {
    thread_id: String,
    spawn_part_id: String,
    arms: bool,
}

/// Insert/re-point one sub-agent thread. Ownership (which spawn row the
/// transcript streams into) always re-points to the newest item; liveness is
/// a separate axis: a driving item (spawn / sendInput / resume / a `started`
/// activity) ARMS it — even for a thread that already retired, so a resumed
/// agent's continuation is drained — while a passive item (wait, close, an
/// interaction/interruption marker) neither invents liveness for an unknown
/// thread (a next-turn report marker must not stall the drain) nor retires a
/// thread that is still live (a `wait` over running agents must not clear
/// them). Only the thread's own `turn/completed` retires it.
fn register_sub_thread(
    sub_threads: &mut HashMap<String, SubThread>,
    thread_id: String,
    spawn_part_id: String,
    arms: bool,
) {
    let live = arms || sub_threads.get(&thread_id).is_some_and(|s| s.live);
    sub_threads.insert(
        thread_id,
        SubThread {
            spawn_part_id,
            live,
        },
    );
}

/// Whether a collab item DRIVES its referenced threads (starts or re-drives an
/// agent), as opposed to passively referencing them.
fn item_arms_thread(item: &Value) -> bool {
    match item.get("type").and_then(Value::as_str) {
        Some("collabAgentToolCall") => matches!(
            item.get("tool").and_then(Value::as_str),
            Some("spawnAgent") | Some("sendInput") | Some("resumeAgent")
        ),
        Some("subAgentActivity") => item.get("kind").and_then(Value::as_str) == Some("started"),
        _ => false,
    }
}

/// Route a sub-agent-thread event into its spawn part's `children`. Resolves the
/// bucket, applies the event, and registers any grandchild threads discovered
/// (a sub-agent spawning its own). On the sub thread's `turn/completed`, stamps
/// the spawn part's status terminal so the UI spinner stops.
fn route_sub_event(
    ctx: &mut TurnCtx,
    sub_threads: &mut HashMap<String, SubThread>,
    parent_thread: &str,
    tid: &str,
    method: &str,
    params: &Value,
) {
    let Some(spawn_part_id) = sub_threads.get(tid).map(|s| s.spawn_part_id.clone()) else {
        return;
    };
    // Track liveness for the post-parent drain: a new turn on this thread (a
    // resumed / re-driven agent) re-arms it, its `turn/completed` retires it.
    if method == "turn/started" {
        if let Some(sub) = sub_threads.get_mut(tid) {
            sub.live = true;
        }
    }
    // A sub-agent's turn/completed → mark the spawn part terminal (don't add a
    // transcript part for it, and never end the parent turn).
    if method == "turn/completed" {
        if let Some(sub) = sub_threads.get_mut(tid) {
            sub.live = false;
        }
        if let Some(part) = find_part_mut(&mut ctx.assistant.parts, &spawn_part_id) {
            let interrupted = params
                .get("turn")
                .and_then(|t| t.get("status"))
                .and_then(Value::as_str)
                == Some("failed");
            if let Some(state) = part.state.as_mut() {
                if state.status == "running" {
                    state.status = if interrupted { "error" } else { "completed" }.into();
                }
            }
        }
        return;
    }
    let live = sub_threads.get(tid).is_some_and(|s| s.live);
    let Some(spawn_part) = find_part_mut(&mut ctx.assistant.parts, &spawn_part_id) else {
        return;
    };
    // Mirror thread liveness onto the spawn part: an async spawn's collab item
    // completes at launch, which would otherwise leave the row (and every
    // running indicator keyed off it) unspinning while the agent still works.
    // The thread's `turn/completed` above stamps it terminal again.
    if live {
        if let Some(state) = spawn_part.state.as_mut() {
            if state.status == "completed" {
                state.status = "running".into();
            }
        }
    }
    let discovered = apply_sub_notification(&mut spawn_part.children, tid, method, params);
    for found in discovered {
        // Same parent-thread guard as `register_sub_threads_from`: a child's
        // handoff item can reference the parent, which must never become a
        // waitable "sub-agent".
        if found.thread_id == parent_thread {
            continue;
        }
        register_sub_thread(
            sub_threads,
            found.thread_id,
            found.spawn_part_id,
            found.arms,
        );
    }
}

/// Stamp any still-`running` `subagent` spawn parts (at any depth) to
/// `completed` — called on parent-turn exit so a sub-agent whose completion we
/// never saw doesn't leave a permanent spinner.
fn settle_running_subagents(parts: &mut [WirePart]) {
    for part in parts.iter_mut() {
        if part.tool.as_deref() == Some("subagent") {
            if let Some(state) = part.state.as_mut() {
                if state.status == "running" {
                    state.status = "completed".into();
                }
            }
        }
        settle_running_subagents(&mut part.children);
    }
}

/// Delta-append into a sub-agent bucket, creating the (empty) part on the first
/// delta. Mirrors `append_delta` but targets `bucket` with namespaced ids.
fn append_delta_into(
    bucket: &mut Vec<WirePart>,
    tid: &str,
    params: &Value,
    make: impl FnOnce(String) -> WirePart,
) {
    let (Some(item_id), Some(delta)) = (
        params.get("itemId").and_then(Value::as_str),
        params.get("delta").and_then(Value::as_str),
    ) else {
        return;
    };
    let pid = namespaced_part_id(tid, item_id);
    if !bucket.iter().any(|p| p.id == pid) {
        bucket.push(make(pid.clone()));
    }
    if let Some(part) = bucket.iter_mut().find(|p| p.id == pid) {
        part.text.get_or_insert_with(String::new).push_str(delta);
    }
}

/// Pretty-print a wire value: pass strings through verbatim, JSON-pretty the
/// rest. Used for MCP/dynamic tool results and errors.
fn value_to_pretty(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

/// The WirePart id of a plan item's text — a pure function of the plan item id
/// so the streamed `item/plan/delta` parts and the completed `plan` item upsert
/// the same part, and `plan_card` can find the authoritative plan text.
fn plan_part_id(item_id: &str) -> String {
    format!("plan-item-{item_id}")
}

/// Display text for a reasoning item: streamed content, else the summary.
fn reasoning_text(item: &Value) -> String {
    let join = |key: &str| {
        item.get(key)
            .and_then(Value::as_array)
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
            .unwrap_or_default()
    };
    let content = join("content");
    if content.is_empty() {
        join("summary")
    } else {
        content
    }
}

/// Best human-readable message out of a TurnError-ish value.
fn error_message(error: Option<&Value>) -> String {
    error
        .and_then(|e| {
            e.get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| e.as_str().map(str::to_string))
        })
        .unwrap_or_else(|| "codex reported an error".to_string())
}

fn append_native_recovery_context(ctx: &TurnCtx, setup: &mut Value) {
    let Some(recovery) = super::native_recovery_context(ctx, "Codex thread") else {
        return;
    };
    let instructions = setup
        .get("developerInstructions")
        .and_then(Value::as_str)
        .unwrap_or_default();
    setup["developerInstructions"] = Value::String(format!("{instructions}\n\n{recovery}"));
}

async fn ensure_codex_pre_accept(
    ctx: &mut TurnCtx,
    native_store: NativeStore,
) -> Result<Arc<CodexClient>> {
    loop {
        let ensure = ctx.host.codex.ensure(&ctx.session_id, native_store);
        let result = match ctx.orx_retry_remaining() {
            Some(remaining) => tokio::time::timeout(remaining, ensure)
                .await
                .map_err(|_| anyhow!("Codex setup exceeded the ORX retry budget"))?,
            None => ensure.await,
        };
        match result {
            Ok(client) => {
                ctx.clear_retry_status();
                return Ok(client);
            }
            Err(error) => {
                ctx.host.codex.kill_session(&ctx.session_id).await;
                let detail = error.to_string();
                let retryable = !["JSON-RPC -32600", "JSON-RPC -32601", "JSON-RPC -32602"]
                    .iter()
                    .any(|code| detail.contains(code));
                let retry = retryable.then(|| ctx.schedule_orx_retry(None)).flatten();
                let Some((retry_number, delay)) = retry else {
                    ctx.mark_delivery(DeliveryState::NotSent);
                    ctx.mark_terminal_failure("codex_setup", error.to_string());
                    return Err(error);
                };
                ctx.show_retry_status(
                    "orx",
                    "Restarting Codex app-server",
                    retry_number as i64 + 1,
                    Some(ORX_MAX_ATTEMPTS as i64),
                    Some(crate::store::now_ms() + delay.as_millis() as i64),
                );
                tokio::time::sleep(delay).await;
            }
        }
    }
}

async fn run_turn_app_server(ctx: &mut TurnCtx) -> Result<()> {
    // Entry sweep: any HELD (native_id) card still unresolved from an earlier
    // turn is a zombie — its JSON-RPC request died with its turn (or child), and
    // worse, codex request ids restart per child, so a click on a stale card
    // could be delivered to a live request minted later. Resolve them before
    // this turn can surface anything. Native-only (`true`) now that end-turn
    // cards exist (the synthesized plan card): those carry no native_id and
    // resume by message — the next user message replaces them, exactly like
    // Claude's precedent. Behavior-preserving for the pre-plan-mode cards (all
    // of which were native).
    ctx.host
        .resolve_stale_prompts(&ctx.session_id, true)
        .await?;
    let project = ctx.project.clone();
    let session_id = ctx.session_id.clone();
    // The modular orx skills land in the harness's session-skills dir, fresh,
    // for this session's agent to auto-load — source of truth is the trait.
    let skills_dir = Codex.session_skills_dir();
    let (repo, playbook) =
        tokio::task::spawn_blocking(move || ensure_playbook(&project, &session_id, skills_dir))
            .await
            .map_err(|e| anyhow!("playbook task failed: {e}"))??;
    let playbook_md = std::fs::read_to_string(&playbook).unwrap_or_default();

    let native_session = match ctx.native_session_id.as_deref() {
        Some(id) => codex_native_session(id).await?,
        None => None,
    };
    let preferred_store = native_session
        .as_ref()
        .map(|session| session.store)
        .unwrap_or(NativeStore::Isolated);
    let mut client = ensure_codex_pre_accept(ctx, preferred_store).await?;
    let auto_review_supported =
        if ctx.permission_mode.unwrap_or(PermissionMode::Auto) == PermissionMode::Auto {
            codex_auto_review_supported(&client, &repo).await
        } else {
            false
        };
    let (sandbox_mode, approval_policy, approvals_reviewer) =
        codex_policies(ctx.permission_mode, auto_review_supported);

    // Thread bring-up: reuse the thread this child already carries, resume a
    // persisted one on a fresh child (after an orx up restart or child crash),
    // else start a new thread. The playbook rides developerInstructions on
    // both start and resume, so a long-lived session picks up playbook
    // improvements on the next restart rather than keeping its first version
    // forever.
    let mut thread_setup = serde_json::json!({
        "cwd": repo.to_string_lossy(),
        "sandbox": sandbox_mode,
        "approvalPolicy": approval_policy,
        "approvalsReviewer": approvals_reviewer,
        "developerInstructions": playbook_md,
    });
    if let Some(service_tier) = &ctx.service_tier {
        thread_setup["serviceTier"] = Value::String(service_tier.clone());
    }
    if let Some(model) = &ctx.model {
        thread_setup["model"] = Value::String(model.clone());
    }
    let thread_id = match (ctx.native_session_id.clone(), native_session.as_ref()) {
        (Some(id), _) if client.resumed_thread().as_deref() == Some(id.as_str()) => id,
        (Some(_), None) => {
            append_native_recovery_context(ctx, &mut thread_setup);
            start_thread(ctx, &client, thread_setup).await?
        }
        (Some(id), Some(session)) => {
            let mut params = thread_setup.clone();
            params["threadId"] = Value::String(id.clone());
            params["path"] = Value::String(session.path.to_string_lossy().into_owned());
            let (resume_client, resumed) = codex_resume_thread(ctx, client.clone(), params).await?;
            client = resume_client;
            match resumed {
                Ok(resumed) => {
                    // Capture the effective model codex reports (top-level
                    // `model`) — the required `settings.model` for a
                    // collaborationMode mask, and the escape path when the
                    // session carries no explicit model.
                    client.set_thread_model(resumed.get("model").and_then(Value::as_str));
                    client.set_resumed_thread(&id);
                    id
                }
                Err(err) => {
                    // Transport failures never reach this arm. Recover only if
                    // the exact rollout disappeared after the initial lookup.
                    if codex_native_session(&id).await?.is_some() {
                        ctx.mark_terminal_failure("json_rpc", err.to_string());
                        return Err(anyhow!("codex thread/resume failed: {err}"));
                    }
                    if client.native_store() != NativeStore::Isolated {
                        ctx.host.codex.kill_session(&ctx.session_id).await;
                        client = ensure_codex_pre_accept(ctx, NativeStore::Isolated).await?;
                    }
                    append_native_recovery_context(ctx, &mut thread_setup);
                    start_thread(ctx, &client, thread_setup).await?
                }
            }
        }
        (None, _) => start_thread(ctx, &client, thread_setup).await?,
    };

    // Route events to this turn before starting it — nothing is missed.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let _route = client.register_turn(tx);

    let mut turn_params = serde_json::json!({
        "threadId": thread_id,
        "input": [{ "type": "text", "text": ctx.text }],
        // Explicit per turn — the composer can change mode/model mid-session,
        // and `sandboxPolicy` is the only carrier of writable roots.
        "approvalPolicy": approval_policy,
        "approvalsReviewer": approvals_reviewer,
        "sandboxPolicy": sandbox_policy_json(ctx.permission_mode, &repo).await,
    });
    if let Some(service_tier) = &ctx.service_tier {
        turn_params["serviceTier"] = Value::String(service_tier.clone());
    }
    if let Some(model) = &ctx.model {
        turn_params["model"] = Value::String(model.clone());
    }
    let effort = codex_reasoning(ctx.reasoning_level.as_deref(), ctx.model.as_deref());
    if let Some(effort) = effort {
        turn_params["effort"] = Value::String(effort.to_string());
    }

    // Conditional collaborationMode mask (see `collaboration_mode_json`).
    // Attaching a mask always injects a template, so attach one ONLY when we
    // want it:
    //   * Plan turn → the `plan` mask (codex's plan.md template + question tool).
    //   * Non-plan turn whose thread MAY be sticky-planned → the `default` mask,
    //     once, to un-stick (a `plan` turn leaves the thread planning until a
    //     turn carries `default`; there is no way back to "no template"). "May
    //     be sticky-planned" fires on either signal: the durable reset marker
    //     (survives restarts) or this child's in-memory `last_collab_mode` (a
    //     `plan` mask we sent and haven't cleared).
    //   * Otherwise → attach nothing (preserves today's template-free context).
    // The mask's required `settings.model` is the session model, falling back to
    // codex's reported thread model; keep the top-level `model`/`effort` above
    // so the None-model escape path still works (mask omitted, plain turn).
    let plan_turn = ctx.plan_mode;
    let mask_mode =
        collaboration_mask_mode(plan_turn, ctx.plan_reset_pending, client.last_collab_mode());
    let mut applied_mask_mode = None;
    if let Some(mode) = mask_mode {
        let collab_model = ctx.model.clone().or_else(|| client.thread_model());
        match collab_model {
            Some(model) => {
                turn_params["collaborationMode"] = collaboration_mode_json(mode, &model, effort);
                client.set_last_collab_mode(mode);
                applied_mask_mode = Some(mode);
            }
            None if plan_turn => {
                // Plan mode with no known model can't build the mask (settings
                // .model is required) — fail clearly rather than silently run a
                // plain (non-planning) turn the user asked to plan.
                return Err(anyhow!(
                    "codex did not report a model — cannot enter plan mode"
                ));
            }
            None => {
                // Un-stick wanted but no model to build the mask: omit it and
                // log. Degrades to today's behavior (the thread stays planned
                // until a turn carries `default`); rare (a resume before any
                // start/resume reported a model).
                eprintln!("orx up: codex reported no model — skipping the plan-mode un-stick mask");
            }
        }
    }

    let started = codex_pre_accept_request(ctx, &client, "turn/start", turn_params, true).await?;
    if applied_mask_mode == Some("default") && ctx.plan_reset_pending {
        Store::open()?.clear_chat_session_plan_reset(&ctx.session_id)?;
        ctx.plan_reset_pending = false;
    }
    // Everything below is filtered to this turn: an earlier turn of the same
    // session that was orx-side aborted (its native interrupt raced or never
    // fired) can still be streaming into the shared channel, and its tail —
    // fatally, its `turn/completed` — must not leak into this transcript.
    let turn_id = started
        .get("turn")
        .and_then(|t| t.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if turn_id.is_some() {
        ctx.mark_delivery(DeliveryState::Accepted);
    } else {
        ctx.mark_delivery(DeliveryState::Unknown);
        return Err(anyhow!("codex turn/start returned no turn id"));
    }
    // Arm the native interrupt now rather than on `turn/started` — an
    // interrupt landing before that notification would otherwise no-op.
    if let Some(turn_id) = turn_id.as_deref() {
        client.set_active_turn(turn_id);
    }

    // Open request cards surfaced this turn: WirePart id → (JSON-RPC request
    // id, kind). Kind picks the settle shape (a permission `decline` vs a
    // userInput empty-answers) on every exit path. Invariant: no unresolved
    // codex card outlives its turn — every exit path below sweeps them (resolve
    // + settle with codex), so a dead turn can't leave a live-looking card. (A
    // task *abort* skips the sweep, but `ChatHost::interrupt` settles pending
    // requests natively first, and the next turn's entry sweep in this function
    // resolves whatever survived.)
    let mut open_requests: HashMap<String, (Value, ServerReqKind)> = HashMap::new();
    // Absolute, so steering can't push the watchdog back on a wedged turn.
    let mut deadline = tokio::time::Instant::now() + TURN_WATCHDOG;

    // Sub-agent threads spawned this turn (Codex collaboration). Their events
    // stream on this same connection with a foreign turnId; we route them into
    // the spawning part's `children` instead of dropping them.
    let mut sub_threads: HashMap<String, SubThread> = HashMap::new();
    // The parent turn's `turn/completed` arrived while sub-agent threads were
    // still live — we're draining their tails before ending the turn (bounded
    // by DRAIN_QUIET_SETTLE, see the deadline below).
    let mut parent_done = false;

    loop {
        // Watchdog (see TURN_WATCHDOG for the false-positive trade-off).
        // Suspended while a card is pending — user think-time is unbounded by
        // design (question think-time too); codex's own ~5-minute approval
        // deadline still applies server-side.
        // The post-parent drain gets a much shorter deadline: the parent turn
        // is already over, so the only legitimate wait is agents actively
        // streaming — a long-quiet drain means a terminal event was missed (or
        // codex never sent one). Settling then is graceful, not lossy: codex
        // holds a finished agent's report for the next turn regardless.
        //
        // `steering` is borrowed out of `ctx` so the borrow ends with the
        // select — the steer arm's handler needs `&mut ctx`.
        let waited = {
            let steering = &mut ctx.steering;
            let event = async {
                if open_requests.is_empty() {
                    tokio::time::timeout_at(deadline, rx.recv()).await
                } else {
                    Ok(rx.recv().await)
                }
            };
            tokio::select! {
                event = event => Waited::Event(event),
                steer = super::next_steer(steering) => Waited::Steer(steer),
            }
        };
        let event = match waited {
            Waited::Steer(steer) => {
                steer_turn(ctx, &client, &thread_id, turn_id.as_deref(), steer).await;
                continue;
            }
            Waited::Event(Ok(event)) => {
                deadline = tokio::time::Instant::now() + turn_phase_quiet(parent_done);
                event
            }
            Waited::Event(Err(_)) if parent_done => {
                let stuck: Vec<&str> = sub_threads
                    .iter()
                    .filter(|(_, s)| s.live)
                    .map(|(tid, _)| tid.as_str())
                    .collect();
                eprintln!(
                    "orx up: codex sub-agent drain settled after {}s of silence \
                     (threads without a terminal event: {stuck:?})",
                    DRAIN_QUIET_SETTLE.as_secs()
                );
                finish_completed_turn(
                    ctx,
                    &client,
                    &thread_id,
                    turn_id.as_deref(),
                    plan_turn,
                    &mut open_requests,
                )
                .await;
                return Ok(());
            }
            Waited::Event(Err(_)) => {
                client.interrupt_active_turn().await;
                let message = format!(
                    "codex produced no output for {} minutes — turn interrupted",
                    TURN_WATCHDOG.as_secs() / 60
                );
                ctx.mark_terminal_failure("codex_watchdog", message.clone());
                ctx.push_error(message);
                settle_running_subagents(&mut ctx.assistant.parts);
                let _ = ctx.flush();
                return Ok(());
            }
        };
        let Some(event) = event else {
            settle_running_subagents(&mut ctx.assistant.parts);
            let _ = ctx.flush();
            return Err(anyhow!("codex app-server event stream ended mid-turn"));
        };
        match event {
            TurnEvent::Notification { method, params } => {
                match classify_event_thread(turn_id.as_deref(), &sub_threads, &params) {
                    EventScope::Stale => continue,
                    EventScope::SubAgent(tid) => {
                        route_sub_event(ctx, &mut sub_threads, &thread_id, &tid, &method, &params);
                        ctx.maybe_flush();
                        // Draining after the parent's turn/completed: the last
                        // live thread retiring ends the turn for real.
                        if parent_done && !sub_threads.values().any(|s| s.live) {
                            finish_completed_turn(
                                ctx,
                                &client,
                                &thread_id,
                                turn_id.as_deref(),
                                plan_turn,
                                &mut open_requests,
                            )
                            .await;
                            return Ok(());
                        }
                        continue;
                    }
                    EventScope::Parent => {}
                }
                // Codex settled a request itself (its approval deadline hit,
                // or our reply raced this notification): the card must not
                // stay live. Part ids are a pure function of the request id;
                // flushed immediately so the card goes read-only right away.
                if method == "serverRequest/resolved" {
                    if let Some(request_id) = params.get("requestId") {
                        let part_id = request_part_id(turn_id.as_deref(), request_id);
                        if open_requests.remove(&part_id).is_some() {
                            resolve_card(ctx, &part_id);
                            let _ = ctx.flush();
                        }
                    }
                }
                // A parent collab item spawns/drives sub-agents — register the
                // thread ids it references so their (foreign-turn) events route
                // into this spawn part's `children` from here on.
                register_sub_threads_from(&thread_id, &method, &params, &mut sub_threads);
                match apply_notification(ctx, &method, &params) {
                    Some(TurnEnd::Done { interrupted }) => {
                        // Spawned sub-agents outlive the parent turn — codex
                        // holds their reports for the NEXT turn and their
                        // threads keep streaming on this connection. Ending now
                        // would freeze those transcripts mid-run, so drain
                        // until every live thread retires (its own
                        // turn/completed); the watchdog still backstops a
                        // thread that never does. An interrupt ends everything.
                        if !interrupted && sub_threads.values().any(|s| s.live) {
                            parent_done = true;
                            deadline = tokio::time::Instant::now() + turn_phase_quiet(parent_done);
                            let _ = ctx.flush();
                            continue;
                        }
                        // Plan card only for a non-interrupted Plan turn — an
                        // interrupted plan turn has no finished plan.
                        finish_completed_turn(
                            ctx,
                            &client,
                            &thread_id,
                            turn_id.as_deref(),
                            plan_turn && !interrupted,
                            &mut open_requests,
                        )
                        .await;
                        return Ok(());
                    }
                    Some(TurnEnd::Failed(message)) => {
                        sweep_open_requests(ctx, &client, &mut open_requests).await;
                        settle_running_subagents(&mut ctx.assistant.parts);
                        // A terminal `error` notification may have already
                        // pushed this exact message — don't render it twice.
                        if !has_error_part(ctx, &message) {
                            ctx.push_error(message);
                        }
                        let _ = ctx.flush();
                        // The turn *finished* (with an error the transcript
                        // already shows); an Err here would double-report.
                        return Ok(());
                    }
                    None => {}
                }
            }
            TurnEvent::Request { id, method, params } => {
                let kind = crate::local::codex::server_req_kind(&method);
                if event_turn_mismatch(turn_id.as_deref(), &params) {
                    // A stale turn's request (aborted predecessor still
                    // streaming) is settled, never surfaced — with the reply
                    // shape its method can actually parse.
                    settle_request(&client, &id, kind).await;
                } else {
                    match kind {
                        ServerReqKind::Approval => {
                            let card = approval_card(turn_id.as_deref(), &method, &id, &params);
                            if let Some((part_id, part)) = card {
                                if matches!(ctx.permission_mode, Some(PermissionMode::Bypass)) {
                                    // Bypass runs sandbox-less with approvals off;
                                    // if codex asks anyway, the user's chosen mode
                                    // answers for them. (Question cards are never
                                    // auto-answered — only approvals.)
                                    let _ = client
                                        .respond(
                                            &id,
                                            serde_json::json!({
                                                "decision": approval_decision(true)
                                            }),
                                        )
                                        .await;
                                } else {
                                    // Surface the card and keep consuming events —
                                    // codex holds the command; the reply arrives
                                    // via `resume_from_prompt` on the user's click.
                                    open_requests.insert(part_id, (id, kind));
                                    ctx.upsert_part(part);
                                    let _ = ctx.flush();
                                }
                            } else {
                                // Classified Approval but no card (unknown method
                                // variant) — decline rather than block.
                                let _ = client.respond_decline(&id).await;
                            }
                        }
                        ServerReqKind::UserInput => {
                            // request_user_input (plan mode's clarifying question)
                            // → a held question card, answered inline or via the
                            // composer. All-secret questions can't be surfaced
                            // (never store secrets) → answer empty so codex
                            // proceeds without them.
                            match user_input_card(turn_id.as_deref(), &id, &params) {
                                Some((part_id, part)) => {
                                    open_requests.insert(part_id, (id, kind));
                                    ctx.upsert_part(part);
                                    let _ = ctx.flush();
                                }
                                None => {
                                    let _ = client
                                        .respond(&id, serde_json::json!({ "answers": {} }))
                                        .await;
                                }
                            }
                        }
                        ServerReqKind::Other => {
                            // A reply schema we don't speak — fail the call
                            // rather than answer in a shape codex can't parse.
                            let _ = client.respond_method_unsupported(&id).await;
                        }
                    }
                }
            }
            TurnEvent::Closed => {
                // Child gone: nothing to settle with codex; just close cards and
                // stamp any orphaned running sub-agent rows so they don't spin
                // forever in the persisted transcript.
                for part_id in std::mem::take(&mut open_requests).into_keys() {
                    resolve_card(ctx, &part_id);
                }
                settle_running_subagents(&mut ctx.assistant.parts);
                let _ = ctx.flush();
                return Err(anyhow!(
                    "codex app-server exited mid-turn; see {}",
                    crate::store::data_dir().join("agent-codex.log").display()
                ));
            }
        }
        ctx.maybe_flush();
    }
}

/// Turn-exit sweep half of the no-card-outlives-its-turn invariant: cards the
/// user never answered are resolved in the transcript and settled with codex in
/// the shape their kind requires (approval → decline, userInput → empty
/// answers). The settle is unconditional — `CodexClient::respond`'s pending-set
/// guard is the single arbiter, so an already-answered/settled id no-ops there.
async fn sweep_open_requests(
    ctx: &mut TurnCtx,
    client: &CodexClient,
    open: &mut HashMap<String, (Value, ServerReqKind)>,
) {
    for (part_id, (rpc_id, kind)) in open.drain() {
        resolve_card(ctx, &part_id);
        settle_request(client, &rpc_id, kind).await;
    }
}

/// Settle one server→client request in the reply shape its kind requires, so a
/// request orx is abandoning never leaves codex blocked. Approval → `decline`;
/// UserInput → an empty `{"answers": {}}` (codex proceeds without answers);
/// Other → a JSON-RPC method-not-found error.
async fn settle_request(client: &CodexClient, id: &Value, kind: ServerReqKind) {
    match kind {
        ServerReqKind::Approval => {
            let _ = client.respond_decline(id).await;
        }
        ServerReqKind::UserInput => {
            let _ = client
                .respond(id, serde_json::json!({ "answers": {} }))
                .await;
        }
        ServerReqKind::Other => {
            let _ = client.respond_method_unsupported(id).await;
        }
    }
}

/// True when the notification names a turn that is not ours. Notifications
/// without a turn id (warnings, thread-level events) pass through.
fn event_turn_mismatch(expected: Option<&str>, params: &Value) -> bool {
    let Some(expected) = expected else {
        return false;
    };
    let event_turn = params.get("turnId").and_then(Value::as_str).or_else(|| {
        params
            .get("turn")
            .and_then(|t| t.get("id"))
            .and_then(Value::as_str)
    });
    event_turn.is_some_and(|t| t != expected)
}

/// PromptAnswer.approve → the codex decision string. Per-command `accept`
/// (never `acceptForSession` — a single Allow must not silently widen future
/// commands); `decline` lets the model continue and report the denial.
fn approval_decision(approve: bool) -> &'static str {
    if approve {
        "accept"
    } else {
        "decline"
    }
}

/// The WirePart id of a server-request card (approval OR question), a pure
/// function of (turn, request id) — shared by `approval_card`, `user_input_card`,
/// and the `serverRequest/resolved` reconciliation so none needs a reverse
/// lookup. Turn-scoped because codex request ids restart at 0 per child process:
/// without the scope, a stale card from a previous child generation would
/// collide with a live one. (The turn-entry `resolve_stale_prompts` sweep is the
/// primary defense; this makes ids honest too.)
fn request_part_id(turn: Option<&str>, id: &Value) -> String {
    format!("appr-{}-{id}", turn.unwrap_or("t"))
}

/// A server→client approval request → a permission card. Returns the WirePart
/// id and the part; `native_id` carries the JSON-RPC request id's raw text —
/// the reply target for `resume_from_prompt`. `None` for request methods we
/// don't card (they get a JSON-RPC error reply instead — including
/// `item/permissions/requestApproval`, whose reply is a permission-profile
/// object, not a `{decision}`). The key list spans both carded schemas:
/// command/cwd exist only on commandExecution; fileChange carries just
/// reason/grantRoot, so its card leans on `reason`.
fn approval_card(
    turn: Option<&str>,
    method: &str,
    id: &Value,
    params: &Value,
) -> Option<(String, WirePart)> {
    let tool = match method {
        "item/commandExecution/requestApproval" => "bash",
        "item/fileChange/requestApproval" => "edit",
        _ => return None,
    };
    let mut input = serde_json::Map::new();
    for key in ["command", "cwd", "reason", "grantRoot"] {
        if let Some(v) = params.get(key).filter(|v| !v.is_null()) {
            input.insert(key.to_string(), v.clone());
        }
    }
    let part_id = request_part_id(turn, id);
    let prompt = WirePrompt {
        kind: "permission".into(),
        tool: Some(tool.into()),
        tool_input: Some(Value::Object(input)),
        native_id: Some(id.to_string()),
        ..Default::default()
    };
    Some((part_id.clone(), WirePart::prompt(part_id, prompt)))
}

/// An `item/tool/requestUserInput` server request → a `question` card. Codex's
/// schema is `{questions: [{id, header, question, isOther, isSecret, options:
/// [{label, description}]|null}]}`. We surface the FIRST non-secret question
/// (the composer answers one at a time); `native_id` carries the JSON-RPC id so
/// `resume_from_prompt` can reply. `tool_input` stashes every question id plus
/// the one we surfaced, so `user_input_reply` can fill an empty answer for the
/// rest (codex tolerates a partial `answers` map). `None` when there is no
/// non-secret question to show (all-secret / empty) — the caller answers empty
/// (`{"answers":{}}`) and never stores a secret prompt.
fn user_input_card(turn: Option<&str>, id: &Value, params: &Value) -> Option<(String, WirePart)> {
    let questions = params.get("questions").and_then(Value::as_array)?;
    // Every question id, for the multi-question reply fill.
    let all_ids: Vec<Value> = questions
        .iter()
        .filter_map(|q| q.get("id").cloned())
        .collect();
    // The first non-secret question is the one we surface.
    let q = questions
        .iter()
        .find(|q| !q.get("isSecret").and_then(Value::as_bool).unwrap_or(false))?;
    let answered_id = q.get("id").cloned()?;
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
    let part_id = request_part_id(turn, id);
    let prompt = WirePrompt {
        kind: "question".into(),
        question: q
            .get("question")
            .and_then(Value::as_str)
            .map(str::to_string),
        header: q.get("header").and_then(Value::as_str).map(str::to_string),
        options,
        // codex's request_user_input takes one answer per question id — no
        // multi-select notion, so leave it false.
        multi_select: false,
        native_id: Some(id.to_string()),
        tool_input: Some(serde_json::json!({
            "questionIds": all_ids,
            "answeredId": answered_id,
        })),
        ..Default::default()
    };
    Some((part_id.clone(), WirePart::prompt(part_id, prompt)))
}

/// The `item/tool/requestUserInput` reply for an answered question card: the
/// surfaced question id gets the selected labels, freeform note, or one
/// contextualized annotation answer; every other stashed id gets an empty
/// `{"answers": []}`. `Err` when none were provided leaves the card actionable.
fn user_input_reply(prompt: &WirePrompt, answer: &PromptAnswer) -> Result<Value> {
    let note = answer.note.as_deref().filter(|s| !s.trim().is_empty());
    let mut selected: Vec<String> = if !answer.answers.is_empty() {
        answer.answers.clone()
    } else if let Some(note) = note {
        vec![note.to_string()]
    } else if !answer.annotations.is_empty() {
        Vec::new()
    } else {
        return Err(anyhow!("select an option (or type an answer) to reply"));
    };
    if !answer.annotations.is_empty() {
        selected = vec![answer.contextualized_answer(answer.plain_answer_text())];
    }
    let tool_input = prompt.tool_input.as_ref();
    let answered_id = tool_input
        .and_then(|t| t.get("answeredId"))
        .cloned()
        .ok_or_else(|| anyhow!("codex question card has no answer id"))?;
    let mut answers = serde_json::Map::new();
    answers.insert(
        json_key(&answered_id),
        serde_json::json!({ "answers": selected }),
    );
    // Fill the remaining question ids empty so the whole call is answered.
    if let Some(ids) = tool_input
        .and_then(|t| t.get("questionIds"))
        .and_then(Value::as_array)
    {
        for qid in ids {
            let key = json_key(qid);
            answers
                .entry(key)
                .or_insert_with(|| serde_json::json!({ "answers": [] }));
        }
    }
    Ok(serde_json::json!({ "answers": Value::Object(answers) }))
}

/// A JSON value used as a `{"answers": {...}}` map key — a JSON object key is a
/// string, so a string id is used bare and anything else (a numeric id) by its
/// JSON text.
fn json_key(id: &Value) -> String {
    id.as_str()
        .map(str::to_string)
        .unwrap_or_else(|| id.to_string())
}

/// The end-turn plan card for a finished Plan-mode turn, as a ready-to-upsert
/// `WirePart` (id `plan-synth-{assistant_id}`, exactly like Claude's). Prefers
/// the authoritative plan item text (the `plan-item-*` part built from
/// `item/plan/delta` + the completed `plan` item; `synthesized: false`); falls
/// back to the last non-empty text part gated by the shared
/// `should_synthesize_plan` predicate (`synthesized: true`) — the model
/// presented the plan as prose without emitting a `plan` item. `None` when there
/// is nothing to approve. No `native_id`: an end-turn card resumes by message,
/// exactly like Claude's synthesized plan card.
fn plan_card(parts: &[WirePart], assistant_id: &str) -> Option<WirePart> {
    // Authoritative plan item text, if any streamed/completed this turn.
    let plan_text = parts
        .iter()
        .find(|p| p.id.starts_with("plan-item-"))
        .and_then(|p| p.text.as_deref())
        .filter(|t| !t.trim().is_empty());
    let card = if let Some(text) = plan_text {
        WirePrompt {
            kind: "plan".into(),
            plan: Some(text.to_string()),
            synthesized: false,
            ..Default::default()
        }
    } else {
        // No plan item — fall back to the last non-empty text part, gated by the
        // same predicate Claude uses (plan mode, no prompt surfaced, no error,
        // non-empty text). `saw_prompt = false`: any surfaced question/approval
        // card here doesn't count as an exit recourse (mirrors Claude — only a
        // plan answer exits plan mode), so a texty plan still gets a card.
        let last_text = parts
            .iter()
            .rev()
            .find(|p| p.kind == "text" && p.text.as_deref().is_some_and(|t| !t.trim().is_empty()))
            .and_then(|p| p.text.as_deref())?;
        let errored = parts.iter().any(|p| {
            p.state
                .as_ref()
                .is_some_and(|s| s.status == "error" && s.error.is_some())
        });
        if !should_synthesize_plan(true, false, errored, last_text) {
            return None;
        }
        WirePrompt {
            kind: "plan".into(),
            plan: Some(last_text.to_string()),
            synthesized: true,
            ..Default::default()
        }
    };
    Some(WirePart::prompt(format!("plan-synth-{assistant_id}"), card))
}

/// Mark a surfaced card resolved in the in-memory transcript (no-op when the
/// user already answered it). A card resolved by the user goes through
/// `ChatHost::respond` → store; `adopt_resolved_prompts` keeps the two views
/// consistent on flush.
fn resolve_card(ctx: &mut TurnCtx, part_id: &str) {
    if let Some(part) = ctx.assistant.parts.iter_mut().find(|p| p.id == part_id) {
        if let Some(prompt) = part.prompt.as_mut() {
            prompt.resolved = true;
        }
    }
}

/// Whether the transcript already shows an error part with this message.
fn has_error_part(ctx: &TurnCtx, message: &str) -> bool {
    ctx.assistant.parts.iter().any(|p| {
        p.state
            .as_ref()
            .is_some_and(|s| s.status == "error" && s.error.as_deref() == Some(message))
    })
}

fn retry_after(error: &JsonRpcError) -> Option<Duration> {
    let data = error.data.as_ref()?;
    data.get("retryAfterMs")
        .and_then(Value::as_u64)
        .map(Duration::from_millis)
        .or_else(|| {
            data.get("retryAfter")
                .and_then(Value::as_u64)
                .map(Duration::from_secs)
        })
}

async fn codex_pre_accept_request(
    ctx: &mut TurnCtx,
    client: &CodexClient,
    method: &str,
    params: Value,
    ambiguous_transport: bool,
) -> Result<Value> {
    loop {
        if ambiguous_transport {
            ctx.persist_delivery(DeliveryState::Unknown)?;
        }
        let request = client.try_request(method, params.clone());
        let result = match ctx.orx_retry_remaining() {
            Some(remaining) => tokio::time::timeout(remaining, request)
                .await
                .map_err(|_| anyhow!("codex {method} exceeded the ORX retry budget"))?,
            None => request.await,
        };
        match result {
            Ok(Ok(value)) => {
                ctx.clear_retry_status();
                return Ok(value);
            }
            Ok(Err(error)) if error.code == -32001 => {
                ctx.mark_delivery(DeliveryState::Rejected);
                let Some((retry_number, delay)) = ctx.schedule_orx_retry(retry_after(&error))
                else {
                    ctx.mark_terminal_failure("server_overloaded", error.to_string());
                    return Err(anyhow!("codex {method} failed: {error}"));
                };
                let next_retry_at = crate::store::now_ms() + delay.as_millis() as i64;
                ctx.show_retry_status(
                    "orx",
                    "Codex app-server is overloaded",
                    retry_number as i64 + 1,
                    Some(ORX_MAX_ATTEMPTS as i64),
                    Some(next_retry_at),
                );
                tokio::time::sleep(delay).await;
            }
            Ok(Err(error)) => {
                ctx.mark_delivery(DeliveryState::Rejected);
                ctx.mark_terminal_failure("json_rpc", error.to_string());
                return Err(anyhow!("codex {method} failed: {error}"));
            }
            Err(error) => {
                ctx.mark_delivery(if ambiguous_transport {
                    DeliveryState::Unknown
                } else {
                    DeliveryState::NotSent
                });
                return Err(error);
            }
        }
    }
}

async fn codex_resume_thread(
    ctx: &mut TurnCtx,
    mut client: Arc<CodexClient>,
    params: Value,
) -> Result<(Arc<CodexClient>, std::result::Result<Value, JsonRpcError>)> {
    loop {
        let request = client.try_request("thread/resume", params.clone());
        let result = match ctx.orx_retry_remaining() {
            Some(remaining) => tokio::time::timeout(remaining, request)
                .await
                .map_err(|_| anyhow!("codex thread/resume exceeded the ORX retry budget"))?,
            None => request.await,
        };
        match result {
            Ok(Ok(value)) => {
                ctx.clear_retry_status();
                return Ok((client, Ok(value)));
            }
            Ok(Err(error)) if error.code == -32001 => {
                ctx.mark_delivery(DeliveryState::Rejected);
                let Some((retry_number, delay)) = ctx.schedule_orx_retry(retry_after(&error))
                else {
                    ctx.mark_terminal_failure("server_overloaded", error.to_string());
                    return Err(anyhow!("codex thread/resume failed: {error}"));
                };
                ctx.show_retry_status(
                    "orx",
                    "Codex app-server is overloaded",
                    retry_number as i64 + 1,
                    Some(ORX_MAX_ATTEMPTS as i64),
                    Some(crate::store::now_ms() + delay.as_millis() as i64),
                );
                tokio::time::sleep(delay).await;
            }
            Ok(Err(error)) => {
                ctx.mark_delivery(DeliveryState::Rejected);
                return Ok((client, Err(error)));
            }
            Err(error) => {
                let Some((retry_number, delay)) = ctx.schedule_orx_retry(None) else {
                    ctx.mark_delivery(DeliveryState::NotSent);
                    ctx.mark_terminal_failure("codex_resume", error.to_string());
                    return Err(error);
                };
                ctx.show_retry_status(
                    "orx",
                    "Reconnecting to Codex",
                    retry_number as i64 + 1,
                    Some(ORX_MAX_ATTEMPTS as i64),
                    Some(crate::store::now_ms() + delay.as_millis() as i64),
                );
                ctx.host.codex.kill_session(&ctx.session_id).await;
                tokio::time::sleep(delay).await;
                let native_store = client.native_store();
                client = ensure_codex_pre_accept(ctx, native_store).await?;
            }
        }
    }
}

/// Bounded well under the shared request timeout: a steer is awaited *in* the
/// event loop, so a slow app-server would otherwise freeze the transcript and
/// hide any card it raises.
const STEER_TIMEOUT: Duration = Duration::from_secs(5);

/// Hand one steer to the turn already running.
///
/// codex rejects `turn/steer` on review and compaction turns, and older
/// app-servers lack the method — those answer definitively, so the message
/// parks for the next turn. A transport failure or timeout answers nothing:
/// codex may already have applied the text, so re-running it as a fresh turn
/// could execute the instruction twice. Say so instead.
async fn steer_turn(
    ctx: &mut TurnCtx,
    client: &CodexClient,
    thread_id: &str,
    turn_id: Option<&str>,
    steer: SteerMessage,
) {
    let Some(turn_id) = turn_id else {
        if let Err(error) = ctx.host.park_steer(&ctx.session_id, steer) {
            ctx.push_error(format!("Could not preserve steering message: {error}"));
        }
        return;
    };
    let answered = client
        .try_request_with_timeout(
            "turn/steer",
            serde_json::json!({
                "threadId": thread_id,
                "input": [{ "type": "text", "text": steer.text }],
                "expectedTurnId": turn_id,
            }),
            STEER_TIMEOUT,
        )
        .await;
    match answered {
        Ok(Ok(_)) => ctx.record_steer(&steer.display),
        Ok(Err(_)) => {
            if let Err(error) = ctx.host.park_steer(&ctx.session_id, steer) {
                ctx.push_error(format!("Could not preserve steering message: {error}"));
            }
        }
        Err(e) => {
            // Record it anyway: the composer is already cleared, so this is
            // the only copy of what the user typed.
            ctx.record_steer(&steer.display);
            ctx.push_error(format!(
                "codex did not confirm the steering message ({e}) — send it again if the turn ignores it"
            ));
        }
    }
}

/// `thread/start` and record the new thread id as the session's native id.
async fn start_thread(ctx: &mut TurnCtx, client: &CodexClient, params: Value) -> Result<String> {
    let result = codex_pre_accept_request(ctx, client, "thread/start", params, false).await?;
    let thread_id = result
        .get("thread")
        .and_then(|t| t.get("id"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("thread/start returned no thread id"))?
        .to_string();
    // Capture the effective model (top-level `model`) — the required
    // `settings.model` for a collaborationMode mask, and the escape path when
    // the session carries no explicit model.
    client.set_thread_model(result.get("model").and_then(Value::as_str));
    ctx.set_native_session_id(&thread_id);
    client.set_resumed_thread(&thread_id);
    Ok(thread_id)
}

// --- shared by both transports -------------------------------------------

/// The workspace's git dir (`git rev-parse --git-common-dir`), canonicalized.
/// Codex's `workspace-write` sandbox denies every git metadata write: it marks
/// each writable root's `.git` read-only, and a *worktree's* real metadata
/// (refs, objects, `FETCH_HEAD` under `.git/worktrees/<id>/`) lives in the
/// parent clone, outside the workspace entirely — orx session repos are always
/// worktrees of the hub clone. Interactively both denials escalate to approval
/// prompts; `codex exec` has none, so `git fetch`/`commit` just dies with
/// "Operation not permitted". Declaring the common dir as an explicit writable
/// root fixes both shapes — an explicit root beats the built-in `.git`
/// protection (verified against codex-cli 0.144 via `codex sandbox`, plain
/// clone and worktree). Canonicalized because codex requires absolute roots
/// and seatbelt matches real paths (`/var` vs `/private/var`).
async fn shared_git_dir(workspace: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(workspace)
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if line.is_empty() {
        return None;
    }
    absolute_git_dir(workspace, Path::new(&line))
}

/// `dir` as an absolute, symlink-free path; `git rev-parse` answers relative
/// to the workspace for a regular clone (`.git`) and absolute for a worktree.
fn absolute_git_dir(workspace: &Path, dir: &Path) -> Option<PathBuf> {
    workspace.join(dir).canonicalize().ok()
}

/// The orx data dir as a sandbox writable root. The `orx` CLI the agent
/// drives opens the SQLite store read-write (plus journal/WAL sidecars)
/// directly at `store::data_dir()`, which sits under `~/.local/share` —
/// outside every workspace — so `workspace-write` denies the open and every
/// store-touching command dies with "unable to open database file". Created
/// here (host side, unsandboxed) so canonicalize can't fail before first use;
/// canonicalized for the same reason as `shared_git_dir`. Note the grant is
/// the whole data dir — every project's store rows plus `run-logs/` and the
/// `agent-*.log` files — not scoped to the session; that's inherent to the
/// CLI opening the shared DB directly, and still strictly narrower than
/// Bypass.
pub(crate) fn ensure_orx_data_dir() -> Option<PathBuf> {
    let dir = crate::store::data_dir();
    std::fs::create_dir_all(&dir).ok()?;
    dir.canonicalize().ok()
}

/// The exact lifecycle lock file required by every stateful `orx` command.
/// Granting only this file avoids exposing the neighboring credentials file.
fn ensure_orx_lifecycle_lock() -> Option<PathBuf> {
    let lock = crate::store::open_lifecycle_lock().ok()?;
    drop(lock);
    crate::store::lifecycle_lock_path().canonicalize().ok()
}

/// Session reasoning id → Codex `model_reasoning_effort` value. See
/// [`resolve_reasoning`] for what a `None` result means.
///
/// Validation is per model, from the fallback table:
///   * a model the table knows → validate against its tiers;
///   * a model it doesn't (the catalog is discovered live now, so this is any
///     model outside the frozen four) → forward the value. The composer only
///     offered what `model/list` reported for that model, so an allowlist here
///     would drop genuinely supported tiers — the same reasoning as
///     `opencode_variant`. A stale/wrong value comes back as a codex 400,
///     which is surfaced to the chat, not swallowed;
///   * no model at all → the CLI's own configured default model, whose tiers
///     we can't know. Conservative intersection; matches what the composer
///     offers in that state, so nothing advertised is dropped.
fn codex_reasoning<'a>(level: Option<&'a str>, model: Option<&str>) -> Option<&'a str> {
    match model {
        Some(m) => match codex_model_reasoning(m) {
            Some(allowed) => resolve_reasoning(level, allowed),
            // Catalog-discovered model: forward anything but the sentinel.
            None => level.filter(|l| *l != REASONING_DEFAULT_ID),
        },
        None => resolve_reasoning(level, &CODEX_REASONING_LEVELS),
    }
}

fn command_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

fn command_argv(v: &Value) -> Option<Vec<String>> {
    let Value::Array(parts) = v else {
        return None;
    };
    if parts.is_empty() {
        return None;
    }
    parts
        .iter()
        .map(|part| part.as_str().map(str::to_string))
        .collect()
}

fn guardian_review_failure(params: &Value) -> Option<String> {
    let review = params.get("review");
    let status = review
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str);
    if matches!(status, Some("inProgress") | Some("approved")) {
        return None;
    }
    if let Some(rationale) = review
        .and_then(|value| value.get("rationale"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(rationale.to_string());
    }
    Some(
        match status {
            Some("denied") => "Automatic approval review denied.",
            Some("timedOut") => "Automatic approval review timed out.",
            Some("aborted") => "Automatic approval review aborted.",
            _ => "Automatic approval review failed.",
        }
        .to_string(),
    )
}

// --- legacy exec path (codex < 0.144, and ORX_CODEX_EXEC=1) -------------------

/// Session mode → Codex `exec` sandbox policy. `codex exec` can't prompt for
/// approval, so the sandbox *is* the permission boundary. `Bypass` is the one
/// mode that also drops the sandbox entirely (`--dangerously-...`); the rest run
/// sandboxed with approvals set to `never` (nothing to escalate to). Returns
/// `None` for `Bypass` to signal "use the bypass flag instead of `-s`".
fn codex_sandbox(mode: Option<PermissionMode>) -> Option<&'static str> {
    match mode.unwrap_or(PermissionMode::Auto) {
        PermissionMode::Plan => Some("read-only"),
        // AcceptEdits/Ask have no distinct exec semantics — treat as the
        // balanced default so a session that carries them still runs sanely.
        PermissionMode::Auto | PermissionMode::AcceptEdits | PermissionMode::Ask => {
            Some("workspace-write")
        }
        PermissionMode::Bypass => None,
    }
}

/// The `-c` value granting `roots` as sandbox writable roots, e.g.
/// `sandbox_workspace_write.writable_roots=["/a", "/b"]`. `None` when there
/// are no roots (omit the flag: `-c ...=[]` would still *replace* the user's
/// configured roots with nothing).
fn writable_roots_override(roots: &[PathBuf]) -> Option<String> {
    if roots.is_empty() {
        return None;
    }
    let list: Vec<String> = roots.iter().map(|p| native_store::toml_string(p)).collect();
    Some(format!(
        "sandbox_workspace_write.writable_roots=[{}]",
        list.join(", ")
    ))
}

async fn run_turn_exec(ctx: &mut TurnCtx) -> Result<()> {
    let bin = find_codex_required()?;
    let project = ctx.project.clone();
    let session_id = ctx.session_id.clone();
    // The modular orx skills land in the harness's session-skills dir, fresh,
    // for this session's agent to auto-load — source of truth is the trait.
    let skills_dir = Codex.session_skills_dir();
    let (repo, playbook) =
        tokio::task::spawn_blocking(move || ensure_playbook(&project, &session_id, skills_dir))
            .await
            .map_err(|e| anyhow!("playbook task failed: {e}"))??;

    let native_session = match ctx.native_session_id.as_deref() {
        Some(id) => codex_native_session(id).await?,
        None => None,
    };
    let native_store = native_session
        .as_ref()
        .map(|session| session.store)
        .unwrap_or(NativeStore::Isolated);
    let codex_home = tokio::task::spawn_blocking(move || native_store::prepare_codex(native_store))
        .await
        .map_err(|error| anyhow!("Codex config preparation failed: {error}"))??;
    let mut cmd = Command::new(&bin);
    match (&ctx.native_session_id, &native_session) {
        (Some(native_id), Some(_)) => {
            cmd.args(["exec", "resume", native_id]);
        }
        _ => {
            cmd.arg("exec");
        }
    }
    cmd.args(["--json", "--skip-git-repo-check"])
        .current_dir(&repo)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(crate::local::chat::harness_log("codex")?))
        .kill_on_drop(true);
    // Permission mode → sandbox policy. `codex exec` can't prompt, so the
    // sandbox is the approval boundary: non-bypass modes run sandboxed with
    // approvals disabled (nothing to escalate to), `Bypass` drops both.
    //
    // Set the policy via `-c sandbox_mode=` rather than `-s`: the `exec resume`
    // subcommand rejects `-s` ("unexpected argument"), but accepts `-c` on both
    // the fresh and resume paths (verified against codex-cli 0.143), so one form
    // works for the whole session lifecycle.
    //
    // Yields the data dir granted as a writable root (if any), so the child's
    // store can be pinned to it below, after `prepare_env`.
    let data_dir_pin = match if ctx.plan_mode {
        Some("read-only")
    } else {
        codex_sandbox(ctx.permission_mode)
    } {
        Some(policy) => {
            cmd.args([
                "-c",
                &format!("sandbox_mode=\"{policy}\""),
                "-c",
                "approval_policy=\"never\"",
            ]);
            // workspace-write out of the box is too tight for the orx
            // workflow in four ways (all verified via `codex sandbox` against
            // codex-cli 0.144; in the TUI these denials escalate to approval
            // prompts, which `codex exec` doesn't have):
            //   * Network is blocked by default — DNS doesn't even resolve, so
            //     `git fetch`/`push`, package installs, and the `orx` CLI's
            //     localhost API calls all die. The agent's job is launching
            //     experiments over that API; Auto must keep the network open.
            //   * The orx store isn't writable — the SQLite DB lives in the
            //     data dir under `~/.local/share`, outside the workspace, so
            //     every `orx` command that touches it fails with "unable to
            //     open database file"; grant the data dir (see
            //     `ensure_orx_data_dir`).
            //   * Every stateful `orx` command takes a lifecycle lock in the
            //     config dir before opening the store. Grant only that lock
            //     file, not the neighboring credentials (see
            //     `ensure_orx_lifecycle_lock`).
            //   * Git metadata isn't writable — codex protects `.git` inside
            //     the workspace, and a worktree's real metadata (the hub
            //     clone's `.git`) sits outside it — so `git fetch`/`commit`
            //     fail outright; grant the common dir (see `shared_git_dir`).
            // Note `-c` *replaces* any `writable_roots` from the user's
            // config.toml for the turn (there is no append form; `exec
            // --add-dir` is unverified on the resume path).
            if policy == "workspace-write" {
                cmd.args(["-c", "sandbox_workspace_write.network_access=true"]);
                let data_dir = ensure_orx_data_dir();
                let roots: Vec<PathBuf> = [
                    data_dir.clone(),
                    ensure_orx_lifecycle_lock(),
                    shared_git_dir(&repo).await,
                ]
                .into_iter()
                .flatten()
                .collect();
                if let Some(override_arg) = writable_roots_override(&roots) {
                    cmd.args(["-c", &override_arg]);
                }
                data_dir
            } else {
                None
            }
        }
        None => {
            cmd.arg("--dangerously-bypass-approvals-and-sandbox");
            None
        }
    };
    // Reasoning level → Codex's own `model_reasoning_effort` config override.
    if let Some(effort) = codex_reasoning(ctx.reasoning_level.as_deref(), ctx.model.as_deref()) {
        cmd.args(["-c", &format!("model_reasoning_effort=\"{effort}\"")]);
    }
    if let Some(service_tier) = &ctx.service_tier {
        cmd.args(["-c", &format!("service_tier=\"{service_tier}\"")]);
    }
    if let Some(model) = &ctx.model {
        cmd.args(["-m", model]);
    }
    if let Some(override_arg) = native_store::codex_sqlite_override(native_store, &codex_home) {
        cmd.args(["-c", &override_arg]);
    }
    let mut turn_text = legacy_exec_text(&ctx.text, ctx.plan_mode);
    if ctx.native_session_id.is_some() && native_session.is_none() {
        if let Some(recovery) = super::native_recovery_context(ctx, "Codex thread") {
            turn_text = format!("{recovery}\n\n{turn_text}");
        }
    }
    let prompt = if native_session.is_none() {
        let playbook_md = std::fs::read_to_string(&playbook).unwrap_or_default();
        format!(
            "<system-context>\n{playbook_md}\n</system-context>\n\n{}",
            turn_text
        )
    } else {
        turn_text
    };
    cmd.arg(prompt);
    prepare_env(&mut cmd);
    cmd.env("CODEX_HOME", codex_home);
    // Plain text only: a synced FORCE_COLOR would put escape codes in the
    // `--json` event stream, and every unparseable line reads as "not delivered".
    cmd.env("NO_COLOR", "1");
    // Tag the run this sandboxed turn may launch (`orx exp run`) with the
    // session so it can be explicitly subscribed to. After prepare_env so it
    // isn't shadowed by a synced value.
    set_chat_session_env(&mut cmd, &ctx.session_id, "codex", ctx.host.up_port());
    // Pin the sandboxed turn's store to the exact path granted above. The
    // grant was resolved from the host's env, but the child could resolve a
    // different data dir — `prepare_env` injects dashboard-synced vars (a
    // synced `ORX_DATA_DIR`/`XDG_DATA_HOME` absent from the host env), and a
    // relative `ORX_DATA_DIR` resolves against the child's cwd, not ours.
    // Must come after `prepare_env`: later `cmd.env` calls win, and the
    // synced-env injection guards on the *process* env, not the cmd's map.
    // (Unsandboxed Bypass has no grant to stay coherent with, so no pin — a
    // synced `ORX_DATA_DIR` still wins there.)
    if let Some(dir) = &data_dir_pin {
        cmd.env("ORX_DATA_DIR", dir);
    }
    ctx.persist_delivery(DeliveryState::Unknown)?;
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            ctx.mark_delivery(DeliveryState::NotSent);
            return Err(anyhow!("Could not spawn {}: {}", bin.display(), error));
        }
    };
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let mut lines = BufReader::new(stdout).lines();
    let mut counter = 0usize;
    let mut next_id = |prefix: &str| {
        counter += 1;
        format!("{prefix}-{counter}")
    };
    // Streaming deltas accumulate into one part until the complete event.
    let mut open_text: Option<String> = None;
    let mut open_reasoning: Option<String> = None;

    while let Some(line) = lines.next_line().await? {
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        ctx.mark_delivery(DeliveryState::Accepted);
        // Legacy events nest under "msg"; item-style events are flat.
        let msg = event.get("msg").unwrap_or(&event);
        let kind = msg.get("type").and_then(Value::as_str).unwrap_or("");

        // Session/thread id, wherever this version put it.
        for key in ["session_id", "thread_id", "conversation_id"] {
            if let Some(sid) = msg
                .get(key)
                .or_else(|| event.get(key))
                .and_then(Value::as_str)
            {
                ctx.set_native_session_id(sid);
            }
        }

        match kind {
            "agent_message_delta" => {
                let delta = msg.get("delta").and_then(Value::as_str).unwrap_or("");
                let id = open_text.get_or_insert_with(|| next_id("text")).clone();
                if ctx.assistant.parts.iter().all(|p| p.id != id) {
                    ctx.upsert_part(WirePart::text(id.clone(), ""));
                }
                ctx.append_part_text(&id, delta);
            }
            "agent_message" => {
                let text = msg.get("message").and_then(Value::as_str).unwrap_or("");
                let id = open_text.take().unwrap_or_else(|| next_id("text"));
                ctx.upsert_part(WirePart::text(id, text));
            }
            "agent_reasoning_delta" => {
                let delta = msg.get("delta").and_then(Value::as_str).unwrap_or("");
                let id = open_reasoning
                    .get_or_insert_with(|| next_id("think"))
                    .clone();
                if ctx.assistant.parts.iter().all(|p| p.id != id) {
                    ctx.upsert_part(WirePart::reasoning(id.clone(), ""));
                }
                ctx.append_part_text(&id, delta);
            }
            "agent_reasoning" => {
                let text = msg.get("text").and_then(Value::as_str).unwrap_or("");
                let id = open_reasoning.take().unwrap_or_else(|| next_id("think"));
                ctx.upsert_part(WirePart::reasoning(id, text));
            }
            "exec_command_begin" => {
                let id = msg
                    .get("call_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| next_id("cmd"));
                let command = msg.get("command").map(command_string).unwrap_or_default();
                ctx.upsert_part(WirePart {
                    id,
                    kind: "tool".into(),
                    text: None,
                    tool: Some("bash".into()),
                    state: Some(WireToolState {
                        status: "running".into(),
                        input: Some(serde_json::json!({ "command": command })),
                        output: None,
                        error: None,
                        title: None,
                    }),
                    prompt: None,
                    children: Vec::new(),
                });
            }
            "exec_command_end" => {
                let call_id = msg.get("call_id").and_then(Value::as_str).unwrap_or("");
                let exit_ok = msg
                    .get("exit_code")
                    .and_then(Value::as_i64)
                    .is_none_or(|c| c == 0);
                let output = msg
                    .get("aggregated_output")
                    .or_else(|| msg.get("stdout"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if let Some(part) = ctx.assistant.parts.iter_mut().find(|p| p.id == call_id) {
                    if let Some(state) = part.state.as_mut() {
                        state.status = if exit_ok { "completed" } else { "error" }.into();
                        state.output = Some(output);
                    }
                }
            }
            "error" | "stream_error" | "turn.failed" => {
                let detail = msg
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("codex reported an error")
                    .to_string();
                ctx.push_error(detail);
            }
            // Item-style shape: everything interesting is under "item".
            "item.completed" | "item.updated" => {
                if let Some(item) = msg.get("item") {
                    handle_item(ctx, item, &mut next_id);
                }
            }
            "token_count" => {
                let (used, context_window) =
                    token_count_usage(msg.get("info").unwrap_or(&Value::Null));
                if let Some(used) = used {
                    ctx.report_usage(ContextUsage {
                        used_tokens: used,
                        context_window,
                    });
                }
            }
            _ => {}
        }
        ctx.maybe_flush();
    }

    let status = child.wait().await?;
    if !status.success() {
        return Err(anyhow!(
            "codex exited with {status}; see {}",
            crate::store::data_dir().join("agent-codex.log").display()
        ));
    }
    if ctx.plan_mode {
        if let Some(card) = plan_card(&ctx.assistant.parts, &ctx.assistant.id) {
            ctx.upsert_part(card);
        }
        let _ = ctx.flush();
    }
    Ok(())
}

async fn codex_native_session(
    native_id: &str,
) -> Result<Option<native_store::NativeSessionLocation>> {
    let native_id = native_id.to_string();
    tokio::task::spawn_blocking(move || native_store::codex_session(&native_id))
        .await
        .map_err(|error| anyhow!("codex rollout lookup failed: {error}"))?
}

fn legacy_exec_text(text: &str, plan_mode: bool) -> String {
    if !plan_mode {
        return text.to_string();
    }
    format!(
        "<plan-mode>\nInvestigate and produce a complete implementation plan only. Do not modify \
         files or run mutating commands. Ask clarifying questions when needed, then present the \
         plan for approval.\n</plan-mode>\n\n{text}"
    )
}

fn handle_item(ctx: &mut TurnCtx, item: &Value, next_id: &mut impl FnMut(&str) -> String) {
    let id = item
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| next_id("item"));
    match item.get("type").and_then(Value::as_str) {
        Some("agent_message") => {
            let text = item.get("text").and_then(Value::as_str).unwrap_or("");
            ctx.upsert_part(WirePart::text(id, text));
        }
        Some("reasoning") => {
            let text = item.get("text").and_then(Value::as_str).unwrap_or("");
            ctx.upsert_part(WirePart::reasoning(id, text));
        }
        Some("command_execution") => {
            let failed = item.get("status").and_then(Value::as_str) == Some("failed")
                || item
                    .get("exit_code")
                    .and_then(Value::as_i64)
                    .is_some_and(|c| c != 0);
            ctx.upsert_part(WirePart {
                id,
                kind: "tool".into(),
                text: None,
                tool: Some("bash".into()),
                state: Some(WireToolState {
                    status: if failed { "error" } else { "completed" }.into(),
                    input: Some(serde_json::json!({
                        "command": item.get("command").map(command_string).unwrap_or_default(),
                        "cwd": item.get("cwd").and_then(Value::as_str),
                    })),
                    output: item
                        .get("aggregated_output")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    error: None,
                    title: None,
                }),
                prompt: None,
                children: Vec::new(),
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::super::options::REASONING_DEFAULT_ID;
    use super::*;
    use serde_json::json;

    fn model_ids(models: &[ModelInfo]) -> Vec<&str> {
        models.iter().map(|model| model.id.as_str()).collect()
    }

    #[test]
    fn custom_provider_uses_its_env_key_and_configured_model() {
        // The exact shape from the bug report: a gateway provider that opts out
        // of OpenAI auth, so there is deliberately no auth.json to find.
        let provider = parse_custom_provider(
            r#"
model = "gateway-model"
model_provider = "custom"

[model_providers.custom]
base_url = "https://gateway.example/v1"
env_key = "CUSTOM_API_KEY"
wire_api = "responses"
requires_openai_auth = false
"#,
        )
        .unwrap();
        assert_eq!(provider.model.as_deref(), Some("gateway-model"));
        assert_eq!(provider.env_key.as_deref(), Some("CUSTOM_API_KEY"));

        // A provider that still wants OpenAI auth falls through to auth.json.
        assert!(parse_custom_provider(
            r#"
model_provider = "openai"
[model_providers.openai]
requires_openai_auth = true
"#
        )
        .is_none());

        // So does a config that names no provider at all.
        assert!(parse_custom_provider("model = \"gpt-5.6-sol\"").is_none());
    }

    #[test]
    fn custom_provider_is_read_off_disk_without_an_auth_json() {
        // End-to-end over the same file read `detect` performs: a CODEX_HOME
        // holding only config.toml (no auth.json, as the bug report describes)
        // still yields a provider whose env_key gates readiness.
        let dir = std::env::temp_dir().join("orx-codex-detect-test");
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config.toml");
        std::fs::write(
            &config,
            r#"
model = "gateway-model"
model_provider = "custom"

[model_providers.custom]
base_url = "https://gateway.example/v1"
env_key = "ORX_TEST_UNSET_CREDENTIAL"
requires_openai_auth = false
"#,
        )
        .unwrap();

        assert!(!dir.join("auth.json").exists());
        let provider = std::fs::read_to_string(&config)
            .ok()
            .as_deref()
            .and_then(parse_custom_provider)
            .expect("config.toml should yield a custom provider");

        assert_eq!(provider.model.as_deref(), Some("gateway-model"));
        // The credential is absent, so detection reports not-ready and the note
        // names the variable to set instead of telling the user to run
        // `codex login` (which would be the wrong instruction here).
        assert!(!provider.is_ready());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn custom_provider_records_whether_a_model_catalog_is_declared() {
        // A declared catalog (the DeepSeek-style setup) means the app-server
        // `model/list` reflects the provider's own models, so orx should probe
        // it and offer every listed model.
        assert!(
            parse_custom_provider(
                r#"
model = "deepseek-v4-flash"
model_provider = "deepseek"
model_catalog_json = "~/.codex/models.json"

[model_providers.deepseek]
base_url = "https://api.deepseek.com/"
wire_api = "responses"
requires_openai_auth = false
"#,
            )
            .unwrap()
            .has_model_catalog
        );

        // A bare gateway provider has no explicit catalog; the CLI would fall
        // back to its bundled first-party list, so orx keeps offering only the
        // configured model instead of a catalog that says nothing about the
        // endpoint.
        assert!(
            !parse_custom_provider(
                r#"
model = "gateway-model"
model_provider = "custom"

[model_providers.custom]
base_url = "https://gateway.example/v1"
wire_api = "responses"
requires_openai_auth = false
"#,
            )
            .unwrap()
            .has_model_catalog
        );
        assert!(
            !parse_custom_provider(
                r#"
model_provider = "custom"
model_catalog_json = "  "
[model_providers.custom]
requires_openai_auth = false
"#,
            )
            .unwrap()
            .has_model_catalog
        );
        assert!(parse_custom_provider("not toml ===").is_none());
    }

    #[test]
    fn custom_provider_models_keep_the_configured_model_first() {
        let models = custom_provider_models(
            Some("configured"),
            Some(vec![
                ModelInfo::new("first"),
                ModelInfo::new("configured").with_label(Some("Configured model"), None),
                ModelInfo::new("last"),
            ]),
        );

        assert_eq!(model_ids(&models), ["configured", "first", "last"]);
        assert_eq!(models[0].display_name.as_deref(), Some("Configured model"));

        let models = custom_provider_models(
            Some("configured"),
            Some(vec![ModelInfo::new("first"), ModelInfo::new("last")]),
        );
        assert_eq!(model_ids(&models), ["configured", "first", "last"]);
        assert!(models[0].reasoning_levels.is_none());
        assert!(models[0].default_reasoning_level.is_none());
    }

    #[test]
    fn custom_provider_models_preserve_catalog_fallbacks() {
        let models = custom_provider_models(Some("configured"), None);
        assert_eq!(model_ids(&models), ["configured"]);

        let models = custom_provider_models(
            None,
            Some(vec![ModelInfo::new("first"), ModelInfo::new("last")]),
        );
        assert_eq!(model_ids(&models), ["first", "last"]);
        assert!(custom_provider_models(None, None).is_empty());
    }

    #[test]
    fn version_parses_cli_output_and_gates() {
        assert_eq!(parse_version("codex-cli 0.144.0"), Some((0, 144, 0)));
        assert_eq!(parse_version("0.150.2"), Some((0, 150, 2)));
        assert_eq!(parse_version("codex-cli 1.0.3-nightly"), Some((1, 0, 3)));
        assert_eq!(parse_version("codex-cli"), None);
        assert_eq!(parse_version(""), None);
        // The gate itself: tuple ordering does the right thing.
        assert!(parse_version("codex-cli 0.143.9").unwrap() < MIN_APP_SERVER_VERSION);
        assert!(parse_version("codex-cli 0.144.0").unwrap() >= MIN_APP_SERVER_VERSION);
    }

    #[test]
    fn policies_map_modes_to_thread_params() {
        // Every non-bypass mode is the balanced sandbox with on-request
        // approvals; Bypass drops the sandbox, so approvals stay off.
        assert_eq!(
            codex_policies(None, true),
            ("workspace-write", "on-request", "auto_review")
        );
        assert_eq!(
            codex_policies(Some(PermissionMode::Auto), true),
            ("workspace-write", "on-request", "auto_review")
        );
        assert_eq!(
            codex_policies(Some(PermissionMode::Auto), false),
            ("workspace-write", "on-request", "user")
        );
        assert_eq!(
            codex_policies(Some(PermissionMode::Ask), true),
            ("workspace-write", "on-request", "user")
        );
        // Plan runs the SAME sandbox as Auto — native plan mode restricts at the
        // prompt level (the plan.md template), not the sandbox level.
        assert_eq!(
            codex_policies(Some(PermissionMode::Plan), true),
            ("workspace-write", "on-request", "user")
        );
        assert_eq!(
            codex_policies(Some(PermissionMode::Bypass), true),
            ("danger-full-access", "never", "user")
        );

        assert!(config_supports_auto_review(&serde_json::json!({
            "config": {"model_provider": null, "openai_base_url": null}
        })));
        assert!(!config_supports_auto_review(&serde_json::json!({
            "config": {"model_provider": "custom", "openai_base_url": null}
        })));
        assert!(!config_supports_auto_review(&serde_json::json!({
            "config": {"model_provider": "openai", "openai_base_url": "https://gateway.test/v1"}
        })));
        assert!(!config_supports_auto_review(&serde_json::json!({})));
        assert!(!config_supports_auto_review(&serde_json::json!({
            "config": {"model_provider": null}
        })));
        assert!(!config_supports_auto_review(&serde_json::json!({
            "config": {"model_provider": 42, "openai_base_url": null}
        })));
    }

    /// Fold a trimmed live transcript (captured from the 0.144 spike, ids
    /// shortened) through the notification mapper and check the final parts.
    /// Pins: streamed deltas accumulate; the completed agentMessage is
    /// authoritative; a declined/failed command renders as an error tool part;
    /// unknown notifications are ignored; turn/completed ends the fold.
    #[test]
    fn transcript_fold_builds_the_expected_parts() {
        let transcript = [
            r#"{"method":"turn/started","params":{"threadId":"t1","turn":{"id":"turn1","status":"inProgress"}}}"#,
            r#"{"method":"mcpServer/startupStatus/updated","params":{"name":"x","status":"ready"}}"#,
            r#"{"method":"item/started","params":{"item":{"type":"userMessage","id":"u1"},"threadId":"t1","turnId":"turn1"}}"#,
            r#"{"method":"item/started","params":{"item":{"type":"reasoning","id":"rs_1","summary":[],"content":[]},"threadId":"t1","turnId":"turn1"}}"#,
            r#"{"method":"item/reasoning/summaryTextDelta","params":{"delta":"thinking…","itemId":"rs_1","threadId":"t1","turnId":"turn1"}}"#,
            r#"{"method":"item/completed","params":{"item":{"type":"reasoning","id":"rs_1","summary":[],"content":[]},"threadId":"t1","turnId":"turn1"}}"#,
            r#"{"method":"item/started","params":{"item":{"type":"commandExecution","id":"call_1","command":"/bin/zsh -lc 'touch /outside/probe.txt'","cwd":"/ws","status":"inProgress","aggregatedOutput":null,"exitCode":null},"threadId":"t1","turnId":"turn1"}}"#,
            r#"{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"call_1","command":"/bin/zsh -lc 'touch /outside/probe.txt'","cwd":"/ws","status":"declined","aggregatedOutput":null,"exitCode":null},"threadId":"t1","turnId":"turn1"}}"#,
            r#"{"method":"item/started","params":{"item":{"type":"agentMessage","id":"msg_1","text":"","phase":"final_answer"},"threadId":"t1","turnId":"turn1"}}"#,
            r#"{"method":"item/agentMessage/delta","params":{"delta":"Command","itemId":"msg_1","threadId":"t1","turnId":"turn1"}}"#,
            r#"{"method":"item/agentMessage/delta","params":{"delta":" was not run.","itemId":"msg_1","threadId":"t1","turnId":"turn1"}}"#,
            r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"msg_1","text":"Command was not run because the required escalation was rejected.","phase":"final_answer"},"threadId":"t1","turnId":"turn1"}}"#,
            r#"{"method":"turn/completed","params":{"threadId":"t1","turn":{"id":"turn1","status":"completed"}}}"#,
        ];

        let mut ctx = TurnCtx::test_stub();
        let mut ended = None;
        for line in transcript {
            match crate::local::codex::classify_line(line) {
                crate::local::codex::Line::Notification { method, params } => {
                    assert!(
                        !event_turn_mismatch(Some("turn1"), &params),
                        "fixture events all belong to turn1"
                    );
                    if let Some(end) = apply_notification(&mut ctx, &method, &params) {
                        ended = Some(end);
                        break;
                    }
                }
                other => panic!("fixture line classified unexpectedly: {other:?}"),
            }
        }
        assert!(matches!(ended, Some(TurnEnd::Done { interrupted: false })));

        let parts = &ctx.assistant.parts;
        assert_eq!(parts.len(), 3, "reasoning + command + message: {parts:?}");
        // Reasoning: streamed summary delta survives the empty completed item.
        assert_eq!(parts[0].kind, "reasoning");
        assert_eq!(parts[0].text.as_deref(), Some("thinking…"));
        // Declined command → error tool part with the command as input.
        assert_eq!(parts[1].kind, "tool");
        let state = parts[1].state.as_ref().unwrap();
        assert_eq!(state.status, "error");
        assert_eq!(
            state.input.as_ref().unwrap()["command"],
            "/bin/zsh -lc 'touch /outside/probe.txt'"
        );
        assert_eq!(state.input.as_ref().unwrap()["cwd"], "/ws");
        // Agent message: the completed item's full text wins over the deltas.
        assert_eq!(parts[2].kind, "text");
        assert_eq!(
            parts[2].text.as_deref(),
            Some("Command was not run because the required escalation was rejected.")
        );
    }

    #[test]
    fn native_history_restores_a_missed_failed_command() {
        let mut parts = vec![tool_part(
            "ok".into(),
            "bash",
            "completed",
            Some(serde_json::json!({ "command": "printf ok" })),
            Some("ok".into()),
        )];
        let items = serde_json::json!([
            {
                "type": "commandExecution",
                "id": "failed",
                "command": "orx projects",
                "cwd": "/worktree",
                "status": "failed",
                "aggregatedOutput": "Operation not permitted (os error 1)\n",
                "exitCode": 1
            },
            {
                "type": "commandExecution",
                "id": "ok",
                "command": "printf ok",
                "cwd": "/worktree",
                "status": "completed",
                "aggregatedOutput": "ok",
                "exitCode": 0
            }
        ]);

        reconcile_items(&mut parts, items.as_array().unwrap());

        assert_eq!(parts.len(), 2);
        let failed = parts.iter().find(|part| part.id == "failed").unwrap();
        assert_eq!(failed.state.as_ref().unwrap().status, "error");
        assert_eq!(
            failed.state.as_ref().unwrap().output.as_deref(),
            Some("Operation not permitted (os error 1)\n")
        );
    }

    /// OR-181: `thread/read` re-mints item ids (`item-N`) for content the
    /// live stream keyed by its Responses id, so reconcile must treat the
    /// renamed item as the streamed part — not append a second copy.
    #[test]
    fn native_history_renamed_text_item_does_not_duplicate() {
        let mut parts = vec![
            WirePart::reasoning("rs_0ea484ab".to_string(), ""),
            WirePart::text("msg_0ea484ab".to_string(), "**Decision:** no JSD."),
        ];
        let items = serde_json::json!([
            { "type": "userMessage", "id": "item-11", "content": "which loss?" },
            { "type": "agentMessage", "id": "item-12", "text": "**Decision:** no JSD." },
            { "type": "agentMessage", "id": "item-13", "text": "A message the stream missed." }
        ]);

        reconcile_items(&mut parts, items.as_array().unwrap());

        let ids: Vec<&str> = parts.iter().map(|part| part.id.as_str()).collect();
        assert_eq!(
            ids,
            ["rs_0ea484ab", "msg_0ea484ab", "item-13"],
            "the streamed part survives under its own id; only the genuinely \
             missing message is appended"
        );
        assert_eq!(parts[1].text.as_deref(), Some("**Decision:** no JSD."));
        assert_eq!(
            parts[2].text.as_deref(),
            Some("A message the stream missed.")
        );
    }

    /// A renamed item whose live part streamed only partial text completes
    /// the streamed part in place instead of appending a sibling.
    #[test]
    fn native_history_renamed_item_completes_partial_streamed_text() {
        let mut parts = vec![
            WirePart::reasoning("rs_aa".to_string(), "Weighing"),
            WirePart::text("msg_bb".to_string(), "**Decision:** no J"),
        ];
        let items = serde_json::json!([
            { "type": "reasoning", "id": "item-1", "summary": ["Weighing the losses."] },
            { "type": "agentMessage", "id": "item-2", "text": "**Decision:** no JSD." }
        ]);

        reconcile_items(&mut parts, items.as_array().unwrap());

        assert_eq!(parts.len(), 2, "renamed items must not add parts");
        assert_eq!(parts[0].id, "rs_aa");
        assert_eq!(parts[0].text.as_deref(), Some("Weighing the losses."));
        assert_eq!(parts[1].id, "msg_bb");
        assert_eq!(parts[1].text.as_deref(), Some("**Decision:** no JSD."));
    }

    /// Consume-once matching: two identical history messages pair with at
    /// most one streamed part each, so a genuine repeat is still restored.
    #[test]
    fn native_history_identical_repeat_is_still_restored() {
        let mut parts = vec![WirePart::text("msg_aa".to_string(), "Done.")];
        let items = serde_json::json!([
            { "type": "agentMessage", "id": "item-1", "text": "Done." },
            { "type": "agentMessage", "id": "item-2", "text": "Done." }
        ]);

        reconcile_items(&mut parts, items.as_array().unwrap());

        let ids: Vec<&str> = parts.iter().map(|part| part.id.as_str()).collect();
        assert_eq!(ids, ["msg_aa", "item-2"], "the second repeat is appended");
    }

    /// A plan part sharing its text with the final message must not swallow a
    /// missing message (plan ids are derived, never re-minted by history).
    #[test]
    fn native_history_message_matching_plan_text_is_restored() {
        let mut parts = vec![WirePart::text("plan-item-plan_1".to_string(), "the plan")];
        let items = serde_json::json!([
            { "type": "agentMessage", "id": "item-2", "text": "the plan" }
        ]);

        reconcile_items(&mut parts, items.as_array().unwrap());

        let ids: Vec<&str> = parts.iter().map(|part| part.id.as_str()).collect();
        assert_eq!(
            ids,
            ["plan-item-plan_1", "item-2"],
            "a plan part must not absorb a real message"
        );
    }

    /// An empty streamed part carries no evidence of which item it is —
    /// `starts_with("")` matches anything — so it never adopts history text.
    #[test]
    fn native_history_empty_part_is_not_a_wildcard_match() {
        let mut parts = vec![WirePart::reasoning("rs_empty".to_string(), "")];
        let items = serde_json::json!([
            { "type": "reasoning", "id": "item-1", "summary": ["First thought."] }
        ]);

        reconcile_items(&mut parts, items.as_array().unwrap());

        let ids: Vec<&str> = parts.iter().map(|part| part.id.as_str()).collect();
        assert_eq!(ids, ["rs_empty", "item-1"], "the item appends, not adopts");
        assert_eq!(
            parts[0].text.as_deref(),
            Some(""),
            "the empty part stays untouched"
        );
        assert_eq!(parts[1].text.as_deref(), Some("First thought."));
    }

    #[test]
    fn interrupted_history_settles_in_progress_tools_as_interrupted() {
        let mut parts = Vec::new();
        let items = serde_json::json!([{
            "type": "commandExecution",
            "id": "command",
            "command": "sleep 30",
            "cwd": "/worktree",
            "status": "inProgress"
        }]);

        reconcile_items(&mut parts, items.as_array().unwrap());
        settle_interrupted_parts(&mut parts);

        assert_eq!(parts[0].state.as_ref().unwrap().status, "interrupted");
    }

    /// Every tool-flavored ThreadItem — web search, MCP, dynamic tool call —
    /// plus the generic fallback for unknown types render as tool parts;
    /// input echoes (userMessage / hookPrompt) render nothing.
    #[test]
    fn tool_items_render_as_tool_parts() {
        let transcript = [
            r#"{"method":"turn/started","params":{"threadId":"t1","turn":{"id":"turn1","status":"inProgress"}}}"#,
            // Input echoes — must not produce parts.
            r#"{"method":"item/started","params":{"item":{"type":"userMessage","id":"u1","content":"hi"},"threadId":"t1","turnId":"turn1"}}"#,
            r#"{"method":"item/completed","params":{"item":{"type":"hookPrompt","id":"h1","fragments":["x"]},"threadId":"t1","turnId":"turn1"}}"#,
            // Web search: query streams empty, then the final query lands.
            r#"{"method":"item/started","params":{"item":{"type":"webSearch","id":"ws1","query":""},"threadId":"t1","turnId":"turn1"}}"#,
            r#"{"method":"item/completed","params":{"item":{"type":"webSearch","id":"ws1","query":"rotary embeddings","action":{"type":"search","query":"rotary embeddings"}},"threadId":"t1","turnId":"turn1"}}"#,
            // Web-tool openPage action: empty query, url in the action.
            r#"{"method":"item/completed","params":{"item":{"type":"webSearch","id":"ws2","query":"","action":{"type":"openPage","url":"https://example.com/post"}},"threadId":"t1","turnId":"turn1"}}"#,
            // MCP tool call that succeeds.
            r#"{"method":"item/started","params":{"item":{"type":"mcpToolCall","id":"mcp1","server":"fs","tool":"read","arguments":{"path":"a.txt"},"status":"inProgress"},"threadId":"t1","turnId":"turn1"}}"#,
            r#"{"method":"item/completed","params":{"item":{"type":"mcpToolCall","id":"mcp1","server":"fs","tool":"read","arguments":{"path":"a.txt"},"status":"completed","result":{"text":"file body"}},"threadId":"t1","turnId":"turn1"}}"#,
            // MCP tool call that fails.
            r#"{"method":"item/completed","params":{"item":{"type":"mcpToolCall","id":"mcp2","server":"fs","tool":"write","arguments":{},"status":"failed","error":"permission denied"},"threadId":"t1","turnId":"turn1"}}"#,
            // Dynamic tool call reporting success:false.
            r#"{"method":"item/completed","params":{"item":{"type":"dynamicToolCall","id":"dyn1","tool":"lookup","namespace":"web","arguments":{"q":"x"},"status":"completed","success":false},"threadId":"t1","turnId":"turn1"}}"#,
            // Unknown future type: running → completed, with an extra field.
            r#"{"method":"item/started","params":{"item":{"type":"futureThing","id":"ft1","status":"inProgress","payload":"abc"},"threadId":"t1","turnId":"turn1"}}"#,
            r#"{"method":"item/completed","params":{"item":{"type":"futureThing","id":"ft1","status":"completed","payload":"abc"},"threadId":"t1","turnId":"turn1"}}"#,
            // contextCompaction: no status field, no residual fields.
            r#"{"method":"item/completed","params":{"item":{"type":"contextCompaction","id":"cc1"},"threadId":"t1","turnId":"turn1"}}"#,
            r#"{"method":"turn/completed","params":{"threadId":"t1","turn":{"id":"turn1","status":"completed"}}}"#,
        ];

        let mut ctx = TurnCtx::test_stub();
        let mut ended = None;
        for line in transcript {
            match crate::local::codex::classify_line(line) {
                crate::local::codex::Line::Notification { method, params } => {
                    if let Some(end) = apply_notification(&mut ctx, &method, &params) {
                        ended = Some(end);
                        break;
                    }
                }
                other => panic!("fixture line classified unexpectedly: {other:?}"),
            }
        }
        assert!(matches!(ended, Some(TurnEnd::Done { interrupted: false })));

        let parts = &ctx.assistant.parts;
        // ws1, ws2, mcp1, mcp2, dyn1, ft1, cc1 — the two input echoes drop.
        assert_eq!(parts.len(), 7, "one part per tool item: {parts:?}");
        assert!(parts.iter().all(|p| p.kind == "tool"));

        // WebSearch: final query wins over the empty streamed one.
        assert_eq!(parts[0].tool.as_deref(), Some("WebSearch"));
        let ws = parts[0].state.as_ref().unwrap();
        assert_eq!(ws.status, "completed");
        assert_eq!(ws.input.as_ref().unwrap()["query"], "rotary embeddings");

        // openPage action: query stays empty, url merged from the action.
        assert_eq!(parts[1].tool.as_deref(), Some("WebSearch"));
        let ws2_input = parts[1].state.as_ref().unwrap().input.as_ref().unwrap();
        assert_eq!(ws2_input["url"], "https://example.com/post");
        assert_eq!(ws2_input["query"], "");

        // MCP success: tool "server:tool", result in the output.
        assert_eq!(parts[2].tool.as_deref(), Some("fs:read"));
        let mcp1 = parts[2].state.as_ref().unwrap();
        assert_eq!(mcp1.status, "completed");
        assert!(mcp1.output.as_ref().unwrap().contains("file body"));

        // MCP failure: error status, error text in the output.
        assert_eq!(parts[3].tool.as_deref(), Some("fs:write"));
        let mcp2 = parts[3].state.as_ref().unwrap();
        assert_eq!(mcp2.status, "error");
        assert_eq!(mcp2.output.as_deref(), Some("permission denied"));

        // Dynamic tool call: success:false → error, name "namespace:tool".
        assert_eq!(parts[4].tool.as_deref(), Some("web:lookup"));
        assert_eq!(parts[4].state.as_ref().unwrap().status, "error");

        // Unknown type: named after the raw type, running → completed, and the
        // extra field is carried as input without id/type.
        assert_eq!(parts[5].tool.as_deref(), Some("futureThing"));
        let ft = parts[5].state.as_ref().unwrap();
        assert_eq!(ft.status, "completed");
        let ft_input = ft.input.as_ref().unwrap();
        assert_eq!(ft_input["payload"], "abc");
        assert!(ft_input.get("id").is_none() && ft_input.get("type").is_none());

        // contextCompaction: no residual fields → no input, completed.
        assert_eq!(parts[6].tool.as_deref(), Some("contextCompaction"));
        let cc = parts[6].state.as_ref().unwrap();
        assert_eq!(cc.status, "completed");
        assert!(cc.input.is_none());
    }

    /// Foreign-turn tails (an aborted predecessor still streaming) are
    /// filtered; turn-less notifications (warnings) pass through.
    #[test]
    fn turn_filter_skips_foreign_turns_only() {
        let expected = Some("turn2");
        assert!(event_turn_mismatch(
            expected,
            &serde_json::json!({"turnId": "turn1", "delta": "stale"})
        ));
        assert!(event_turn_mismatch(
            expected,
            &serde_json::json!({"turn": {"id": "turn1", "status": "completed"}})
        ));
        assert!(!event_turn_mismatch(
            expected,
            &serde_json::json!({"turnId": "turn2"})
        ));
        assert!(!event_turn_mismatch(
            expected,
            &serde_json::json!({"message": "no turn id here"})
        ));
        // Before turn/start answers, nothing is filtered.
        assert!(!event_turn_mismatch(
            None,
            &serde_json::json!({"turnId": "turn1"})
        ));
    }

    /// The 3-way classifier: parent-turn events are Parent, a registered
    /// sub-agent thread's foreign-turn events are SubAgent, and an *unregistered*
    /// foreign turn (an aborted predecessor's tail) is still Stale — the
    /// load-bearing behavior `event_turn_mismatch` guarded before this feature.
    #[test]
    fn classify_routes_subagents_but_still_drops_stale_predecessors() {
        let mut subs: HashMap<String, SubThread> = HashMap::new();
        subs.insert(
            "sub".into(),
            SubThread {
                spawn_part_id: "spawn1".into(),
                live: true,
            },
        );
        // Same turn as parent → Parent.
        assert!(matches!(
            classify_event_thread(Some("turn1"), &subs, &json!({"turnId":"turn1"})),
            EventScope::Parent
        ));
        // Foreign turn, known sub thread → SubAgent.
        assert!(matches!(
            classify_event_thread(
                Some("turn1"),
                &subs,
                &json!({"turnId":"subturn","threadId":"sub"})
            ),
            EventScope::SubAgent(tid) if tid == "sub"
        ));
        // Foreign turn, UNKNOWN thread (stale predecessor) → Stale, dropped.
        assert!(matches!(
            classify_event_thread(
                Some("turn1"),
                &subs,
                &json!({"turnId":"turn0","threadId":"other"})
            ),
            EventScope::Stale
        ));
    }

    /// A spawned sub-agent's items stream into the spawn part's `children`, and
    /// its `turn/completed` settles the spawn row without ending the parent turn.
    #[test]
    fn subagent_transcript_streams_into_spawn_part_children() {
        let mut ctx = TurnCtx::test_stub();
        let mut subs: HashMap<String, SubThread> = HashMap::new();
        // Parent emits the collab spawn item (parent turn) → spawn part + register.
        let spawn = json!({"item":{"type":"collabAgentToolCall","id":"spawn1",
            "tool":"spawnAgent","status":"inProgress","receiverThreadIds":["sub"],
            "prompt":"go"},"threadId":"parent","turnId":"turn1"});
        register_sub_threads_from("parent-thread", "item/started", &spawn, &mut subs);
        apply_notification(&mut ctx, "item/started", &spawn);
        assert_eq!(subs.get("sub").unwrap().spawn_part_id, "spawn1");
        assert_eq!(ctx.assistant.parts[0].tool.as_deref(), Some("subagent"));
        assert_eq!(
            ctx.assistant.parts[0].state.as_ref().unwrap().status,
            "running"
        );

        // Sub-agent's own bash item streams into children (foreign turn).
        route_sub_event(
            &mut ctx,
            &mut subs,
            "parent-thread",
            "sub",
            "item/completed",
            &json!({"item":{"type":"commandExecution","id":"c1","command":"ls",
                "status":"completed","exitCode":0,"aggregatedOutput":"out"},
                "threadId":"sub","turnId":"subturn"}),
        );
        let children = &ctx.assistant.parts[0].children;
        assert_eq!(children.len(), 1, "sub bash lands under the spawn part");
        assert_eq!(children[0].id, "sub:c1", "child id is namespaced by thread");
        assert_eq!(
            children[0].state.as_ref().unwrap().output.as_deref(),
            Some("out")
        );

        // Sub-agent turn/completed settles the spawn row (parent turn unaffected).
        route_sub_event(
            &mut ctx,
            &mut subs,
            "parent-thread",
            "sub",
            "turn/completed",
            &json!({"turn":{"id":"subturn","status":"completed"},"threadId":"sub"}),
        );
        assert_eq!(
            ctx.assistant.parts[0].state.as_ref().unwrap().status,
            "completed"
        );
    }

    #[test]
    fn subagent_command_completion_preserves_streamed_state() {
        let mut bucket = Vec::new();
        apply_sub_notification(
            &mut bucket,
            "sub",
            "item/started",
            &json!({"item":{"type":"commandExecution","id":"c1","command":"orx logs $id","status":"inProgress"}}),
        );
        apply_sub_notification(
            &mut bucket,
            "sub",
            "item/commandExecution/outputDelta",
            &json!({"itemId":"c1","delta":"streamed\n"}),
        );
        let input = bucket[0].state.as_mut().unwrap().input.as_mut().unwrap();
        input["runTargetIds"] = json!(["11111111-1111-1111-1111-111111111111"]);
        input["runTargetIdsAuthoritative"] = json!(true);

        apply_sub_notification(
            &mut bucket,
            "sub",
            "item/completed",
            &json!({"item":{"type":"commandExecution","id":"c1","command":"orx logs $id","status":"completed","exitCode":0}}),
        );

        let state = bucket[0].state.as_ref().unwrap();
        assert_eq!(bucket[0].id, "sub:c1");
        assert_eq!(state.output.as_deref(), Some("streamed\n"));
        assert_eq!(
            state.input.as_ref().unwrap()["runTargetIds"],
            json!(["11111111-1111-1111-1111-111111111111"])
        );
    }

    /// A sub-agent that spawns its own sub-agent: the grandchild's transcript
    /// nests under the child spawn part (which itself lives in the parent's
    /// children), and orphan-settle stamps any still-running spawn part.
    #[test]
    fn nested_subagents_nest_and_orphans_settle() {
        let mut ctx = TurnCtx::test_stub();
        let mut subs: HashMap<String, SubThread> = HashMap::new();
        let spawn = json!({"item":{"type":"collabAgentToolCall","id":"spawn1",
            "tool":"spawnAgent","status":"inProgress","receiverThreadIds":["child"]},
            "threadId":"parent","turnId":"turn1"});
        register_sub_threads_from("parent-thread", "item/started", &spawn, &mut subs);
        apply_notification(&mut ctx, "item/started", &spawn);

        // Child spawns a grandchild — a collab item on the CHILD thread.
        route_sub_event(
            &mut ctx,
            &mut subs,
            "parent-thread",
            "child",
            "item/started",
            &json!({"item":{"type":"collabAgentToolCall","id":"spawn2",
                "tool":"spawnAgent","status":"inProgress","receiverThreadIds":["grand"]},
                "threadId":"child","turnId":"childturn"}),
        );
        // Grandchild registered, its spawn part namespaced under the child.
        assert_eq!(subs.get("grand").unwrap().spawn_part_id, "child:spawn2");

        // Grandchild does work → nests two levels deep.
        route_sub_event(
            &mut ctx,
            &mut subs,
            "parent-thread",
            "grand",
            "item/completed",
            &json!({"item":{"type":"agentMessage","id":"m1","text":"hi"},
                "threadId":"grand","turnId":"grandturn"}),
        );
        let child_spawn = &ctx.assistant.parts[0].children[0];
        assert_eq!(child_spawn.id, "child:spawn2");
        assert_eq!(child_spawn.children[0].id, "grand:m1");

        // Orphan-settle: both spawn parts still "running" → stamped completed.
        settle_running_subagents(&mut ctx.assistant.parts);
        assert_eq!(
            ctx.assistant.parts[0].state.as_ref().unwrap().status,
            "completed"
        );
        assert_eq!(
            ctx.assistant.parts[0].children[0]
                .state
                .as_ref()
                .unwrap()
                .status,
            "completed"
        );
    }

    /// A later collab item (`sendInput`) on an already-spawned thread re-points
    /// the thread to the new spawn row, so its continued activity streams under
    /// the new row — not the original, already-completed spawn.
    #[test]
    fn send_input_repoints_thread_to_the_new_spawn_row() {
        let mut ctx = TurnCtx::test_stub();
        let mut subs: HashMap<String, SubThread> = HashMap::new();
        let spawn = json!({"item":{"type":"collabAgentToolCall","id":"spawn1",
            "tool":"spawnAgent","status":"completed","receiverThreadIds":["sub"]},
            "threadId":"parent","turnId":"turn1"});
        register_sub_threads_from("parent-thread", "item/completed", &spawn, &mut subs);
        apply_notification(&mut ctx, "item/completed", &spawn);
        assert_eq!(subs.get("sub").unwrap().spawn_part_id, "spawn1");

        // Parent sends more input to the same thread → a new collab item/row.
        let send = json!({"item":{"type":"collabAgentToolCall","id":"spawn2",
            "tool":"sendInput","status":"inProgress","receiverThreadIds":["sub"]},
            "threadId":"parent","turnId":"turn1"});
        register_sub_threads_from("parent-thread", "item/started", &send, &mut subs);
        apply_notification(&mut ctx, "item/started", &send);
        // Thread now owned by the new row.
        assert_eq!(subs.get("sub").unwrap().spawn_part_id, "spawn2");

        // The sub-agent's fresh activity streams under spawn2, not spawn1.
        route_sub_event(
            &mut ctx,
            &mut subs,
            "parent-thread",
            "sub",
            "item/completed",
            &json!({"item":{"type":"agentMessage","id":"m2","text":"more"},
                "threadId":"sub","turnId":"subturn2"}),
        );
        let spawn1 = ctx
            .assistant
            .parts
            .iter()
            .find(|p| p.id == "spawn1")
            .unwrap();
        let spawn2 = ctx
            .assistant
            .parts
            .iter()
            .find(|p| p.id == "spawn2")
            .unwrap();
        assert!(
            spawn1.children.is_empty(),
            "original row gets no new activity"
        );
        assert_eq!(spawn2.children[0].id, "sub:m2");
    }

    /// The parent's own thread must never be registered as a waitable
    /// sub-agent: a child's handoff/interaction item can reference it, and a
    /// registered parent thread never emits another `turn/completed` — the
    /// post-parent drain would wait on it forever (the OR-178 hang).
    #[test]
    fn parent_thread_is_never_registered_as_a_sub_agent() {
        let mut subs: HashMap<String, SubThread> = HashMap::new();
        let activity = json!({"item":{"type":"subAgentActivity","id":"act1",
            "kind":"interacted","agentThreadId":"parent-thread"},
            "threadId":"child","turnId":"childturn"});
        register_sub_threads_from("parent-thread", "item/completed", &activity, &mut subs);
        assert!(subs.is_empty(), "parent thread must not be registered");

        // Same guard on the grandchild-discovery path inside route_sub_event.
        let mut ctx = TurnCtx::test_stub();
        let spawn = json!({"item":{"type":"collabAgentToolCall","id":"spawn1",
            "tool":"spawnAgent","status":"inProgress","receiverThreadIds":["child"]},
            "threadId":"parent-thread","turnId":"turn1"});
        register_sub_threads_from("parent-thread", "item/started", &spawn, &mut subs);
        apply_notification(&mut ctx, "item/started", &spawn);
        route_sub_event(
            &mut ctx,
            &mut subs,
            "parent-thread",
            "child",
            "item/completed",
            &activity,
        );
        assert!(
            !subs.contains_key("parent-thread"),
            "handoff item must not re-register the parent"
        );
    }

    #[test]
    fn command_output_deltas_accumulate_and_final_output_wins() {
        let mut ctx = TurnCtx::test_stub();
        apply_notification(
            &mut ctx,
            "item/started",
            &serde_json::json!({"item":{"type":"commandExecution","id":"c1","command":"ls","status":"inProgress"}}),
        );
        for delta in ["a\n", "b\n"] {
            apply_notification(
                &mut ctx,
                "item/commandExecution/outputDelta",
                &serde_json::json!({"itemId":"c1","delta":delta}),
            );
        }
        {
            let input = ctx.assistant.parts[0]
                .state
                .as_mut()
                .unwrap()
                .input
                .as_mut()
                .unwrap();
            input["runTargetIds"] = serde_json::json!(["11111111-1111-1111-1111-111111111111"]);
            input["runTargetIdsAuthoritative"] = serde_json::json!(true);
        }
        // No aggregatedOutput on the completed item → streamed output survives.
        apply_notification(
            &mut ctx,
            "item/completed",
            &serde_json::json!({"item":{"type":"commandExecution","id":"c1","command":"ls","status":"completed","exitCode":0}}),
        );
        let state = ctx.assistant.parts[0].state.as_ref().unwrap();
        assert_eq!(state.status, "completed");
        assert_eq!(state.output.as_deref(), Some("a\nb\n"));
        assert_eq!(
            state.input.as_ref().unwrap()["runTargetIds"],
            serde_json::json!(["11111111-1111-1111-1111-111111111111"])
        );
        assert_eq!(
            state.input.as_ref().unwrap()["runTargetIdsAuthoritative"],
            true
        );

        // With aggregatedOutput present, it is authoritative.
        apply_notification(
            &mut ctx,
            "item/completed",
            &serde_json::json!({"item":{"type":"commandExecution","id":"c1","command":"ls","status":"completed","exitCode":0,"aggregatedOutput":"final"}}),
        );
        let state = ctx.assistant.parts[0].state.as_ref().unwrap();
        assert_eq!(state.output.as_deref(), Some("final"));
    }

    #[test]
    fn command_execution_preserves_structured_argv() {
        let array_part = item_to_part(
            &serde_json::json!({
                "type": "commandExecution",
                "id": "c1",
                "command": ["/bin/zsh", "-lc", "orx discover keyword \"multi word query\""]
            }),
            false,
            &[],
        )
        .unwrap();
        let array_input = array_part.state.unwrap().input.unwrap();
        assert_eq!(
            array_input["command"],
            "/bin/zsh -lc orx discover keyword \"multi word query\""
        );
        assert_eq!(
            array_input["commandArgv"],
            serde_json::json!([
                "/bin/zsh",
                "-lc",
                "orx discover keyword \"multi word query\""
            ])
        );

        let string_part = item_to_part(
            &serde_json::json!({
                "type": "commandExecution",
                "id": "c2",
                "command": "orx projects"
            }),
            false,
            &[],
        )
        .unwrap();
        assert!(
            string_part.state.unwrap().input.unwrap()["commandArgv"].is_null(),
            "string commands keep the legacy wire shape"
        );

        let malformed_part = item_to_part(
            &serde_json::json!({
                "type": "commandExecution",
                "id": "c3",
                "command": ["orx", 7, "projects"]
            }),
            false,
            &[],
        )
        .unwrap();
        let malformed_input = malformed_part.state.unwrap().input.unwrap();
        assert_eq!(malformed_input["command"], "orx projects");
        assert!(malformed_input["commandArgv"].is_null());

        let empty_part = item_to_part(
            &serde_json::json!({
                "type": "commandExecution",
                "id": "c4",
                "command": []
            }),
            false,
            &[],
        )
        .unwrap();
        let empty_input = empty_part.state.unwrap().input.unwrap();
        assert_eq!(empty_input["command"], "");
        assert!(empty_input["commandArgv"].is_null());
    }

    /// The live spike's approval request (trimmed) → a permission card whose
    /// native_id round-trips the JSON-RPC id, plus the decision mapping.
    #[test]
    fn approval_request_becomes_a_permission_card() {
        let id = serde_json::json!(0);
        let params = serde_json::json!({
            "threadId": "t1", "turnId": "turn1", "itemId": "call_1",
            "command": "/bin/zsh -lc 'touch /outside/probe.txt'",
            "cwd": "/ws",
            "reason": "Allow writing the requested probe file outside the workspace?",
            "grantRoot": null,
        });
        let (part_id, part) = approval_card(
            Some("turn1"),
            "item/commandExecution/requestApproval",
            &id,
            &params,
        )
        .unwrap();
        assert_eq!(part_id, "appr-turn1-0");
        assert_eq!(part.kind, "prompt");
        let prompt = part.prompt.as_ref().unwrap();
        assert_eq!(prompt.kind, "permission");
        assert_eq!(prompt.tool.as_deref(), Some("bash"));
        assert!(!prompt.resolved);
        // native_id is the raw JSON text of the id — parseable back to Value.
        assert_eq!(prompt.native_id.as_deref(), Some("0"));
        let input = prompt.tool_input.as_ref().unwrap();
        assert_eq!(input["command"], "/bin/zsh -lc 'touch /outside/probe.txt'");
        assert_eq!(input["cwd"], "/ws");
        assert!(input.get("grantRoot").is_none(), "nulls are dropped");

        // fileChange requests carry only reason/grantRoot (no command/cwd) —
        // the edit card leans on `reason`.
        let fc_params = serde_json::json!({
            "threadId": "t1", "turnId": "turn1", "itemId": "fc_1",
            "reason": "Allow writing outside the workspace?",
            "grantRoot": "/outside",
        });
        let (_, part) = approval_card(
            Some("turn1"),
            "item/fileChange/requestApproval",
            &id,
            &fc_params,
        )
        .unwrap();
        let prompt = part.prompt.unwrap();
        assert_eq!(prompt.tool.as_deref(), Some("edit"));
        let input = prompt.tool_input.as_ref().unwrap();
        assert!(input.get("command").is_none());
        assert_eq!(input["reason"], "Allow writing outside the workspace?");
        assert_eq!(input["grantRoot"], "/outside");

        // Non-approval request types → no card (JSON-RPC error reply instead).
        assert!(approval_card(Some("turn1"), "item/tool/requestUserInput", &id, &params).is_none());
        assert!(approval_card(
            Some("turn1"),
            "item/permissions/requestApproval",
            &id,
            &params
        )
        .is_none());

        assert_eq!(approval_decision(true), "accept");
        assert_eq!(approval_decision(false), "decline");
    }

    /// Part ids are turn-scoped: codex request ids restart at 0 per child
    /// process, so the same rpc id in two turns must yield distinct cards.
    #[test]
    fn request_part_ids_are_turn_scoped() {
        let id = serde_json::json!(0);
        assert_eq!(request_part_id(Some("turn1"), &id), "appr-turn1-0");
        assert_ne!(
            request_part_id(Some("turn1"), &id),
            request_part_id(Some("turn2"), &id)
        );
        // No turn id (filter disabled): still deterministic.
        assert_eq!(request_part_id(None, &id), "appr-t-0");
    }

    #[test]
    fn resolve_card_marks_prompts_and_ignores_unknown_parts() {
        let mut ctx = TurnCtx::test_stub();
        let (part_id, part) = approval_card(
            Some("turn1"),
            "item/commandExecution/requestApproval",
            &serde_json::json!(7),
            &serde_json::json!({"command": "x"}),
        )
        .unwrap();
        ctx.upsert_part(part);
        resolve_card(&mut ctx, &part_id);
        assert!(ctx.assistant.parts[0].prompt.as_ref().unwrap().resolved);
        resolve_card(&mut ctx, &part_id); // idempotent
        resolve_card(&mut ctx, "missing"); // no-op, no panic
        assert_eq!(ctx.assistant.parts.len(), 1);
    }

    /// The Failed-dedup guard matches exactly how `push_error` stores errors
    /// (status "error" + the `error` field), and nothing else.
    #[test]
    fn has_error_part_matches_pushed_errors_only() {
        let mut ctx = TurnCtx::test_stub();
        assert!(!has_error_part(&ctx, "boom"));
        ctx.push_error("boom".to_string());
        assert!(has_error_part(&ctx, "boom"));
        assert!(!has_error_part(&ctx, "other"));
        // A failed *command* part is not an error part — its state.error is
        // None, so identical text can't false-match.
        apply_notification(
            &mut ctx,
            "item/completed",
            &serde_json::json!({"item":{"type":"commandExecution","id":"c1","command":"x","status":"failed"}}),
        );
        assert!(!has_error_part(&ctx, "x"));
    }

    #[test]
    fn error_notification_respects_will_retry() {
        let mut ctx = TurnCtx::test_stub();
        apply_notification(
            &mut ctx,
            "error",
            &serde_json::json!({"error":{"message":"transient"},"willRetry":true}),
        );
        assert_eq!(ctx.assistant.parts.len(), 1);
        let retry = ctx.assistant.parts[0].state.as_ref().unwrap();
        assert_eq!(retry.status, "running");
        assert_eq!(retry.input.as_ref().unwrap()["retryOwner"], "native");
        apply_notification(
            &mut ctx,
            "error",
            &serde_json::json!({"error":{"message":"fatal"},"willRetry":false}),
        );
        assert_eq!(ctx.assistant.parts.len(), 1);
        let state = ctx.assistant.parts[0].state.as_ref().unwrap();
        assert_eq!(state.status, "error");
    }

    #[test]
    fn guardian_approval_reviews_use_typed_statuses() {
        let mut ctx = TurnCtx::test_stub();
        apply_notification(
            &mut ctx,
            "guardianWarning",
            &serde_json::json!({"message":"Automatic review stopped this turn."}),
        );
        apply_notification(
            &mut ctx,
            "item/autoApprovalReview/started",
            &serde_json::json!({"review":{"status":"inProgress","rationale":null}}),
        );
        apply_notification(
            &mut ctx,
            "item/autoApprovalReview/completed",
            &serde_json::json!({"review":{"status":"approved","rationale":"Safe."}}),
        );
        assert!(ctx.assistant.parts.is_empty());

        apply_notification(
            &mut ctx,
            "item/autoApprovalReview/completed",
            &serde_json::json!({"review":{"status":"denied","rationale":"Blocked by policy."}}),
        );
        assert_eq!(ctx.assistant.parts.len(), 1);
        assert_eq!(
            ctx.assistant.parts[0]
                .state
                .as_ref()
                .unwrap()
                .error
                .as_deref(),
            Some("Blocked by policy.")
        );
    }

    #[test]
    fn guardian_terminal_reviews_have_deterministic_fallbacks() {
        for (status, expected) in [
            ("denied", "Automatic approval review denied."),
            ("timedOut", "Automatic approval review timed out."),
            ("aborted", "Automatic approval review aborted."),
            ("futureStatus", "Automatic approval review failed."),
        ] {
            let mut ctx = TurnCtx::test_stub();
            apply_notification(
                &mut ctx,
                "guardianWarning",
                &serde_json::json!({"message":"duplicate untyped notice"}),
            );
            apply_notification(
                &mut ctx,
                "item/autoApprovalReview/completed",
                &serde_json::json!({"review":{"status":status,"rationale":"  "}}),
            );
            assert_eq!(ctx.assistant.parts.len(), 1);
            assert_eq!(
                ctx.assistant.parts[0]
                    .state
                    .as_ref()
                    .unwrap()
                    .error
                    .as_deref(),
                Some(expected)
            );
        }

        let mut missing = TurnCtx::test_stub();
        apply_notification(
            &mut missing,
            "item/autoApprovalReview/completed",
            &serde_json::json!({"review":{}}),
        );
        assert_eq!(
            missing.assistant.parts[0]
                .state
                .as_ref()
                .unwrap()
                .error
                .as_deref(),
            Some("Automatic approval review failed.")
        );
    }

    #[test]
    fn failed_turn_surfaces_its_error() {
        let mut ctx = TurnCtx::test_stub();
        let end = apply_notification(
            &mut ctx,
            "turn/completed",
            &serde_json::json!({"turn":{"id":"t","status":"failed","error":{"message":"boom"}}}),
        );
        match end {
            Some(TurnEnd::Failed(msg)) => assert_eq!(msg, "boom"),
            _ => panic!("expected Failed"),
        }
        // Interrupted is a clean end, not a failure — and it carries the flag
        // that suppresses the end-turn plan card.
        let end = apply_notification(
            &mut ctx,
            "turn/completed",
            &serde_json::json!({"turn":{"id":"t","status":"interrupted"}}),
        );
        assert!(matches!(end, Some(TurnEnd::Done { interrupted: true })));
        // A plain completed turn is Done with interrupted:false.
        let end = apply_notification(
            &mut ctx,
            "turn/completed",
            &serde_json::json!({"turn":{"id":"t","status":"completed"}}),
        );
        assert!(matches!(end, Some(TurnEnd::Done { interrupted: false })));
    }

    #[test]
    fn sandbox_maps_modes_to_exec_policies() {
        // Plan is the only read-only mode; the interactive-only modes collapse to
        // the balanced default (exec can't tell them apart); Bypass drops the
        // sandbox (None → the `--dangerously-...` flag).
        assert_eq!(codex_sandbox(Some(PermissionMode::Plan)), Some("read-only"));
        assert_eq!(
            codex_sandbox(Some(PermissionMode::Auto)),
            Some("workspace-write")
        );
        assert_eq!(
            codex_sandbox(Some(PermissionMode::AcceptEdits)),
            Some("workspace-write")
        );
        assert_eq!(
            codex_sandbox(Some(PermissionMode::Ask)),
            Some("workspace-write")
        );
        assert_eq!(codex_sandbox(Some(PermissionMode::Bypass)), None);
        // No mode set → the balanced default, never an accidental full-access.
        assert_eq!(codex_sandbox(None), Some("workspace-write"));
    }

    #[test]
    fn exec_line_agent_message_reads_both_jsonl_shapes() {
        // Legacy shape: the message nests under "msg".
        assert_eq!(
            exec_line_agent_message(
                r#"{"msg":{"type":"agent_message","message":"Fix the login redirect"}}"#
            )
            .as_deref(),
            Some("Fix the login redirect")
        );
        // Item shape: the message rides an `item.completed` wrapper.
        assert_eq!(
            exec_line_agent_message(
                r#"{"type":"item.completed","item":{"type":"agent_message","id":"m1","text":"Fix the login redirect"}}"#
            )
            .as_deref(),
            Some("Fix the login redirect")
        );
        // Everything else is not the answer.
        for line in [
            "",
            "not json",
            r#"{"msg":{"type":"agent_reasoning","text":"thinking"}}"#,
            r#"{"type":"item.completed","item":{"type":"reasoning","text":"thinking"}}"#,
            r#"{"type":"token_count","info":{}}"#,
        ] {
            assert!(exec_line_agent_message(line).is_none(), "line: {line}");
        }
    }

    #[test]
    fn git_dir_resolves_relative_and_absolute_rev_parse_answers() {
        let base = std::env::temp_dir().join(format!("orx-codex-test-{}", std::process::id()));
        let workspace = base.join("worktree");
        let hub_git = base.join("hub").join(".git");
        std::fs::create_dir_all(workspace.join(".git")).unwrap();
        std::fs::create_dir_all(&hub_git).unwrap();

        // Worktree: rev-parse answers with the hub clone's absolute path.
        assert_eq!(
            absolute_git_dir(&workspace, &hub_git),
            Some(hub_git.canonicalize().unwrap())
        );
        // Regular clone: rev-parse answers `.git`, relative to the workspace.
        assert_eq!(
            absolute_git_dir(&workspace, Path::new(".git")),
            Some(workspace.join(".git").canonicalize().unwrap())
        );
        // No git dir at all → no writable root (flag omitted, fail-safe).
        assert_eq!(absolute_git_dir(&workspace, Path::new("missing")), None);

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn toml_string_quotes_and_escapes_paths() {
        assert_eq!(
            native_store::toml_string(Path::new("/a/with space")),
            r#""/a/with space""#
        );
        assert_eq!(
            native_store::toml_string(Path::new(r#"/a/"q""#)),
            r#""/a/\"q\"""#
        );
        // DEL is the one char serde_json leaves raw that TOML rejects.
        assert_eq!(
            native_store::toml_string(Path::new("/a/\u{7f}b")),
            r#""/a/\u007Fb""#
        );
    }

    #[test]
    fn writable_roots_override_joins_and_omits_empty() {
        assert_eq!(
            writable_roots_override(&[PathBuf::from("/data dir"), PathBuf::from("/hub/.git")]),
            Some(r#"sandbox_workspace_write.writable_roots=["/data dir", "/hub/.git"]"#.into())
        );
        // No roots → no flag at all; `=[]` would clobber the user's own
        // config.toml roots for the turn.
        assert_eq!(writable_roots_override(&[]), None);
    }

    #[test]
    fn reasoning_accepts_only_codex_ids() {
        let sol = Some("gpt-5.6-sol");
        assert_eq!(codex_reasoning(Some("low"), sol), Some("low"));
        assert_eq!(codex_reasoning(Some("high"), sol), Some("high"));
        assert_eq!(codex_reasoning(Some("xhigh"), sol), Some("xhigh"));
        // Junk is dropped (the flag is omitted → CLI default), never forwarded
        // as an invalid `model_reasoning_effort`.
        assert_eq!(codex_reasoning(Some("nonsense"), sol), None);
        assert_eq!(codex_reasoning(None, sol), None);
    }

    /// The point of issue #123: the top tiers are model-specific, so the same
    /// stored level resolves differently per model rather than being clamped to
    /// one hard-coded intersection.
    #[test]
    fn reasoning_is_model_specific() {
        // Sol/Terra reach `ultra`; Luna stops at `max` (the catalog's word —
        // codex's own picker doesn't offer Luna `ultra`, so neither do we).
        for model in ["gpt-5.6-sol", "gpt-5.6-terra"] {
            assert_eq!(codex_reasoning(Some("ultra"), Some(model)), Some("ultra"));
        }
        assert_eq!(
            codex_reasoning(Some("max"), Some("gpt-5.6-luna")),
            Some("max")
        );
        assert_eq!(codex_reasoning(Some("ultra"), Some("gpt-5.6-luna")), None);
        // 5.5 stops at `xhigh`. An unsupported tier is dropped rather than sent
        // — this is the "changing models clears a stale effort" guarantee,
        // enforced backend-side too, and it matters because codex answers an
        // unsupported effort with a 400 that kills the turn.
        assert_eq!(
            codex_reasoning(Some("xhigh"), Some("gpt-5.5")),
            Some("xhigh")
        );
        assert_eq!(codex_reasoning(Some("max"), Some("gpt-5.5")), None);
        // A model outside the fallback table is catalog-discovered: the
        // composer offered only what `model/list` reported for it, so the value
        // is forwarded rather than clamped (same reasoning as opencode).
        assert_eq!(codex_reasoning(Some("ultra"), Some("gpt-9")), Some("ultra"));
        assert_eq!(
            codex_reasoning(Some(REASONING_DEFAULT_ID), Some("gpt-9")),
            None
        );
        // No model at all → the conservative fallback intersection.
        assert_eq!(codex_reasoning(Some("xhigh"), None), Some("xhigh"));
        assert_eq!(codex_reasoning(Some("max"), None), None);
    }

    /// The `model/list` parser against the live 0.144 response shape (headers
    /// trimmed to the fields we read). Efforts come out in catalog order,
    /// hidden entries are skipped, and every model leads with the sentinel.
    #[test]
    fn model_list_parses_catalog_models_and_efforts() {
        let result = serde_json::json!({
            "data": [
                {
                    "id": "gpt-5.6-sol", "model": "gpt-5.6-sol",
                    "displayName": "GPT-5.6 Sol", "hidden": false, "isDefault": true,
                    "defaultReasoningEffort": "low",
                    "supportedReasoningEfforts": [
                        { "reasoningEffort": "low", "description": "" },
                        { "reasoningEffort": "medium", "description": "" },
                        { "reasoningEffort": "high", "description": "" },
                        { "reasoningEffort": "xhigh", "description": "" },
                        { "reasoningEffort": "max", "description": "" },
                        { "reasoningEffort": "ultra", "description": "" },
                    ],
                    "serviceTiers": [
                        { "id": "priority", "name": "Fast", "description": "1.5x speed, increased usage" },
                        { "id": "ultrafast", "name": "Ultrafast", "description": "Access controlled" }
                    ],
                },
                {
                    "id": "gpt-5.4-mini", "model": "gpt-5.4-mini",
                    "displayName": "GPT-5.4 mini", "hidden": false, "isDefault": false,
                    "defaultReasoningEffort": "medium",
                    "supportedReasoningEfforts": [
                        { "reasoningEffort": "low", "description": "" },
                        { "reasoningEffort": "medium", "description": "" },
                        { "reasoningEffort": "high", "description": "" },
                        { "reasoningEffort": "xhigh", "description": "" },
                    ],
                },
                {
                    "id": "secret", "model": "secret", "displayName": "hidden one",
                    "hidden": true, "isDefault": false,
                    "defaultReasoningEffort": "medium",
                    "supportedReasoningEfforts": [],
                },
            ],
        });
        let models = parse_model_list(&result, None);
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["gpt-5.6-sol", "gpt-5.4-mini"]
        );
        let ids = |m: &ModelInfo| {
            m.reasoning_levels
                .as_ref()
                .map(|c| c.iter().map(|c| c.id.clone()).collect::<Vec<_>>())
        };
        // A reported default means a concrete preselected tier and NO sentinel
        // row — the picker shows the value that actually runs.
        assert_eq!(
            ids(&models[0]).unwrap(),
            ["low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(models[0].default_reasoning_level.as_deref(), Some("low"));
        assert_eq!(ids(&models[1]).unwrap(), ["low", "medium", "high", "xhigh"]);
        assert_eq!(models[1].default_reasoning_level.as_deref(), Some("medium"));
        // The catalog's display name rides along for the picker.
        assert_eq!(models[0].display_name.as_deref(), Some("GPT-5.6 Sol"));
        assert_eq!(models[0].service_tiers.as_ref().unwrap()[0].id, "priority");
        assert_eq!(models[0].service_tiers.as_ref().unwrap()[0].label, "Fast");
        assert_eq!(models[0].service_tiers.as_ref().unwrap().len(), 1);
        assert!(models[1].service_tiers.as_ref().unwrap().is_empty());

        // A config.toml `model_reasoning_effort` outranks the catalog default —
        // codex resolves it that way, so the preselect must too. A configured
        // value the model doesn't support falls back to the catalog default
        // rather than preselecting something the model rejects.
        let configured = parse_model_list(&result, Some("xhigh"));
        assert_eq!(
            configured[0].default_reasoning_level.as_deref(),
            Some("xhigh")
        );
        let unsupported = parse_model_list(&result, Some("ultra"));
        assert_eq!(
            unsupported[1].default_reasoning_level.as_deref(),
            Some("medium"),
            "gpt-5.4-mini has no ultra; the catalog default stands"
        );
        // Junk shapes parse to nothing rather than panicking.
        assert!(parse_model_list(&serde_json::json!({}), None).is_empty());
        assert!(parse_model_list(&serde_json::json!({ "data": "nope" }), None).is_empty());
    }

    #[test]
    fn configured_effort_reads_config_toml() {
        assert_eq!(
            parse_configured_effort("model_reasoning_effort = \"max\"").as_deref(),
            Some("max")
        );
        assert_eq!(parse_configured_effort("model = \"gpt-5.6-sol\""), None);
        assert_eq!(parse_configured_effort("not toml ==="), None);
    }

    /// `Default` must send no `model_reasoning_effort` at all — otherwise a
    /// user's configured `max` in `~/.codex/config.toml` is silently overridden
    /// by the composer (the concrete bug in issue #123).
    #[test]
    fn reasoning_default_sends_no_override() {
        for model in [Some("gpt-5.6-sol"), Some("gpt-5.5"), None] {
            assert_eq!(codex_reasoning(Some(REASONING_DEFAULT_ID), model), None);
        }
    }

    /// Every advertised per-model choice must survive the mapper for that same
    /// model — the picker can never offer an effort `run_turn` would drop.
    /// Iterating `CODEX_MODELS` also means a model added without tiers fails
    /// here rather than silently degrading to the fallback.
    #[test]
    fn advertised_model_choices_all_map_back() {
        for (model, levels) in CODEX_MODELS {
            assert!(!levels.is_empty(), "{model} has no reasoning tiers");
            for level in levels {
                assert_eq!(
                    codex_reasoning(Some(level), Some(model)),
                    Some(*level),
                    "{model} advertises {level} but the mapper drops it"
                );
            }
        }
    }

    fn answer(
        approve: bool,
        resume_mode: Option<&str>,
        answers: &[&str],
        note: Option<&str>,
    ) -> PromptAnswer {
        PromptAnswer {
            session_id: "s".into(),
            prompt_id: "p".into(),
            approve,
            resume_mode: resume_mode.map(str::to_string),
            answers: answers.iter().map(|s| s.to_string()).collect(),
            note: note.map(str::to_string),
            annotations: Vec::new(),
        }
    }

    #[test]
    fn collaboration_mode_json_shapes_the_mask() {
        // Envelope key `mode`; settings snake_case; developer_instructions null
        // (independent of the thread-level playbook channel); effort included.
        let plan = collaboration_mode_json("plan", "gpt-5.6-sol", Some("xhigh"));
        assert_eq!(plan["mode"], "plan");
        assert_eq!(plan["settings"]["model"], "gpt-5.6-sol");
        assert_eq!(plan["settings"]["reasoning_effort"], "xhigh");
        assert!(plan["settings"]["developer_instructions"].is_null());

        // Default kind; effort omitted → no `reasoning_effort` key at all.
        let default = collaboration_mode_json("default", "gpt-5.6-sol", None);
        assert_eq!(default["mode"], "default");
        assert_eq!(default["settings"]["model"], "gpt-5.6-sol");
        assert!(default["settings"].get("reasoning_effort").is_none());
        assert!(default["settings"]["developer_instructions"].is_null());
    }

    #[test]
    fn collaboration_mode_is_independent_from_permissions_and_resets_once() {
        assert_eq!(collaboration_mask_mode(true, false, None), Some("plan"));
        assert_eq!(collaboration_mask_mode(false, true, None), Some("default"));
        assert_eq!(
            collaboration_mask_mode(false, false, Some("plan")),
            Some("default")
        );
        assert_eq!(collaboration_mask_mode(false, false, None), None);
    }

    /// A plan turn: streamed deltas accumulate, the completed `plan` item is
    /// authoritative, and `plan_card` surfaces it as a NON-synthesized card.
    #[test]
    fn plan_deltas_accumulate_and_plan_card_is_authoritative() {
        let mut ctx = TurnCtx::test_stub();
        for delta in ["## Plan\n", "1. do X\n", "2. do Y\n"] {
            apply_notification(
                &mut ctx,
                "item/plan/delta",
                &serde_json::json!({"itemId":"plan_1","delta":delta,"threadId":"t","turnId":"turn1"}),
            );
        }
        // The completed plan item's text is authoritative (upserts the part the
        // deltas built).
        apply_notification(
            &mut ctx,
            "item/completed",
            &serde_json::json!({"item":{"type":"plan","id":"plan_1","text":"## Plan\n1. do X\n2. do Y\n"},"threadId":"t","turnId":"turn1"}),
        );
        let part = ctx
            .assistant
            .parts
            .iter()
            .find(|p| p.id == "plan-item-plan_1")
            .expect("plan part");
        assert_eq!(part.text.as_deref(), Some("## Plan\n1. do X\n2. do Y\n"));

        let card = plan_card(&ctx.assistant.parts, "msgA").expect("plan card");
        assert_eq!(card.id, "plan-synth-msgA");
        let prompt = card.prompt.as_ref().unwrap();
        assert_eq!(prompt.kind, "plan");
        assert!(!prompt.synthesized, "plan item is authoritative");
        assert_eq!(prompt.plan.as_deref(), Some("## Plan\n1. do X\n2. do Y\n"));
        assert!(prompt.native_id.is_none(), "end-turn card has no reply id");
    }

    /// A completed plan item with empty text never wipes the streamed deltas.
    #[test]
    fn empty_completed_plan_item_keeps_streamed_deltas() {
        let mut ctx = TurnCtx::test_stub();
        apply_notification(
            &mut ctx,
            "item/plan/delta",
            &serde_json::json!({"itemId":"plan_1","delta":"streamed plan"}),
        );
        apply_notification(
            &mut ctx,
            "item/completed",
            &serde_json::json!({"item":{"type":"plan","id":"plan_1","text":""}}),
        );
        let part = ctx
            .assistant
            .parts
            .iter()
            .find(|p| p.id == "plan-item-plan_1")
            .unwrap();
        assert_eq!(part.text.as_deref(), Some("streamed plan"));
    }

    /// No plan item, but a texty plan in the final message → a SYNTHESIZED card.
    #[test]
    fn plan_card_falls_back_to_texty_plan() {
        let mut ctx = TurnCtx::test_stub();
        ctx.upsert_part(WirePart::text(
            "msg_1",
            "Here's the plan: step one, step two.",
        ));
        let card = plan_card(&ctx.assistant.parts, "msgA").expect("synthesized card");
        let prompt = card.prompt.as_ref().unwrap();
        assert_eq!(prompt.kind, "plan");
        assert!(prompt.synthesized, "no plan item → synthesized from text");
        assert_eq!(
            prompt.plan.as_deref(),
            Some("Here's the plan: step one, step two.")
        );

        // Nothing to card → None (empty transcript, or only whitespace text).
        assert!(plan_card(&[], "msgA").is_none());
        let mut blank = TurnCtx::test_stub();
        blank.upsert_part(WirePart::text("msg_1", "   "));
        assert!(plan_card(&blank.assistant.parts, "msgA").is_none());
    }

    /// An errored plan turn's transcript → no synthesized card (the error is the
    /// surface, not a phantom approval). An authoritative plan item still cards.
    #[test]
    fn plan_card_suppressed_on_error_unless_plan_item_present() {
        let mut ctx = TurnCtx::test_stub();
        ctx.upsert_part(WirePart::text("msg_1", "partial plan"));
        ctx.push_error("boom".to_string());
        assert!(
            plan_card(&ctx.assistant.parts, "msgA").is_none(),
            "texty plan under an error is not carded"
        );
        // A real plan item is authoritative regardless of an error part.
        ctx.upsert_part(WirePart::text("plan-item-p1", "the plan"));
        let card = plan_card(&ctx.assistant.parts, "msgA").expect("plan item cards");
        assert!(!card.prompt.as_ref().unwrap().synthesized);
    }

    /// requestUserInput → a question card: the first non-secret question is
    /// surfaced, every question id is stashed for the multi-fill reply.
    #[test]
    fn user_input_card_surfaces_first_nonsecret_question() {
        let id = serde_json::json!(3);
        let params = serde_json::json!({
            "threadId":"t","turnId":"turn1","itemId":"call_1",
            "questions":[
                {"id":"q1","header":"Color","question":"Which color?","isOther":false,"isSecret":false,
                 "options":[{"label":"red","description":"warm"},{"label":"blue","description":null}]},
                {"id":"q2","header":"Size","question":"Which size?","isOther":false,"isSecret":false,"options":null},
            ],
        });
        let (part_id, part) = user_input_card(Some("turn1"), &id, &params).expect("card");
        assert_eq!(part_id, "appr-turn1-3");
        let prompt = part.prompt.as_ref().unwrap();
        assert_eq!(prompt.kind, "question");
        assert_eq!(prompt.header.as_deref(), Some("Color"));
        assert_eq!(prompt.question.as_deref(), Some("Which color?"));
        assert_eq!(prompt.native_id.as_deref(), Some("3"));
        assert_eq!(prompt.options.len(), 2);
        assert_eq!(prompt.options[0].label, "red");
        // Both question ids stashed; the surfaced one recorded.
        let ti = prompt.tool_input.as_ref().unwrap();
        assert_eq!(ti["questionIds"], serde_json::json!(["q1", "q2"]));
        assert_eq!(ti["answeredId"], "q1");
    }

    /// A secret first question is skipped for the first non-secret one; an
    /// all-secret call yields no card (never store secrets).
    #[test]
    fn user_input_card_skips_secret_questions() {
        let id = serde_json::json!(0);
        let mixed = serde_json::json!({
            "questions":[
                {"id":"s1","header":"Token","question":"API token?","isSecret":true,"options":null},
                {"id":"q2","header":"Env","question":"Which env?","isSecret":false,"options":null},
            ],
        });
        let (_, part) = user_input_card(Some("turn1"), &id, &mixed).expect("skips to non-secret");
        let prompt = part.prompt.unwrap();
        assert_eq!(prompt.header.as_deref(), Some("Env"));
        // Still stashes BOTH ids so the reply covers the secret one (empty).
        assert_eq!(
            prompt.tool_input.as_ref().unwrap()["questionIds"],
            serde_json::json!(["s1", "q2"])
        );

        let all_secret = serde_json::json!({
            "questions":[{"id":"s1","question":"secret?","isSecret":true,"options":null}],
        });
        assert!(user_input_card(Some("turn1"), &id, &all_secret).is_none());
    }

    /// The reply fills the surfaced id with the selection/note and every other
    /// stashed id empty; a bare (no selection, no note) answer errs.
    #[test]
    fn user_input_reply_fills_selected_and_empties_the_rest() {
        let prompt = WirePrompt {
            kind: "question".into(),
            tool_input: Some(serde_json::json!({
                "questionIds": ["q1", "q2"],
                "answeredId": "q1",
            })),
            ..Default::default()
        };
        // Selection labels fill q1; q2 gets an empty answer.
        let reply = user_input_reply(&prompt, &answer(true, None, &["red"], None)).unwrap();
        assert_eq!(
            reply["answers"]["q1"]["answers"],
            serde_json::json!(["red"])
        );
        assert_eq!(reply["answers"]["q2"]["answers"], serde_json::json!([]));

        // Note-only (freeform) answers the surfaced id.
        let reply = user_input_reply(&prompt, &answer(true, None, &[], Some("teal"))).unwrap();
        assert_eq!(
            reply["answers"]["q1"]["answers"],
            serde_json::json!(["teal"])
        );

        let mut annotated = answer(true, None, &[], Some("explain"));
        annotated.annotations = vec![crate::local::chat::TextAnnotation {
            text: "quoted excerpt".into(),
        }];
        let reply = user_input_reply(&prompt, &annotated).unwrap();
        let contextualized = reply["answers"]["q1"]["answers"][0].as_str().unwrap();
        let payload: Value = serde_json::from_str(contextualized.lines().last().unwrap()).unwrap();
        assert_eq!(payload["currentUserMessage"], "explain");
        assert_eq!(
            payload["selectedChatExcerpts"],
            serde_json::json!(["quoted excerpt"])
        );

        // Neither selection nor note → Err (card stays actionable).
        assert!(user_input_reply(&prompt, &answer(true, None, &[], None)).is_err());
    }

    /// Numeric question ids stringify to their JSON text as map keys.
    #[test]
    fn user_input_reply_stringifies_numeric_ids() {
        let prompt = WirePrompt {
            kind: "question".into(),
            tool_input: Some(serde_json::json!({
                "questionIds": [1, 2],
                "answeredId": 1,
            })),
            ..Default::default()
        };
        let reply = user_input_reply(&prompt, &answer(true, None, &["x"], None)).unwrap();
        assert_eq!(reply["answers"]["1"]["answers"], serde_json::json!(["x"]));
        assert_eq!(reply["answers"]["2"]["answers"], serde_json::json!([]));
    }

    fn plan_prompt_card() -> WirePrompt {
        // An end-turn plan card: no native_id, so `resume_from_prompt`'s plan
        // arm never touches the host (no busy-check / no client).
        WirePrompt {
            kind: "plan".into(),
            plan: Some("the plan".into()),
            synthesized: true,
            ..Default::default()
        }
    }

    fn test_resume_ctx() -> ResumeCtx {
        ResumeCtx {
            host: std::sync::Arc::new(crate::local::chat::ChatHost::new(
                std::sync::Arc::new(crate::local::opencode::AgentHost::new(None)),
                std::sync::Arc::new(crate::local::codex::CodexHost::new()),
                std::sync::Arc::new(crate::local::claude::ClaudeHost::new()),
            )),
            session_id: "s".into(),
            native_session_id: None,
        }
    }

    /// The codex plan card resume arms: approve → "Implement the plan." with
    /// permission unchanged; revise → shared plan-deny wording in Plan mode;
    /// note-less reject → Nothing.
    #[tokio::test]
    async fn plan_resume_arms() {
        let ctx = test_resume_ctx();
        let card = plan_prompt_card();

        // Approve leaves Plan without changing the selected permission mode.
        let action = Codex
            .resume_from_prompt(&ctx, &card, &answer(true, None, &[], None))
            .await
            .unwrap();
        match action {
            ResumeAction::SendMessage {
                text,
                mode,
                plan_mode,
            } => {
                assert_eq!(text, "Implement the plan.");
                assert_eq!(mode, None);
                assert_eq!(plan_mode, Some(false));
            }
            _ => panic!("approve should send a message"),
        }

        // A stale resumeMode cannot overwrite Codex's permission choice.
        let action = Codex
            .resume_from_prompt(
                &ctx,
                &card,
                &answer(true, Some("bypassPermissions"), &[], Some("skip tests")),
            )
            .await
            .unwrap();
        match action {
            ResumeAction::SendMessage {
                text,
                mode,
                plan_mode,
            } => {
                assert!(text.contains("Implement the plan."));
                assert!(text.contains("skip tests"));
                assert_eq!(mode, None);
                assert_eq!(plan_mode, Some(false));
            }
            _ => panic!("approve should send a message"),
        }

        // Revise (note-carrying reject) → shared wording, stays in Plan.
        let action = Codex
            .resume_from_prompt(&ctx, &card, &answer(false, None, &[], Some("tweak X")))
            .await
            .unwrap();
        let (shared_text, _) =
            synthesize_resume("plan", &answer(false, None, &[], Some("tweak X")));
        match action {
            ResumeAction::SendMessage {
                text,
                mode,
                plan_mode,
            } => {
                assert_eq!(text, shared_text, "revise reuses Claude's wording");
                assert_eq!(mode, None);
                assert_eq!(plan_mode, Some(true));
            }
            _ => panic!("revise should send a message"),
        }

        // Note-less reject → close the card, no resume.
        let action = Codex
            .resume_from_prompt(&ctx, &card, &answer(false, None, &[], None))
            .await
            .unwrap();
        assert!(matches!(action, ResumeAction::Nothing));
    }

    #[test]
    fn legacy_exec_plan_mode_injects_an_explicit_planning_contract() {
        let planned = legacy_exec_text("investigate this", true);
        assert!(planned.contains("<plan-mode>"));
        assert!(planned.contains("Do not modify files"));
        assert!(planned.ends_with("investigate this"));
        assert_eq!(legacy_exec_text("implement this", false), "implement this");
    }

    /// server_req_kind classifies the three reply schemas the settle paths key
    /// on.
    #[test]
    fn server_req_kind_classifies_reply_schemas() {
        use crate::local::codex::{server_req_kind, ServerReqKind};
        assert_eq!(
            server_req_kind("item/commandExecution/requestApproval"),
            ServerReqKind::Approval
        );
        assert_eq!(
            server_req_kind("item/fileChange/requestApproval"),
            ServerReqKind::Approval
        );
        assert_eq!(
            server_req_kind("item/tool/requestUserInput"),
            ServerReqKind::UserInput
        );
        // A reply schema we don't speak (permission-profile object).
        assert_eq!(
            server_req_kind("item/permissions/requestApproval"),
            ServerReqKind::Other
        );
    }

    #[test]
    fn turn_completed_reports_input_plus_output_tokens() {
        // Real shape captured 2026-07-22 from codex 0.144.0 exec, here delivered
        // over the app-server as `turn/completed` with the usage nested.
        let mut ctx = TurnCtx::test_stub();
        ctx.model = Some("gpt-5.6-sol".into());
        let params = serde_json::json!({
            "turn": {
                "status": "completed",
                "usage": {"input_tokens":21498,"cached_input_tokens":9984,"output_tokens":5,"reasoning_output_tokens":0},
                "model_context_window": 272000
            }
        });
        let end = apply_notification(&mut ctx, "turn/completed", &params);
        assert!(matches!(end, Some(TurnEnd::Done { interrupted: false })));
        let usage = ctx.context_usage.expect("usage reported");
        // cached_input_tokens is a subset of input_tokens, not additive.
        assert_eq!(usage.used_tokens, 21498 + 5);
        assert_eq!(usage.context_window, Some(272000));
    }

    #[test]
    fn turn_completed_reads_top_level_usage_when_turn_lacks_it() {
        let mut ctx = TurnCtx::test_stub();
        let params = serde_json::json!({
            "turn": {"status": "completed"},
            "usage": {"input_tokens":100,"output_tokens":20}
        });
        apply_notification(&mut ctx, "turn/completed", &params);
        assert_eq!(ctx.context_usage.unwrap().used_tokens, 120);
    }

    #[test]
    fn legacy_token_count_prefers_last_usage_and_reads_window() {
        // The `token_count` legacy-exec info the loop's arm folds via
        // `token_count_usage`: `last_token_usage` (the latest request, whose
        // input already carries the full context) wins over `total_token_usage`
        // (a running session-wide sum). model_context_window comes along.
        let info = serde_json::json!({
            "total_token_usage": {"input_tokens":999999,"cached_input_tokens":9984,"output_tokens":50,"reasoning_output_tokens":0},
            "last_token_usage": {"input_tokens":21498,"cached_input_tokens":9984,"output_tokens":5,"reasoning_output_tokens":0},
            "model_context_window": 272000
        });
        // cached_input_tokens is a subset of input_tokens, not additive.
        assert_eq!(token_count_usage(&info), (Some(21498 + 5), Some(272000)));

        // No last → fall back to total.
        let total_only = serde_json::json!({
            "total_token_usage": {"input_tokens":100,"output_tokens":20},
            "model_context_window": 272000
        });
        assert_eq!(token_count_usage(&total_only), (Some(120), Some(272000)));
    }

    #[test]
    fn codex_used_tokens_is_none_when_absent_or_all_zero() {
        assert_eq!(codex_used_tokens(None), None);
        assert_eq!(codex_used_tokens(Some(&serde_json::json!({}))), None);
        // An all-zero payload isn't real occupancy — must not render "0%".
        assert_eq!(
            codex_used_tokens(Some(
                &serde_json::json!({"input_tokens":0,"output_tokens":0})
            )),
            None
        );
    }
}
