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
//! POST. `Bypass` mode auto-resolves permission cards (replies "always" without
//! a blocking card); `Auto`/`Plan` surface them. Questions always need a human,
//! so they always surface regardless of mode.
//!
//! Detection: opencode's `auth.json` is `{provider: {type}}`; the signed-in
//! providers are its account line, and `opencode models --verbose` is the model
//! list plus each model's reasoning `variants` (plain `opencode models` is the
//! fallback for a CLI too old for `--verbose`).

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};

use super::detect::{bin_version, read_json, HarnessInfo};
use super::options::{HarnessOptions, PermissionMode, REASONING_DEFAULT_ID};
use super::{Harness, ResumeAction};
use crate::error::{anyhow, Result};
use crate::local::chat::{
    ContextUsage, PromptAnswer, ResumeCtx, TurnCtx, WirePart, WirePrompt, WireQuestionOption,
    WireToolState,
};
use crate::local::opencode::find_opencode;

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

    async fn detect(&self) -> Option<HarnessInfo> {
        let mut info = HarnessInfo::new(self.id(), self.name());
        let mut models = Vec::new();
        if let Ok(bin) = find_opencode() {
            info.installed = true;
            info.version = bin_version(&bin).await;
            models = opencode_models(&bin).await;
            info.bin_path = Some(bin.to_string_lossy().into_owned());
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
        info.agent_ready = info.installed && info.authenticated;
        if info.agent_ready {
            info.models = models;
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

    async fn run_turn(&self, ctx: &mut TurnCtx) -> Result<()> {
        run_turn(ctx).await
    }

    fn options(&self) -> HarnessOptions {
        // Two native OpenCode axes folded onto the one Mode toggle:
        //  * which built-in agent runs — `plan` (read-only: allows inspection
        //    like `orx …`, denies edits) vs `build` (the default). A real, clean
        //    plan mode, unlike Claude/Codex — verified live.
        //  * how a `permission.asked` is answered. NOTE opencode's default is
        //    permissive (`allow *`); it only prompts on a few risky cases
        //    (runaway loops, out-of-workspace writes, `.env` reads), so a
        //    dedicated "ask for everything" mode would be hollow (cards would
        //    almost never fire). So we don't offer one:
        //      * Plan   → plan agent, and surface the rare cards that do fire.
        //      * Auto   → build agent, opencode's permissive default (still
        //                 surfaces those rare cards / questions).
        //      * Bypass → build agent, auto-approve even those.
        // Reasoning IS a model property in opencode, so there is no meaningful
        // harness-wide list: the real choices are each model's `variants`, read
        // from `opencode models --verbose` in `detect` and attached per-model.
        // Leaving this axis empty means a model with no variants shows no
        // picker at all, rather than falling back to a bogus union.
        HarnessOptions::none().with_permission_modes(
            &[
                PermissionMode::Plan,
                PermissionMode::Auto,
                PermissionMode::Bypass,
            ],
            PermissionMode::Auto,
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
        reply_inline(ctx, prompt, answer).await?;
        Ok(ResumeAction::Handled)
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
    let base = std::env::var_os("XDG_DATA_HOME")
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

/// Run `opencode <args>` in the home dir, returning stdout on success.
async fn run_models(bin: &PathBuf, args: &[&str]) -> Option<String> {
    let fut = tokio::process::Command::new(bin)
        .args(args)
        .current_dir(dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .stdin(std::process::Stdio::null())
        .output();
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
            })
        }
        _ => None,
    }
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
fn question_card(props: &Value) -> Option<WirePrompt> {
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
fn opencode_agent(mode: Option<PermissionMode>) -> &'static str {
    match mode {
        Some(PermissionMode::Plan) => "plan",
        _ => "build",
    }
}

async fn run_turn(ctx: &mut TurnCtx) -> Result<()> {
    // Lazy bring-up: spawns serve in this session's worktree or reuses the
    // session's live child.
    let status = ctx
        .host
        .opencode
        .ensure(&ctx.project, &ctx.session_id)
        .await?;
    let port = status
        .port
        .ok_or_else(|| anyhow!("opencode agent has no port"))?;
    let base = format!("http://127.0.0.1:{port}");

    let native_id = match &ctx.native_session_id {
        Some(id) => id.clone(),
        None => {
            let session: Value = ctx
                .http()
                .post(format!("{base}/session"))
                .header("content-type", "application/json")
                .body("{}")
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let id = session
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("opencode session response had no id"))?
                .to_string();
            ctx.set_native_session_id(&id);
            id
        }
    };

    // Subscribe before sending so no early part events are missed.
    let events = ctx
        .http()
        .get(format!("{base}/event"))
        .send()
        .await?
        .error_for_status()?;
    let mut stream = events.bytes_stream();

    let mut body = json!({
        "parts": [{ "type": "text", "text": ctx.text }],
        // Select opencode's built-in agent from the session's mode: `plan` (the
        // read-only planning agent — allows inspection, denies edits) vs `build`
        // (the default). The message endpoint takes `agent` directly (verified),
        // so no separate switch call is needed.
        "agent": opencode_agent(ctx.permission_mode),
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
    let send = ctx
        .http()
        .post(format!("{base}/session/{native_id}/message"))
        .json(&body)
        .send();
    tokio::pin!(send);

    // Parts are attributed via message.updated role info; a part arriving
    // before its message would be misfiled, and assistant messages are always
    // announced before their parts stream.
    let mut assistant_msgs: HashSet<String> = HashSet::new();
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
                    // Interactive prompts (permission/question) pause the turn and
                    // are handled async (emit a card, or auto-reply per mode); all
                    // other events are message/part updates handled synchronously.
                    if !handle_prompt_event(ctx, &native_id, &base, &event).await? {
                        handle_event(ctx, &native_id, &event, &mut assistant_msgs);
                    }
                }
            }
            resp = &mut send => {
                // Turn done — the response body is the final assistant message;
                // merge its parts as the authoritative versions.
                let resp = resp?.error_for_status()?;
                if let Ok(message) = resp.json::<Value>().await {
                    if let Some(parts) = message.get("parts").and_then(Value::as_array) {
                        for part in parts {
                            if let Some(wire) = to_wire_part(part) {
                                ctx.upsert_part(wire);
                            }
                        }
                    }
                }
                return Ok(());
            }
        }
    }
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
) {
    let props = event.get("properties").unwrap_or(&Value::Null);
    match event.get("type").and_then(Value::as_str) {
        Some("message.updated") => {
            let info = props.get("info").unwrap_or(&Value::Null);
            if info.get("sessionID").and_then(Value::as_str) == Some(native_id)
                && info.get("role").and_then(Value::as_str) == Some("assistant")
            {
                if let Some(id) = info.get("id").and_then(Value::as_str) {
                    assistant_msgs.insert(id.to_string());
                }
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
            if part.get("sessionID").and_then(Value::as_str) != Some(native_id) {
                return;
            }
            let owned_by_assistant = part
                .get("messageID")
                .and_then(Value::as_str)
                .is_some_and(|mid| assistant_msgs.contains(mid));
            if !owned_by_assistant {
                return;
            }
            if let Some(wire) = to_wire_part(part) {
                ctx.upsert_part(wire);
                ctx.maybe_flush();
            }
        }
        Some("message.part.delta") => {
            if props.get("sessionID").and_then(Value::as_str) != Some(native_id) {
                return;
            }
            if props.get("field").and_then(Value::as_str) != Some("text") {
                return;
            }
            if let (Some(part_id), Some(delta)) = (
                props.get("partID").and_then(Value::as_str),
                props.get("delta").and_then(Value::as_str),
            ) {
                ctx.append_part_text(part_id, delta);
                ctx.maybe_flush();
            }
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
/// Permissions honor the session's mode: only `Bypass` auto-replies `always`
/// over the live session (no blocking card); `Auto`/`Plan` (and anything else)
/// surface a card and pause — opencode's default is already permissive, so the
/// rare card it raises is worth showing. Questions always surface — there's no
/// sensible auto-answer. A single flaky auto-reply must not lose the whole turn,
/// so on POST failure we fall back to surfacing the card rather than erroring.
async fn handle_prompt_event(
    ctx: &mut TurnCtx,
    native_id: &str,
    base: &str,
    event: &Value,
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
            // Only Bypass auto-approves. Auto is opencode's permissive default —
            // the rare card it does raise (out-of-workspace write, `.env` read)
            // is worth surfacing; Plan surfaces them too.
            let auto_approve = matches!(ctx.permission_mode, Some(PermissionMode::Bypass));
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
            match question_card(props) {
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
        assert_eq!(opencode_agent(Some(PermissionMode::Plan)), "plan");
        assert_eq!(opencode_agent(Some(PermissionMode::Ask)), "build");
        assert_eq!(opencode_agent(Some(PermissionMode::Auto)), "build");
        assert_eq!(opencode_agent(Some(PermissionMode::Bypass)), "build");
        // No mode set → the default build agent, never plan.
        assert_eq!(opencode_agent(None), "build");
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
        let card = question_card(&props).expect("should parse");
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
        assert!(!question_card(&claude_shaped).unwrap().multi_select);
        // No questions → no card.
        assert!(question_card(&json!({ "id": "que_1" })).is_none());
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
        handle_event(&mut ctx, "ses_x", &event, &mut msgs);
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
        handle_event(&mut ctx, "ses_x", &no_tokens, &mut msgs);
        assert!(ctx.context_usage.is_none());
        // All-zero placeholder tokens must also be ignored.
        let zero_tokens = json!({
            "type": "message.updated",
            "properties": { "info": { "id": "msg_1", "sessionID": "ses_x", "role": "assistant",
                "tokens": { "input": 0, "output": 0, "reasoning": 0, "cache": { "read": 0, "write": 0 } }}}
        });
        handle_event(&mut ctx, "ses_x", &zero_tokens, &mut msgs);
        assert!(ctx.context_usage.is_none());
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
}
