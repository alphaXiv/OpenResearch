//! OpenCode harness.
//!
//! Chat: talks to a lazily spawned `opencode serve` child (the `AgentHost` the
//! up server shares), OpenCode 2 only. HTTP on loopback is just this adapter's
//! transport, never exposed to the browser; `opencode_v2.rs` owns the turn and
//! its event stream.
//!
//! Interactive prompts: unlike Claude (which ends its turn and resumes with a
//! new message), opencode approves *inline*. The server holds a permission or
//! question request while the prompt is still open — the turn is paused, not
//! finished. We surface those as `permission` / `question` cards and reply over
//! the live session (`resume_from_prompt` → [`ResumeAction::Handled`]). Full
//! access resolves native `ask` requests without a card; Manual surfaces them;
//! Auto has orx's reviewer resolve them or surfaces them. Questions always need
//! a human, so they always surface.
//!
//! Detection: opencode's `auth.json` is `{provider: {type}}`; the signed-in
//! providers are its account line, and the served API supplies the model list
//! and each model's reasoning `variants`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::detect::{read_json, BinProbe, HarnessAuthState, HarnessInfo, ModelInfo};
use super::options::{
    HarnessOptions, OptionChoice, PermissionMode, PlanActivation, REASONING_DEFAULT_ID,
};
use super::{
    CompactCtx, CompactOutcome, Harness, OneShot, ResumeAction, TurnFailure, TurnOutcome,
    TurnResult,
};
use crate::error::{anyhow, Result};
use crate::local::chat::{
    ContextUsage, DeliveryState, PromptAnswer, ResumeCtx, TurnCtx, WirePart, WirePrompt,
    WireQuestionOption, WireToolState,
};
use crate::local::local_models::is_loopback_url;
use crate::local::native_store::{self, NativeStore};
use crate::local::opencode::find_opencode;

#[path = "opencode_v2.rs"]
mod v2;

pub struct OpenCode;

impl OpenCode {
    /// `snapshot` skips every catalog probe and the server bring-up and answers
    /// install/auth only; readiness is deferred entirely and the entry is marked
    /// `catalog_pending` until a full pass replaces it. A full pass resolves
    /// OpenCode 2 and hands over to [`v2::detect`]; what is left here is an
    /// OpenCode that is missing or does not run.
    async fn detect_at(&self, snapshot: bool) -> Option<HarnessInfo> {
        let mut info = HarnessInfo::new(self.id(), self.name());
        let mut config = Value::Null;
        if let Ok(discovered) = find_opencode() {
            if snapshot {
                // Discovery already proves the file is there, and the served
                // API alone answers a real sign-in.
                info.record_bin(&discovered, BinProbe::Unknown);
                config = snapshot_config().0;
            } else {
                match super::detect::timed_probe(
                    "opencode",
                    "resolve",
                    crate::local::opencode::resolve_binary(),
                )
                .await
                {
                    Ok(binary) => {
                        info.record_bin(
                            &binary.path,
                            BinProbe::Answered(Some(binary.version.clone())),
                        );
                        return Some(v2::detect(binary, info).await);
                    }
                    // Report what discovery first named, so the UI shows a
                    // broken install rather than "not detected".
                    Err(error) => info.record_bin(&discovered, BinProbe::Broken(error.to_string())),
                }
            }
        }
        let providers: Vec<_> = opencode_providers()
            .into_iter()
            .filter(|id| provider_enabled(&config, id))
            .collect();
        if !providers.is_empty() {
            info.authenticated = true;
            info.auth_method = Some("oauth");
            info.account = Some(providers.join(", "));
        }
        // opencode also takes provider keys straight from the environment,
        // writing no auth.json — same fallback claude.rs has. Checked against
        // orx's synced env too, since that's a source the harness child gets
        // but this process may not.
        const PROVIDER_KEYS: &[(&str, &str)] = &[
            ("anthropic", "ANTHROPIC_API_KEY"),
            ("openai", "OPENAI_API_KEY"),
            ("openrouter", "OPENROUTER_API_KEY"),
            ("google", "GEMINI_API_KEY"),
            ("google", "GOOGLE_API_KEY"),
            ("groq", "GROQ_API_KEY"),
            ("xai", "XAI_API_KEY"),
            ("deepseek", "DEEPSEEK_API_KEY"),
        ];
        if !info.authenticated
            && (crate::local::local_models::configures_remote()
                || PROVIDER_KEYS.iter().any(|(id, key)| {
                    provider_enabled(&config, id) && super::detect::api_key(key).is_some()
                }))
        {
            info.authenticated = true;
            info.auth_method = Some("apiKey");
        }
        info.configured_model = config
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if !info.authenticated && !local_providers(&config).is_empty() {
            info.auth_method = Some("local");
        }
        if info.install_broken {
            info.agent_note = Some(info.broken_note("Reinstall it"));
        } else if !info.installed {
            info.agent_note = Some(
                "Install OpenCode, then configure a local model or sign in with `opencode auth login`."
                    .to_string(),
            );
        }
        if info.auth_state == HarnessAuthState::Unknown && !config.is_null() {
            info.auth_state = if info.authenticated {
                HarnessAuthState::Ready
            } else {
                HarnessAuthState::NeedsLogin
            };
        }
        if let Err(error) = crate::local::local_models::read() {
            info.agent_note = Some(error.to_string());
        }
        Some(info)
    }
}

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

    fn supports_compaction(&self) -> bool {
        true
    }

    async fn compact(&self, ctx: &CompactCtx) -> Result<CompactOutcome> {
        let Some(native_id) = ctx.native_session_id.as_deref() else {
            return Ok(CompactOutcome::Fallback);
        };
        ctx.host
            .opencode
            .compact(
                &ctx.project,
                &ctx.session_id,
                native_id,
                ctx.model.as_deref(),
            )
            .await?;
        Ok(CompactOutcome::Native)
    }

    async fn one_shot(&self, request: OneShot<'_>) -> Result<String> {
        let _in_use = super::managed::OPENCODE_IN_USE.read().await;
        let model = selected_generation_model(request.model)?.to_owned();
        let binary = crate::local::opencode::resolve_binary().await?;
        let message = format!("{}\n\n{}", request.system, request.prompt);
        v2::generate(binary, Some(model), message, request.timeout).await
    }

    async fn detect(&self) -> Option<HarnessInfo> {
        let mut info = self.detect_at(false).await?;
        info.found_providers =
            found_providers(info.models.iter().map(|model| model_provider(&model.id)));
        Some(info)
    }

    async fn detect_snapshot(&self) -> Option<HarnessInfo> {
        let mut info = self.detect_at(true).await?;
        info.found_providers = found_providers([]);
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
        // Manual makes every change ask, Auto makes shell commands and anything
        // outside the worktree ask and has orx's reviewer answer, and Full
        // access, the default since it is what OpenCode ran as before, answers
        // every native `ask`; none overrides an explicit deny.
        // Reasoning IS a model property in opencode, so there is no meaningful
        // harness-wide list: the real choices are each model's `variants`,
        // attached per-model in `detect`. Leaving this axis empty means a model
        // with no variants shows no picker at all, rather than falling back to
        // a bogus union.
        HarnessOptions::none().with_permission_choices(
            vec![
                OptionChoice::described("manual", "Manual", "Always ask before making changes"),
                OptionChoice::described(
                    "auto",
                    "Auto",
                    "Uses the same model to review each command, and asks you when unsure",
                ),
                OptionChoice::described(
                    "auto-approve",
                    "Full access",
                    "Approve every request without asking, except actions you have denied",
                ),
            ],
            "auto-approve",
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

    fn native_skill_roots(&self) -> Vec<PathBuf> {
        self.config_home()
            .map(|home| vec![home.join("skills")])
            .unwrap_or_default()
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
        .or_else(|| crate::config::home_dir().map(|h| h.join(".local").join("share")))?;
    Some(crate::config::test_contained(
        base.join("opencode").join("auth.json"),
    ))
}

/// Built-in providers OpenCode activates from a key in the environment, which writes no auth.json.
const PROVIDER_KEYS: &[(&str, &str)] = &[
    ("anthropic", "ANTHROPIC_API_KEY"),
    ("openai", "OPENAI_API_KEY"),
    ("openrouter", "OPENROUTER_API_KEY"),
    ("google", "GEMINI_API_KEY"),
    ("google", "GOOGLE_API_KEY"),
    ("groq", "GROQ_API_KEY"),
    ("xai", "XAI_API_KEY"),
    ("deepseek", "DEEPSEEK_API_KEY"),
    ("thinkingmachines", "TINKER_API_KEY"),
];

/// A provider set up in the user's own OpenCode rather than by OpenResearch.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FoundProvider {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub npm: Option<String>,
    /// The variables that hold its key: its config's own list, else the ones OpenCode reads.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
    pub sources: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_path: Option<PathBuf>,
    /// Deleting it from the user's OpenCode would succeed.
    pub removable: bool,
}

/// One layer of the user's own OpenCode configuration.
struct UserConfig {
    /// `None` for `OPENCODE_CONFIG_CONTENT`.
    path: Option<PathBuf>,
    value: Value,
    /// Strict JSON, which a rewrite keeps whole.
    rewritable: bool,
}

/// The user's OpenCode configuration as they wrote it, without the inline content orx gives its
/// own children: the stock directory, `OPENCODE_CONFIG`, `OPENCODE_CONFIG_DIR` and
/// `OPENCODE_CONFIG_CONTENT`, later layers overriding earlier ones. Layers orx can't read or
/// parse come back as errors beside the rest.
fn user_configs() -> (Vec<UserConfig>, Vec<String>) {
    let stock = super::xdg_config_home().join("opencode");
    let custom = crate::config::env_path("OPENCODE_CONFIG").map(crate::config::test_contained);
    let dir = crate::config::env_path("OPENCODE_CONFIG_DIR").map(crate::config::test_contained);
    let files = [stock.join("opencode.json"), stock.join("opencode.jsonc")]
        .into_iter()
        .chain(custom)
        .chain(
            dir.into_iter()
                .flat_map(|dir| [dir.join("opencode.json"), dir.join("opencode.jsonc")]),
        );
    let mut layers: Vec<UserConfig> = Vec::new();
    let mut unread = Vec::new();
    for path in files {
        if layers
            .iter()
            .any(|layer| layer.path.as_ref() == Some(&path))
        {
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                unread.push(format!("Could not read {}: {error}", path.display()));
                continue;
            }
        };
        let strict = serde_json::from_str::<Value>(&text).ok();
        let rewritable = strict.is_some();
        match strict.or_else(|| parse_jsonc(&text)) {
            Some(value) => layers.push(UserConfig {
                path: Some(path),
                value,
                rewritable,
            }),
            None => unread.push(format!(
                "{} is not a valid OpenCode config. Fix it, then try again.",
                path.display()
            )),
        }
    }
    if let Some(value) = crate::local::shell_env::var("OPENCODE_CONFIG_CONTENT")
        .and_then(|content| parse_jsonc(&content.to_string_lossy()))
    {
        layers.push(UserConfig {
            path: None,
            value,
            rewritable: false,
        });
    }
    (layers, unread)
}

/// JSON with the comments and trailing commas OpenCode's config parser accepts.
fn parse_jsonc(text: &str) -> Option<Value> {
    let uncommented = outside_strings(text, |c, rest, out| match c {
        '/' if rest.peek() == Some(&'/') => {
            out.extend(rest.by_ref().find(|c| *c == '\n'));
        }
        '/' if rest.peek() == Some(&'*') => {
            rest.next();
            let mut last = ' ';
            for c in rest.by_ref() {
                if last == '*' && c == '/' {
                    break;
                }
                last = c;
            }
            out.push(' ');
        }
        _ => out.push(c),
    });
    let strict = outside_strings(&uncommented, |c, rest, out| {
        let trailing =
            c == ',' && matches!(rest.clone().find(|c| !c.is_whitespace()), Some('}' | ']'));
        if !trailing {
            out.push(c);
        }
    });
    serde_json::from_str(&strict).ok().filter(Value::is_object)
}

/// Copies `text`, passing each character outside a JSON string to `step`.
fn outside_strings(
    text: &str,
    mut step: impl FnMut(char, &mut std::iter::Peekable<std::str::Chars>, &mut String),
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '"' {
            step(c, &mut chars, &mut out);
            continue;
        }
        out.push(c);
        while let Some(c) = chars.next() {
            out.push(c);
            match c {
                '\\' => out.extend(chars.next()),
                '"' => break,
                _ => {}
            }
        }
    }
    out
}

/// Providers set up in the user's own OpenCode that no record or endpoint of OpenResearch's
/// stands for: its sign-ins, its config's provider blocks, keys in the environment, and any other
/// provider `catalog` lists models from.
fn found_providers<'a>(catalog: impl IntoIterator<Item = &'a str>) -> Vec<FoundProvider> {
    let (layers, _) = user_configs();
    let records = crate::local::local_models::providers().unwrap_or_default();
    let endpoints = crate::local::local_models::read().unwrap_or_default();
    let owned = |id: &str| {
        id == "opencode"
            || id.starts_with(crate::local::model_routes::HOSTED_PROVIDER_PREFIX)
            || endpoints.contains_key(id)
            || records.contains_key(id)
    };
    let mut gates = json!({});
    for layer in &layers {
        for key in ["enabled_providers", "disabled_providers"] {
            if let Some(list) = layer.value.get(key) {
                gates[key] = list.clone();
            }
        }
    }

    let skip = |id: &str| owned(id) || !provider_enabled(&gates, id);
    let mut found = std::collections::BTreeMap::new();
    for id in opencode_providers() {
        if !skip(&id) {
            add_source(&mut found, &id, "auth");
        }
    }
    for layer in &layers {
        for (group, base_url, package) in CONFIG_GROUPS {
            let blocks = layer.value.get(group).and_then(Value::as_object);
            for (id, block) in blocks.into_iter().flatten().filter(|(id, _)| !skip(id)) {
                let entry = add_source(&mut found, id, "config");
                let text = |pointer| block.pointer(pointer).and_then(Value::as_str);
                if let Some(name) = text("/name") {
                    entry.name = Some(name.to_owned());
                }
                if let Some(url) = text(base_url) {
                    entry.base_url = Some(url.to_owned());
                }
                if let Some(npm) = text(package) {
                    entry.npm = Some(npm.to_owned());
                }
                if let Some(env) = block.get("env").and_then(Value::as_array) {
                    entry.env = env
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect();
                }
                if entry.config_path.is_none() {
                    entry.config_path.clone_from(&layer.path);
                }
                entry.removable &= layer.rewritable;
            }
        }
    }
    for (id, key) in PROVIDER_KEYS {
        if !skip(id) && super::detect::api_key(key).is_some() {
            let entry = add_source(&mut found, id, "env");
            entry.removable = false;
            if !entry.sources.contains(&"config") {
                entry.env.push((*key).to_owned());
            }
        }
    }
    for id in catalog {
        if !skip(id) && !found.contains_key(id) {
            add_source(&mut found, id, "catalog").removable = false;
        }
    }
    found.into_values().collect()
}

/// The config groups that set up a provider, with where each keeps its address and package:
/// OpenCode 1's `provider` and OpenCode 2's `providers`.
const CONFIG_GROUPS: [(&str, &str, &str); 2] = [
    ("provider", "/options/baseURL", "/npm"),
    ("providers", "/settings/baseURL", "/package"),
];

/// A provider id as OpenCode accepts one: any key of its config or auth.json that is not a path.
pub(crate) fn is_found_provider_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id != "opencode"
        && !id.contains('/')
        && !id.chars().any(char::is_control)
}

fn add_source<'a>(
    found: &'a mut std::collections::BTreeMap<String, FoundProvider>,
    id: &str,
    source: &'static str,
) -> &'a mut FoundProvider {
    let entry = found.entry(id.to_owned()).or_insert_with(|| FoundProvider {
        id: id.to_owned(),
        name: None,
        base_url: None,
        npm: None,
        env: Vec::new(),
        sources: Vec::new(),
        config_path: None,
        removable: true,
    });
    if !entry.sources.contains(&source) {
        entry.sources.push(source);
    }
    entry
}

/// Deletes `found` from the user's own OpenCode: its sign-in in auth.json and its provider block
/// in each config file. Changes nothing unless every source can go, so a provider that stays set
/// up by something orx cannot edit keeps its sign-in too.
pub(crate) fn delete_found_provider(id: &str) -> Result<()> {
    static REWRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _lock = REWRITE.lock().unwrap_or_else(|error| error.into_inner());
    let (layers, unread) = user_configs();
    if let Some(problem) = unread.into_iter().next() {
        return Err(anyhow!("{problem}"));
    }
    let found = found_providers([])
        .into_iter()
        .find(|found| found.id == id)
        .ok_or_else(|| FoundProviderGone(id.to_owned()))?;
    if let Some((_, var)) = PROVIDER_KEYS
        .iter()
        .find(|(provider, key)| *provider == id && super::detect::api_key(key).is_some())
    {
        return Err(anyhow!(
            "`{var}` in your environment provides {id}. Remove it there, or turn this provider off."
        ));
    }
    let defines = |config: &Value| {
        CONFIG_GROUPS.iter().any(|(group, ..)| {
            config
                .get(group)
                .and_then(|blocks| blocks.get(id))
                .is_some()
        })
    };
    let layers: Vec<_> = layers
        .into_iter()
        .filter(|layer| defines(&layer.value))
        .collect();
    for layer in &layers {
        if layer.rewritable {
            continue;
        }
        return Err(match &layer.path {
            Some(path) => anyhow!(
                "OpenResearch can't edit {} because it has comments. Remove the `{id}` provider there, then re-check OpenCode.",
                path.display()
            ),
            None => anyhow!(
                "`OPENCODE_CONFIG_CONTENT` in your environment sets up {id}. Remove it there, or turn this provider off."
            ),
        });
    }
    for path in layers.iter().filter_map(|layer| layer.path.as_ref()) {
        rewrite_json(path, None, |config| {
            for (group, ..) in CONFIG_GROUPS {
                if let Some(blocks) = config.get_mut(group).and_then(Value::as_object_mut) {
                    blocks.retain(|key, _| key != id);
                }
            }
        })?;
    }
    if found.sources.contains(&"auth") {
        if let Some(path) = opencode_auth_path() {
            rewrite_json(&path, Some(0o600), |auth| {
                if let Some(auth) = auth.as_object_mut() {
                    auth.retain(|key, _| key != id);
                }
            })?;
        }
    }
    Ok(())
}

/// No source orx can edit sets up this provider any more.
#[derive(Debug)]
pub struct FoundProviderGone(String);

impl std::fmt::Display for FoundProviderGone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "OpenResearch can't find where your OpenCode sets up {}. Remove it in OpenCode, or turn this provider off.",
            self.0
        )
    }
}

impl std::error::Error for FoundProviderGone {}

/// Rewrites a JSON file of the user's in place, through a link to the file it names.
fn rewrite_json(path: &Path, mode: Option<u32>, change: impl FnOnce(&mut Value)) -> Result<()> {
    let path = std::fs::canonicalize(path)
        .map_err(|error| anyhow!("Could not read {}: {error}", path.display()))?;
    let text = std::fs::read_to_string(&path)
        .map_err(|error| anyhow!("Could not read {}: {error}", path.display()))?;
    let mut value: Value = serde_json::from_str(&text).map_err(|error| {
        anyhow!(
            "{} is not valid JSON ({error}). Fix it, then try again.",
            path.display()
        )
    })?;
    change(&mut value);
    let mut bytes = serde_json::to_vec_pretty(&value)?;
    bytes.push(b'\n');
    crate::local::git::atomic_write_with_mode(&path, &bytes, mode)
        .map_err(|error| anyhow!("Could not write {}: {error}", path.display()))
}

/// The cheap config read for the snapshot pass: `OPENCODE_CONFIG_CONTENT`,
/// the file `OPENCODE_CONFIG` points at, else `opencode.json` under
/// `OPENCODE_CONFIG_DIR` or the stock `~/.config/opencode`. `debug config
/// --pure` additionally merges project and plugin layers, but it costs a
/// child process — the snapshot settles for the file and the full pass
/// re-reads it properly.
///
/// One merge the file read must not skip: orx's own connected local models,
/// which `local_models::prepare_env` folds into `OPENCODE_CONFIG_CONTENT` for
/// every spawned CLI — and inline content shadows the file entirely, so a
/// connections-only user sees exactly those providers and nothing else.
///
/// Returns the config plus whether a source exists the strict parse could
/// not honor (`.jsonc`, comments) — a cue for the full pass to ask the CLI.
fn snapshot_config() -> (Value, bool) {
    let connections = crate::local::local_models::read().unwrap_or_default();
    // `true` when a config source exists that the strict file read cannot
    // honor — opencode accepts `.jsonc` and comments, which parse as `Null`
    // here and would silently drop configured providers. The full pass
    // resolves those with `debug config --pure`.
    let mut unresolved = false;
    let mut config = if let Some(content) = crate::local::shell_env::var("OPENCODE_CONFIG_CONTENT")
    {
        // The env var overrides the file entirely — present-but-unparseable
        // means "no usable config", not "fall through to the file" — but its
        // comments still outrun the strict parse.
        let parsed = serde_json::from_str::<Value>(&content.to_string_lossy());
        unresolved = parsed.is_err();
        parsed.unwrap_or(Value::Null)
    } else if !connections.is_empty() {
        // The spawned CLI would get inline content built from the connections
        // alone; the file never enters the picture.
        json!({})
    } else if let Some(custom) = crate::local::shell_env::var("OPENCODE_CONFIG") {
        let path = crate::config::test_contained(PathBuf::from(custom));
        let parsed = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        unresolved = path.exists() && parsed.is_none();
        parsed.unwrap_or(Value::Null)
    } else {
        let dir = crate::local::shell_env::var("OPENCODE_CONFIG_DIR")
            .map(|dir| crate::config::test_contained(dir.into()))
            .unwrap_or_else(|| super::xdg_config_home().join("opencode"));
        let json = dir.join("opencode.json");
        let parsed = std::fs::read_to_string(&json)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        // opencode honors a sibling `.jsonc` alongside — and comments inside
        // — `.json`; either outruns what the file read can claim to cover.
        unresolved = dir.join("opencode.jsonc").exists() || (json.exists() && parsed.is_none());
        parsed.unwrap_or(Value::Null)
    };
    if !connections.is_empty() && config.is_object() {
        let _ = crate::local::local_models::merge_config(&mut config, &connections, None);
    }
    (config, unresolved)
}

/// Whether the user keeps OpenCode's free models on; they are unless switched off.
pub(crate) fn free_models_offered() -> bool {
    crate::store::Store::open()
        .and_then(|store| store.ui_state())
        .map_or(true, |state| state.opencode_free_models)
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

fn model_provider(id: &str) -> &str {
    id.split_once('/').map_or(id, |(provider, _)| provider)
}

fn local_providers(config: &Value) -> Vec<(&str, &Value)> {
    config
        .get("provider")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(id, provider)| {
            provider_enabled(config, id)
                && provider
                    .pointer("/options/baseURL")
                    .and_then(Value::as_str)
                    .is_some_and(is_loopback_url)
        })
        .map(|(id, provider)| (id.as_str(), provider))
        .collect()
}

fn provider_enabled(config: &Value, id: &str) -> bool {
    let contains = |key| {
        config.get(key).and_then(Value::as_array).map(|providers| {
            providers
                .iter()
                .any(|provider| provider.as_str() == Some(id))
        })
    };
    contains("enabled_providers").unwrap_or(true)
        && !contains("disabled_providers").unwrap_or(false)
}

fn selected_generation_model(model: Option<&str>) -> Result<&str> {
    model
        .filter(|model| !model.trim().is_empty())
        .ok_or_else(|| anyhow!("Select an OpenCode model before generating text."))
}

/// Session reasoning id → opencode's top-level `variant` value.
///
/// Only the `default` sentinel (and an absent level) send nothing; every other
/// value is forwarded as-is, since the ids come from opencode's own catalog and
/// opencode itself is the validator of last resort.
fn opencode_variant(level: Option<&str>) -> Option<&str> {
    level.filter(|l| *l != REASONING_DEFAULT_ID)
}

/// opencode part → wire part (the shapes are already close).
pub(crate) fn to_wire_part(part: &Value) -> Option<WirePart> {
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
            phase: None,
            children: Vec::new(),
            orx: None,
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
                phase: None,
                children: Vec::new(),
                orx: None,
            })
        }
        _ => None,
    }
}

/// The chosen labels, then a typed answer, which OpenCode takes as a label of
/// the user's own.
fn submitted_answers(answers: &[String], note: Option<&String>) -> Vec<String> {
    answers
        .iter()
        .chain(note.filter(|note| !note.trim().is_empty()))
        .cloned()
        .collect()
}

/// Deliver an answered card's reply to the live serve session, unblocking the
/// paused `session.prompt` POST. Permission → `{response: once|always|reject}`;
/// question → `{answers: [[label,...]]}` (or reject). The reply target is the
/// card's `native_id` (the opencode permission/question request id).
async fn reply_inline(ctx: &ResumeCtx, prompt: &WirePrompt, answer: &PromptAnswer) -> Result<()> {
    // The reply only lands if the turn is still paused waiting for it. If the
    // turn already ended (errored / interrupted), serve may still accept the
    // POST but no one is consuming the resumed stream, so the reply would be
    // lost and the card would falsely mark answered.
    if !ctx.is_busy().await {
        ctx.host
            .resolve_zombie_prompt(&ctx.session_id, &answer.prompt_id);
        return Err(anyhow!(
            "this turn is no longer running — its prompt can't be answered"
        ));
    }
    // Reach this session's live serve child through the shared host, exactly
    // as `ChatHost::interrupt` does — the reply goes to the same loopback
    // serve whose `session.prompt` POST is paused on this prompt.
    let endpoint = ctx
        .host
        .opencode
        .endpoint_for(&ctx.session_id)
        .await
        .ok_or_else(|| anyhow!("OpenCode serve is not running"))?;
    v2::reply(ctx, &endpoint, prompt, answer).await
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
    matches!(mode, Some(PermissionMode::Bypass))
}

fn plan_exit_transition(prompt: &WirePrompt, answer: &PromptAnswer) -> Option<bool> {
    prompt.plan_exit.then(|| {
        !answer
            .answers
            .iter()
            .any(|choice| choice.eq_ignore_ascii_case("yes"))
    })
}

async fn ensure_runtime(
    ctx: &mut TurnCtx,
    store: NativeStore,
    binary: crate::local::opencode::ResolvedBinary,
    database: PathBuf,
) -> Result<crate::local::opencode::AgentStatus> {
    let host = ctx.host.clone();
    let project = ctx.project.clone();
    let session = ctx.session_id.clone();
    let model = ctx.model.clone();
    let (sender, mut progress) = tokio::sync::watch::channel("Preparing OpenCode".to_string());
    let setup = host.opencode.ensure(
        &project,
        &session,
        model.as_deref(),
        crate::local::opencode::ResolvedRuntime {
            binary,
            database,
            store,
        },
        sender,
    );
    tokio::pin!(setup);
    let mut progress_open = true;
    let result = loop {
        tokio::select! {
            result = &mut setup => break result,
            changed = progress.changed(), if progress_open => {
                if changed.is_err() { progress_open = false; continue; }
                let mut part = WirePart::tool("opencode-setup", "OpenCode", "running", None);
                if let Some(state) = &mut part.state { state.title = Some(progress.borrow().clone()); }
                ctx.upsert_part(part);
                ctx.flush()?;
            }
        }
    };
    if let Some(state) = ctx
        .assistant
        .parts
        .iter_mut()
        .find(|part| part.id == "opencode-setup")
        .and_then(|part| part.state.as_mut())
    {
        state.status = if result.is_ok() { "completed" } else { "error" }.into();
        state.error = result.as_ref().err().map(ToString::to_string);
    }
    ctx.flush()?;
    result
}

async fn run_turn(ctx: &mut TurnCtx) -> Result<()> {
    if let Some(model) = ctx.model.as_deref() {
        crate::commands::up::model_feed::ensure_offered(model)?;
    }
    let _in_use = super::managed::OPENCODE_IN_USE.read().await;
    // Native permission and question requests die with their turn. Clear any
    // crash/restart leftovers before a new live request can be surfaced.
    ctx.host.resolve_stale_prompts(&ctx.session_id, true)?;

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
        if let Some(recovery) = super::native_recovery_context(ctx, "OpenCode")? {
            ctx.text = format!("{recovery}\n\n{}", ctx.text);
        }
        ctx.native_session_id = None;
    }

    let binary = crate::local::opencode::resolve_binary().await?;
    let database = match native_session {
        Some(session) => session.path,
        None => native_store::prepare_opencode(store)?,
    };
    v2::run_turn(ctx, store, binary, database).await
}

/// OpenCode assistant `tokens` occupying the context window:
/// `input + output + reasoning + cache.read + cache.write`. Returns `None` when
/// the object is absent, and `None` (not `Some(0)`) when every field is zero —
/// the early `message.updated` events carry an all-zero placeholder.
fn opencode_native_usage(tokens: &Value) -> crate::store::TokenUsage {
    let field = |name| tokens.get(name).and_then(Value::as_u64);
    let cache_read_tokens = tokens.pointer("/cache/read").and_then(Value::as_u64);
    let cache_write_tokens = tokens.pointer("/cache/write").and_then(Value::as_u64);
    let reasoning_tokens = field("reasoning");
    crate::store::TokenUsage {
        input_tokens: field("input").and_then(|input| {
            input
                .checked_add(cache_read_tokens?)
                .and_then(|n| n.checked_add(cache_write_tokens?))
        }),
        output_tokens: field("output").and_then(|output| output.checked_add(reasoning_tokens?)),
        cache_read_tokens,
        cache_write_tokens,
        reasoning_tokens,
    }
}

/// The dollars OpenCode reports a step or message cost.
fn opencode_cost(value: &Value) -> Option<crate::store::Cost> {
    value
        .get("cost")
        .and_then(Value::as_f64)
        .and_then(|dollars| crate::store::Cost::of(dollars, "USD"))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_overrides_outside_the_test_root_panic() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        for var in ["OPENCODE_CONFIG", "OPENCODE_CONFIG_DIR"] {
            std::env::set_var(var, "/orx-not-a-config");
            let read = std::panic::catch_unwind(snapshot_config);
            assert!(read.is_err(), "{var} read {:?}", read.unwrap());
            std::env::remove_var(var);
        }
    }

    #[tokio::test]
    async fn one_shot_without_a_model_fails_before_binary_resolution() {
        for model in [None, Some(""), Some(" \t")] {
            let error = OpenCode
                .one_shot(OneShot {
                    model,
                    ..super::super::title::title_request("hello")
                })
                .await
                .unwrap_err();
            assert!(error.to_string().contains("Select an OpenCode model"));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn v2_invalid_model_fails_before_launch() {
        let path = sh_script("unused", "#!/bin/sh\nexit 99\n");
        let binary = crate::local::opencode::resolve_binary_at(path.clone())
            .await
            .unwrap();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        for model in [
            None,
            Some(""),
            Some(" "),
            Some("bare"),
            Some("/model"),
            Some("provider/"),
            Some(" /model"),
        ] {
            let error = v2::generate(
                binary.clone(),
                model.map(str::to_owned),
                "hello".into(),
                Duration::from_secs(1),
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains("model"), "{error}");
            assert!(!error.to_string().contains("No such file"), "{error}");
        }
    }

    #[test]
    fn found_providers_come_from_the_users_own_opencode() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let dir = super::super::xdg_config_home().join("opencode");
        std::fs::create_dir_all(&dir).unwrap();
        let json = dir.join("opencode.json");
        std::fs::write(
            &json,
            r#"{
  "$schema": "https://opencode.ai/config.json",
  "model": "tinker/gpt-oss",
  "provider": {
    "tinker": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "Tinker",
      "options": {"baseURL": "https://tinker.thinkingmachines.dev/services/tinker-prod/oai/api/v1", "apiKey": "{env:TINKER_API_KEY}"},
      "env": ["TINKER_API_KEY"],
      "models": {"gpt-oss": {}}
    },
    "switched-off": {"options": {"baseURL": "https://example.com/v1"}}
  },
  "providers": {
    "team_router": {"name": "Team router", "package": "@ai-sdk/openai-compatible", "settings": {"baseURL": "https://llm.example.edu/v1"}}
  },
  "disabled_providers": ["switched-off"]
}"#,
        )
        .unwrap();
        let jsonc = dir.join("opencode.jsonc");
        std::fs::write(
            &jsonc,
            "{\n  // a lab box\n  \"provider\": {\"commented\": {\"name\": \"Lab, \\\"quoted\\\" // not a comment\"},},\n  /* done */\n}\n",
        )
        .unwrap();
        let auth = opencode_auth_path().unwrap();
        std::fs::create_dir_all(auth.parent().unwrap()).unwrap();
        std::fs::write(
            &auth,
            r#"{"openrouter":{"type":"api","key":"one"},"github-copilot":{"type":"oauth"},"anthropic":{"type":"api","key":"two"},"openai":{"type":"api","key":"three"}}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&auth, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let records = crate::local::local_models::providers_path();
        std::fs::create_dir_all(records.parent().unwrap()).unwrap();
        std::fs::create_dir_all(crate::store::data_dir()).unwrap();
        std::fs::write(
            records,
            r#"{"openai":{"kind":"key","key":"sk-test","name":"OpenAI"},"anthropic-abc123":{"kind":"key","key":"sk-test","canonical":"anthropic","name":"Second"}}"#,
        )
        .unwrap();
        std::fs::write(
            crate::store::data_dir().join("local-models.json"),
            r#"{"orx-local-1":{"name":"Box","baseUrl":"http://127.0.0.1:1234/v1","models":{"m":4096}}}"#,
        )
        .unwrap();
        std::env::set_var("TINKER_API_KEY", "fixture-only");
        std::env::set_var("XAI_API_KEY", "fixture-only");

        let snapshot = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(OpenCode.detect_snapshot())
            .unwrap();
        assert!(snapshot
            .found_providers
            .iter()
            .any(|found| found.id == "tinker"));
        let found = found_providers([
            "mistral",
            "opencode",
            "openresearch-k3v9q2",
            "orx-local-1",
            "tinker",
            "openai",
        ]);
        let shown = serde_json::to_value(&found).unwrap();
        assert_eq!(
            shown,
            json!([
                {"id":"anthropic","sources":["auth"],"removable":true},
                {"id":"commented","name":"Lab, \"quoted\" // not a comment","sources":["config"],"configPath":jsonc,"removable":false},
                {"id":"github-copilot","sources":["auth"],"removable":true},
                {"id":"mistral","sources":["catalog"],"removable":false},
                {"id":"openrouter","sources":["auth"],"removable":true},
                {"id":"team_router","name":"Team router","baseUrl":"https://llm.example.edu/v1","npm":"@ai-sdk/openai-compatible","sources":["config"],"configPath":json,"removable":true},
                {"id":"thinkingmachines","env":["TINKER_API_KEY"],"sources":["env"],"removable":false},
                {"id":"tinker","name":"Tinker","baseUrl":"https://tinker.thinkingmachines.dev/services/tinker-prod/oai/api/v1","npm":"@ai-sdk/openai-compatible","env":["TINKER_API_KEY"],"sources":["config"],"configPath":json,"removable":true},
                {"id":"xai","env":["XAI_API_KEY"],"sources":["env"],"removable":false}
            ])
        );

        let refused = |id: &str| delete_found_provider(id).unwrap_err().to_string();
        assert_eq!(
            refused("xai"),
            "`XAI_API_KEY` in your environment provides xai. Remove it there, or turn this provider off."
        );
        assert_eq!(
            refused("commented"),
            format!("OpenResearch can't edit {} because it has comments. Remove the `commented` provider there, then re-check OpenCode.", jsonc.display())
        );
        let gone = delete_found_provider("mistral").unwrap_err();
        assert!(gone.downcast_ref::<FoundProviderGone>().is_some(), "{gone}");
        std::env::set_var(
            "OPENCODE_CONFIG_CONTENT",
            r#"{"provider":{"inline":{},"github-copilot":{}}}"#,
        );
        assert_eq!(
            refused("inline"),
            "`OPENCODE_CONFIG_CONTENT` in your environment sets up inline. Remove it there, or turn this provider off."
        );
        assert!(refused("github-copilot").contains("OPENCODE_CONFIG_CONTENT"));
        assert!(std::fs::read_to_string(&auth)
            .unwrap()
            .contains("github-copilot"));
        std::env::remove_var("OPENCODE_CONFIG_CONTENT");

        delete_found_provider("tinker").unwrap();
        let config = std::fs::read_to_string(&json).unwrap();
        let keys: Vec<_> = serde_json::from_str::<serde_json::Map<String, Value>>(&config)
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            keys,
            [
                "$schema",
                "model",
                "provider",
                "providers",
                "disabled_providers"
            ]
        );
        assert!(!config.contains("tinker.thinkingmachines.dev"));
        assert!(config.contains("switched-off"));

        let label: crate::local::agent_profiles::Edit =
            serde_json::from_value(json!({"logo": "openrouter"})).unwrap();
        crate::local::agent_profiles::edit("opencode:team_router", &label).unwrap();
        delete_found_provider("team_router").unwrap();
        assert!(!std::fs::read_to_string(&json)
            .unwrap()
            .contains("llm.example.edu"));

        delete_found_provider("openrouter").unwrap();
        let signed_in: Value =
            serde_json::from_str(&std::fs::read_to_string(&auth).unwrap()).unwrap();
        assert_eq!(
            signed_in.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["github-copilot", "anthropic", "openai"]
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&auth).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let broken = dir.join("broken.json");
        std::fs::write(&broken, "{ not json").unwrap();
        std::env::set_var("OPENCODE_CONFIG", &broken);
        assert_eq!(
            refused("anthropic"),
            format!(
                "{} is not a valid OpenCode config. Fix it, then try again.",
                broken.display()
            )
        );
        assert!(std::fs::read_to_string(&auth)
            .unwrap()
            .contains("anthropic"));
        std::env::remove_var("OPENCODE_CONFIG");

        let ids: Vec<_> = found_providers([])
            .into_iter()
            .map(|found| found.id)
            .collect();
        assert_eq!(
            ids,
            [
                "anthropic",
                "commented",
                "github-copilot",
                "thinkingmachines",
                "xai"
            ]
        );
    }

    #[test]
    fn variant_is_sent_unless_it_is_the_default_sentinel() {
        assert_eq!(opencode_variant(Some("high")), Some("high"));
        assert_eq!(opencode_variant(Some("minimal")), Some("minimal"));
        assert_eq!(opencode_variant(Some("none")), Some("none"));
        assert_eq!(opencode_variant(Some("brand-new")), Some("brand-new"));
        assert_eq!(opencode_variant(Some(REASONING_DEFAULT_ID)), None);
        assert_eq!(opencode_variant(None), None);
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
            (false, Some(PermissionMode::Bypass), "build", true),
            (true, Some(PermissionMode::Ask), "plan", false),
            (true, Some(PermissionMode::Bypass), "plan", true),
        ] {
            assert_eq!(opencode_agent(plan_mode), agent);
            assert_eq!(opencode_auto_approve(permission_mode), auto_approve);
        }
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
            from: None,
            env: Vec::new(),
        };
        assert_eq!(plan_exit_transition(&prompt, &response("Yes")), Some(false));
        assert_eq!(plan_exit_transition(&prompt, &response("No")), Some(true));
        prompt.plan_exit = false;
        assert_eq!(plan_exit_transition(&prompt, &response("Yes")), None);
    }

    #[test]
    fn native_step_tokens_restore_inclusive_counters() {
        let fixture: Value =
            serde_json::from_str(include_str!("fixtures/opencode-mock-usage.json")).unwrap();
        let usage = opencode_native_usage(&fixture[0]["tokens"]);
        usage.validate().unwrap();
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.output_tokens, Some(20));
        assert_eq!(usage.reasoning_tokens, Some(5));
        assert_eq!(usage.total(), Some(120));
    }

    #[test]
    fn step_and_message_costs_are_dollars_and_absence_is_unreported() {
        let step = json!({"type": "step-finish", "cost": 0.0123, "tokens": {"input": 1}});
        let cost = opencode_cost(&step).unwrap();
        assert_eq!((cost.micros, cost.currency.as_str()), (12_300, "USD"));
        // A free model reports zero, which is a price; no field is no price.
        assert_eq!(opencode_cost(&json!({"cost": 0})).unwrap().micros, 0);
        assert!(opencode_cost(&json!({"tokens": {}})).is_none());
        assert!(opencode_cost(&json!({"cost": -1})).is_none());
    }

    /// A `#!/bin/sh` stand-in for the opencode CLI, as in `detect.rs`.
    #[cfg(unix)]
    fn sh_script(name: &str, body: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("orx-opencode-capture-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let body = body.replacen(
            "#!/bin/sh\n",
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 2.0.1; exit 0; fi\n",
            1,
        );
        orx_test_support::write_executable(&path, body, 0o755).unwrap();
        path
    }
}
