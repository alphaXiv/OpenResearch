//! ZCode harness (Z.ai's coding agent).
//!
//! Chat: one `zcode -p <prompt> --output-format stream-json` child per turn,
//! continued with `--resume <sessionId>`. The stream is one JSON event per
//! line: `model.streaming` (text, reasoning and tool-call deltas),
//! `tool.updated` (tool results), `permission.resolved`, and a closing
//! `turn.completed` or `turn.failed`.
//!
//! Print mode has no approval channel: in `build`/`edit` mode ZCode denies
//! high-risk tools itself ("No permission client configured"). So the
//! composer offers `edit` (edits allowed, risky commands denied; the default)
//! and `yolo` (everything allowed). Plan runs in `build`, where every tool
//! with side effects needs approval and so is denied: `Write`, `Edit`, every
//! `Bash` command, and the Node REPL. The planning instruction only asks for
//! a plan; the mode is what keeps the turn read-only.
//!
//! Detection: a `zcode` CLI (official `~/.zcode/runtime` install, npm, PATH),
//! else the runtime bundled with the ZCode desktop app (`resources/glm/zcode.cjs`)
//! run through the app's own Electron binary as Node (`ELECTRON_RUN_AS_NODE=1`).
//! The desktop login in `~/.zcode/v2` is shared with both.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::detect::{HarnessAuthState, HarnessInfo, ModelInfo};
use super::options::{HarnessOptions, OptionChoice, PermissionMode, PlanActivation};
use super::{Harness, ResumeAction, TurnFailure, TurnOutcome, TurnResult, TURN_WATCHDOG};
use crate::error::{anyhow, Result};
use crate::local::chat::{
    find_part_mut, harness_log, prepare_env, set_chat_session_env, DeliveryState, PromptAnswer,
    ResumeCtx, TurnCtx, WirePart, WirePrompt, WireToolState,
};
use crate::local::opencode::ensure_playbook;
use crate::local::shell_env::{find_in_dir, find_on_path};

const KEY: &str = "zcode";
const INSTALL_HINT: &str =
    "Install the ZCode desktop app from https://zcode.z.ai and sign in there, then re-check this harness.";
const LOGIN_HINT: &str =
    "Sign in for command-line use with `zcode login` (Z.ai Individual or Team Coding Plan), or enable a provider with an API key in the ZCode app, then re-check this harness. Start Plan works only inside the ZCode app.";
const OTHER_ACCOUNT_HINT: &str =
    "ZCode's command-line sign-in belongs to a different Z.ai account than the plan key ZCode saved last, so its runtime finds no model. Sign in again with `zcode login` using the account that has the plan, then re-check this harness.";
const VERSION_TIMEOUT: Duration = Duration::from_secs(20);

pub struct ZCode;

/// How to start the ZCode runtime.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Launch {
    pub program: PathBuf,
    /// Arguments before ZCode's own (the bundled script path).
    pub prefix: Vec<OsString>,
    pub env: Vec<(&'static str, OsString)>,
    pub bundled: bool,
}

impl Launch {
    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.prefix);
        for (key, value) in &self.env {
            cmd.env(key, value);
        }
        cmd
    }
}

/// The desktop app's resources directory for this platform, if installed.
fn desktop_candidates() -> Vec<(PathBuf, PathBuf)> {
    let mut found = Vec::new();
    if cfg!(windows) {
        if let Some(local) = dirs::data_local_dir() {
            let dir = local.join("Programs").join("ZCode");
            found.push((dir.join("ZCode.exe"), dir.join("resources")));
        }
    } else if cfg!(target_os = "macos") {
        let apps = [PathBuf::from("/Applications")]
            .into_iter()
            .chain(dirs::home_dir().map(|home| home.join("Applications")));
        for apps in apps {
            let contents = apps.join("ZCode.app").join("Contents");
            found.push((
                contents.join("MacOS").join("ZCode"),
                contents.join("Resources"),
            ));
        }
    } else {
        for dir in [PathBuf::from("/opt/ZCode"), PathBuf::from("/usr/lib/zcode")] {
            found.push((dir.join("zcode"), dir.join("resources")));
        }
    }
    found
}

/// The runtime bundled with the desktop app, launched as Node.
fn bundled_launch_in(exe: &Path, resources: &Path) -> Option<Launch> {
    let script = resources.join("glm").join("zcode.cjs");
    if !exe.is_file() || !script.is_file() {
        return None;
    }
    let mut env = vec![("ELECTRON_RUN_AS_NODE", OsString::from("1"))];
    let builtin = resources
        .join("config")
        .join("provider")
        .join("zcode-builtin.json");
    if builtin.is_file() {
        env.push((
            "ZCODE_BUILTIN_PROVIDER_CONFIG_FILE",
            builtin.into_os_string(),
        ));
    }
    Some(Launch {
        program: exe.to_path_buf(),
        prefix: vec![script.into_os_string()],
        env,
        bundled: true,
    })
}

/// A standalone `zcode` CLI — never the desktop app, which Windows would
/// otherwise match for `zcode` because its file system ignores case.
fn cli_candidates() -> Vec<PathBuf> {
    let home_bins = dirs::home_dir().into_iter().flat_map(|home| {
        [
            home.join(".local").join("bin"),
            home.join(".zcode").join("runtime").join("bin"),
            home.join(".zcode").join("runtime"),
        ]
    });
    let found = find_on_path("zcode")
        .into_iter()
        .chain(home_bins.filter_map(|dir| find_in_dir(&dir, "zcode")))
        .filter(|path| !super::acp::is_desktop_app(path))
        .map(super::detect::resolve_symlinks)
        .collect();
    super::detect::unique(found)
}

pub(crate) fn find_launch() -> Option<Launch> {
    if let Some(program) = cli_candidates().into_iter().next() {
        return Some(Launch {
            program,
            prefix: Vec::new(),
            env: Vec::new(),
            bundled: false,
        });
    }
    desktop_candidates()
        .into_iter()
        .find_map(|(exe, resources)| bundled_launch_in(&exe, &resources))
}

fn zcode_home() -> Option<PathBuf> {
    crate::local::shell_env::var("ZCODE_DATA_BASE_DIR")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .map(|base| base.join(".zcode"))
}

/// Whether ZCode's command-line runtime can reach a model.
#[derive(Debug, PartialEq)]
enum Access {
    /// A plan signed in with `zcode login`, or an enabled API-key provider.
    Ready,
    /// The plan's identity and its API key belong to different accounts
    /// (a later sign-in in the app replaced the key), so no model is usable.
    OtherAccount,
    None,
}

/// What `zcode login` leaves for the runtime: `account-provider:<id>:identity`
/// and the plan's API key, `account-provider:coding-plan:<id>:account:<identity>:api-key`.
/// The runtime uses a plan only when the key is the identity's own. Returns
/// the plans it can use, and whether some plan's key is another account's.
fn signed_in_plans(
    store: &serde_json::Map<String, Value>,
    cipher_key: Option<&[u8; 32]>,
) -> (Vec<String>, bool) {
    let mut plans = Vec::new();
    let mut other_account = false;
    for (name, value) in store {
        let Some(provider) = name
            .strip_prefix("account-provider:")
            .and_then(|rest| rest.strip_suffix(":identity"))
        else {
            continue;
        };
        let prefix = format!("account-provider:coding-plan:{provider}:account:");
        let accounts: Vec<&str> = store
            .keys()
            .filter_map(|key| key.strip_prefix(&prefix)?.strip_suffix(":api-key"))
            .collect();
        if accounts.is_empty() {
            continue;
        }
        let identity = cipher_key
            .zip(value.as_str())
            .and_then(|(cipher_key, value)| decrypt_credential(value, cipher_key));
        // Unreadable (a custom ZCODE_CREDENTIAL_SECRET): both keys are there.
        let own = identity.is_none_or(|identity| {
            let encoded = urlencoding::encode(&identity);
            accounts
                .iter()
                .any(|account| *account == encoded || *account == identity)
        });
        if own {
            plans.push(provider.to_string());
        } else {
            other_account = true;
        }
    }
    (plans, other_account)
}

fn plan_access(store: &serde_json::Map<String, Value>, cipher_key: Option<&[u8; 32]>) -> Access {
    match signed_in_plans(store, cipher_key) {
        (plans, _) if !plans.is_empty() => Access::Ready,
        (_, true) => Access::OtherAccount,
        _ => Access::None,
    }
}

/// Enabled providers in provider_config.json that carry an API key.
fn keyed_providers(config: &Value) -> Vec<&Value> {
    config
        .pointer("/config/providerConfigRules/providerRules")
        .and_then(Value::as_array)
        .map(|rules| {
            rules
                .iter()
                .filter(|rule| {
                    rule.get("enabled").and_then(Value::as_bool) != Some(false)
                        && rule
                            .pointer("/config/access/apiKey")
                            .and_then(Value::as_str)
                            .is_some_and(|key| !key.trim().is_empty())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn access(home: &Path) -> Access {
    let v2 = home.join("v2");
    let plan = super::detect::read_json(v2.join("credentials.json"))
        .and_then(|store| {
            store
                .as_object()
                .map(|store| plan_access(store, Some(&credential_cipher_key())))
        })
        .unwrap_or(Access::None);
    if plan == Access::Ready {
        return plan;
    }
    let keyed_provider = super::detect::read_json(v2.join("provider_config.json"))
        .is_some_and(|config| !keyed_providers(&config).is_empty());
    if keyed_provider {
        Access::Ready
    } else {
        plan
    }
}

/// The runtime's built-in provider catalog: the app's file, handed to the
/// bundled runtime, or the one an npm install ships next to its launcher.
fn builtin_catalog(launch: &Launch) -> Option<PathBuf> {
    if let Some((_, path)) = launch
        .env
        .iter()
        .find(|(key, _)| *key == "ZCODE_BUILTIN_PROVIDER_CONFIG_FILE")
    {
        return Some(PathBuf::from(path));
    }
    let package = launch.program.parent()?.parent()?;
    Some(
        package
            .join("vendor")
            .join("provider")
            .join("zcode-builtin.json"),
    )
    .filter(|path| path.is_file())
}

/// The models ZCode's runtime can use, from the same places it reads them: the
/// built-in models of each plan `zcode login` signed in (from the app's
/// built-in provider catalog), and the models of each enabled API-key provider
/// (its own list, else its template's). Ids are `<provider>/<model>`, the form
/// of ZCode's own model selection.
fn catalog(home: &Path, builtin: Option<&Path>) -> Vec<ModelInfo> {
    let v2 = home.join("v2");
    let builtin = builtin.and_then(|path| super::detect::read_json(path.to_path_buf()));
    let rules = |kind: &str| {
        builtin
            .as_ref()
            .and_then(|config| config.pointer(&format!("/config/providerConfigRules/{kind}")))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let (providers, templates) = (rules("providerRules"), rules("templateRules"));
    let models_of = |rule: &Value, key: &str| -> Vec<String> {
        rule.pointer(&format!("/config/{key}"))
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut entries: Vec<(String, String, String)> = Vec::new();
    let plans = super::detect::read_json(v2.join("credentials.json"))
        .and_then(|store| {
            store
                .as_object()
                .map(|store| signed_in_plans(store, Some(&credential_cipher_key())).0)
        })
        .unwrap_or_default();
    for plan in plans {
        if let Some(rule) = providers
            .iter()
            .find(|rule| rule.get("providerId").and_then(Value::as_str) == Some(&plan))
        {
            let name = rule
                .get("providerName")
                .and_then(Value::as_str)
                .unwrap_or(&plan)
                .to_string();
            for model in models_of(rule, "builtinModelIds") {
                entries.push((plan.clone(), model, name.clone()));
            }
        }
    }
    if let Some(config) = super::detect::read_json(v2.join("provider_config.json")) {
        for rule in keyed_providers(&config) {
            let Some(provider) = rule.get("providerId").and_then(Value::as_str) else {
                continue;
            };
            let name = rule
                .get("providerName")
                .and_then(Value::as_str)
                .unwrap_or(provider)
                .to_string();
            let mut models = models_of(rule, "personalModelIds");
            if models.is_empty() {
                let template = rule
                    .get("templateId")
                    .and_then(Value::as_str)
                    .unwrap_or(provider);
                if let Some(template) = templates
                    .iter()
                    .find(|rule| rule.get("templateId").and_then(Value::as_str) == Some(template))
                {
                    models = models_of(template, "builtinModelIds");
                }
            }
            for model in models {
                entries.push((provider.to_string(), model, name.clone()));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    entries
        .into_iter()
        .filter(|(provider, model, _)| seen.insert(format!("{provider}/{model}")))
        .map(|(provider, model, name)| {
            ModelInfo::new(format!("{provider}/{model}")).with_label(Some(&model), Some(&name))
        })
        .collect()
}

/// Held by a new chat from writing ZCode's default model until its runtime has
/// read it, so chats started together do not run on each other's model.
static MODEL_SELECTION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Whether `event` shows the runtime has settled on its model (or ended), so
/// the shared default model may change again.
fn model_settled(event: &Value) -> bool {
    matches!(
        event.get("type").and_then(Value::as_str),
        Some("turn.completed" | "turn.failed")
    ) || event.pointer("/payload/modelId").is_some()
}

/// Make `provider/model` ZCode's default model, which is the only model
/// choice its print mode reads. The default is shared with the ZCode app.
fn select_model(home: &Path, model: &str) -> Result<()> {
    let Some((provider, model)) = model.split_once('/') else {
        return Err(anyhow!("Unknown ZCode model {model}"));
    };
    let path = home.join("v2").join("provider_config.json");
    let mut config = super::detect::read_json(path.clone()).unwrap_or_else(|| {
        serde_json::json!({
            "schemaVersion": 1,
            "config": {
                "providerConfigRules": {"providerRules": []},
                "modelConfigRules": {"providerModelRules": [], "manualProviderModelRules": []},
            },
        })
    });
    let selection = serde_json::json!({"providerId": provider, "modelId": model});
    let Some(settings) = config.get_mut("config").and_then(Value::as_object_mut) else {
        return Err(anyhow!("{} has no config object", path.display()));
    };
    if settings.get("defaultModelSelection") == Some(&selection) {
        return Ok(());
    }
    settings.insert("defaultModelSelection".into(), selection);
    let tmp = path.with_extension(format!("json.orx-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&tmp, serde_json::to_vec_pretty(&config)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// The key ZCode encrypts credentials.json with: SHA-256 of
/// `ZCODE_CREDENTIAL_SECRET`, else of a fallback built from the platform,
/// home directory and user name, as Node reports them.
fn credential_cipher_key() -> [u8; 32] {
    use sha2::Digest;
    let seed = crate::local::shell_env::var("ZCODE_CREDENTIAL_SECRET")
        .map(|secret| secret.to_string_lossy().trim().to_string())
        .filter(|secret| !secret.is_empty())
        .unwrap_or_else(|| {
            let platform = if cfg!(windows) {
                "win32"
            } else if cfg!(target_os = "macos") {
                "darwin"
            } else {
                "linux"
            };
            let home = dirs::home_dir()
                .map(|home| home.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!(
                "zcode-credential-fallback:{platform}:{home}:{}",
                user_name()
            )
        });
    sha2::Sha256::digest(seed.as_bytes()).into()
}

fn user_name() -> String {
    let from_env = if cfg!(windows) {
        std::env::var("USERNAME").ok()
    } else {
        std::env::var("USER")
            .ok()
            .or_else(|| std::env::var("LOGNAME").ok())
    };
    from_env
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

/// A credentials.json value: `enc:v1:<iv>.<tag>.<data>` (base64url,
/// AES-256-GCM), or plain text.
fn decrypt_credential(value: &str, cipher_key: &[u8; 32]) -> Option<String> {
    use aes_gcm::aead::{Aead, KeyInit};
    use base64::Engine;
    let Some(sealed) = value.strip_prefix("enc:v1:") else {
        return Some(value.to_string());
    };
    let decode = |part: &str| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(part.trim_end_matches('='))
            .ok()
    };
    let mut parts = sealed.split('.');
    let (iv, tag, data) = (
        decode(parts.next()?)?,
        decode(parts.next()?)?,
        decode(parts.next()?)?,
    );
    if parts.next().is_some() || iv.len() != 12 || tag.len() != 16 {
        return None;
    }
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(cipher_key).ok()?;
    let mut sealed = data;
    sealed.extend_from_slice(&tag);
    let plain = cipher
        .decrypt(aes_gcm::Nonce::from_slice(&iv), sealed.as_ref())
        .ok()?;
    String::from_utf8(plain).ok()
}

async fn version(launch: &Launch) -> Option<String> {
    let mut cmd = launch.command();
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let out = super::detect::detect_spawn_output_timed(cmd, VERSION_TIMEOUT)
        .await?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

impl ZCode {
    async fn detect_at(&self, snapshot: bool) -> Option<HarnessInfo> {
        let mut info = HarnessInfo::new(self.id(), self.name());
        if let Some(launch) = find_launch() {
            info.installed = true;
            info.bin_path = Some(launch.program.to_string_lossy().into_owned());
            if !snapshot {
                match version(&launch).await {
                    Some(version) => info.version = Some(version),
                    None if launch.bundled => {
                        info.install_broken = true;
                        info.agent_note = Some(
                            "The ZCode desktop app's runtime did not start from the command line. Update ZCode, or install the ZCode CLI, then re-check this harness."
                                .into(),
                        );
                    }
                    None => {}
                }
            }
            if !info.install_broken {
                match zcode_home().map_or(Access::None, |home| access(&home)) {
                    Access::Ready => {
                        info.authenticated = true;
                        info.auth_state = HarnessAuthState::Ready;
                        info.auth_method = Some("oauth");
                    }
                    Access::OtherAccount => {
                        info.auth_state = HarnessAuthState::NeedsLogin;
                        info.agent_note = Some(OTHER_ACCOUNT_HINT.into());
                    }
                    Access::None => {
                        info.auth_state = HarnessAuthState::NeedsLogin;
                        info.agent_note = Some(LOGIN_HINT.into());
                    }
                }
            }
        } else {
            info.agent_note = Some(INSTALL_HINT.into());
        }
        if let (Some(home), Some(launch)) = (zcode_home(), find_launch()) {
            let builtin = builtin_catalog(&launch);
            info = info.with_models(catalog(&home, builtin.as_deref()));
        }
        info.agent_ready = info.ready();
        Some(info)
    }
}

#[async_trait]
impl Harness for ZCode {
    fn id(&self) -> &'static str {
        KEY
    }

    fn name(&self) -> &'static str {
        "ZCode"
    }

    fn supports_chat(&self) -> bool {
        true
    }

    async fn detect(&self) -> Option<HarnessInfo> {
        self.detect_at(false).await
    }

    async fn detect_snapshot(&self) -> Option<HarnessInfo> {
        self.detect_at(true).await
    }

    async fn run_turn(&self, ctx: &mut TurnCtx) -> TurnResult {
        run_turn(ctx)
            .await
            .map(|()| TurnOutcome::Completed)
            .map_err(|error| TurnFailure::adapter(error, ctx.delivery_state()))
    }

    fn options(&self) -> HarnessOptions {
        HarnessOptions::none().with_permission_choices(
            vec![
                OptionChoice::described(
                    "accept-edits",
                    "Edit",
                    "Allow file edits; ZCode denies high-risk commands",
                ),
                OptionChoice::described("bypass", "YOLO", "Allow every tool"),
            ],
            "accept-edits",
            PlanActivation::Command,
        )
    }

    async fn resume_from_prompt(
        &self,
        ctx: &ResumeCtx,
        prompt: &WirePrompt,
        answer: &PromptAnswer,
    ) -> Result<ResumeAction> {
        super::acp::resume_from_prompt(ctx, prompt, answer).await
    }

    fn config_home(&self) -> Option<PathBuf> {
        zcode_home()
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
        Some(super::CLAUDE_SKILL)
    }

    fn session_skills_dir(&self) -> Option<&'static str> {
        Some(".agents/skills")
    }
}

fn zcode_mode(mode: Option<PermissionMode>, plan: bool) -> &'static str {
    if plan || mode == Some(PermissionMode::Plan) {
        return "build";
    }
    match mode.unwrap_or(PermissionMode::AcceptEdits) {
        PermissionMode::Bypass => "yolo",
        PermissionMode::Ask => "build",
        PermissionMode::AcceptEdits | PermissionMode::Auto | PermissionMode::Plan => "edit",
    }
}

const PLAN_NOTE: &str = "Plan mode: investigate read-only and reply with a concrete plan. Do not modify files or run commands that change state.";

fn turn_prompt(text: &str, first: bool, plan: bool) -> String {
    let mut prompt = String::new();
    if first {
        prompt.push_str(&super::acp::playbook_pointer());
        prompt.push_str("\n\n");
    }
    if plan {
        prompt.push_str(PLAN_NOTE);
        prompt.push_str("\n\n");
    }
    prompt.push_str(text);
    prompt
}

async fn run_turn(ctx: &mut TurnCtx) -> Result<()> {
    let launch = find_launch().ok_or_else(|| anyhow!("ZCode not found. {INSTALL_HINT}"))?;
    let project = ctx.project.clone();
    let session_id = ctx.session_id.clone();
    let (repo, _playbook) = tokio::task::spawn_blocking(move || {
        ensure_playbook(&project, &session_id, Some(".agents/skills"))
    })
    .await
    .map_err(|e| anyhow!("playbook task failed: {e}"))??;

    let resume = ctx.native_session_id.clone();
    // A new session starts on ZCode's default model; a resumed one keeps its own.
    let mut selection = match resume {
        None => Some(MODEL_SELECTION.lock().await),
        Some(_) => None,
    };
    if let (None, Some(model), Some(home)) = (&resume, ctx.model.as_deref(), zcode_home()) {
        select_model(&home, model)
            .map_err(|error| anyhow!("Could not select {model} in ZCode: {error}"))?;
    }
    let plan = ctx.plan_mode || ctx.permission_mode == Some(PermissionMode::Plan);
    let prompt = turn_prompt(&ctx.text, resume.is_none(), plan);

    let log_name = format!("zcode-{}", uuid::Uuid::new_v4());
    let mut cmd = launch.command();
    cmd.arg("-p")
        .arg(&prompt)
        .args(["--output-format", "stream-json", "--no-color", "--mode"])
        .arg(zcode_mode(ctx.permission_mode, ctx.plan_mode))
        .arg("--cwd")
        .arg(&repo);
    if let Some(native_id) = &resume {
        cmd.args(["--resume", native_id]);
    }
    cmd.current_dir(&repo)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(harness_log(&log_name)?))
        .kill_on_drop(true);
    prepare_env(&mut cmd);
    // prepare_env may reset the environment; the launch's own variables win.
    for (key, value) in &launch.env {
        cmd.env(key, value);
    }
    cmd.env("NO_COLOR", "1");
    set_chat_session_env(&mut cmd, &ctx.session_id, KEY, ctx.host.up_port());

    ctx.persist_delivery(DeliveryState::Unknown)?;
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            ctx.mark_delivery(DeliveryState::NotSent);
            return Err(anyhow!(
                "Could not spawn {}: {}",
                launch.program.display(),
                error
            ));
        }
    };
    let _processes = super::antigravity::TurnProcesses(child.id());
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let mut lines = BufReader::new(stdout).lines();
    let mut state = TurnState::default();

    loop {
        match tokio::time::timeout(TURN_WATCHDOG, lines.next_line()).await {
            Ok(Ok(Some(line))) => {
                let Ok(event) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                ctx.mark_delivery(DeliveryState::Accepted);
                if selection.is_some() && model_settled(&event) {
                    selection = None;
                }
                apply_event(ctx, &mut state, &event);
                if let Some(sid) = state.session_id.as_deref() {
                    ctx.set_native_session_id(sid);
                }
                ctx.maybe_flush();
            }
            Ok(Ok(None)) => break,
            Ok(Err(error)) => return Err(anyhow!("zcode stdout: {error}")),
            Err(_) => {
                return Err(anyhow!(
                    "ZCode went silent for {} minutes and was interrupted.",
                    TURN_WATCHDOG.as_secs() / 60
                ))
            }
        }
    }
    let status = tokio::time::timeout(Duration::from_secs(30), child.wait())
        .await
        .map_err(|_| anyhow!("ZCode did not exit after its response"))??;
    let log_path = crate::store::data_dir().join(format!("agent-{log_name}.log"));
    if let Some(message) = state.failure.take() {
        ctx.mark_terminal_failure("zcode_turn_failed", explain_failure(message, &log_path));
    } else if !state.completed {
        let detail = std::fs::read_to_string(&log_path)
            .ok()
            .and_then(|log| {
                log.lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        return Err(anyhow!(
            "ZCode ended without a result ({status}). {detail} (log: {})",
            log_path.display()
        ));
    } else {
        let _ = std::fs::remove_file(&log_path);
    }
    if plan {
        if let Some(card) =
            super::acp::synthesized_plan_card(&ctx.assistant.parts, &ctx.assistant.id)
        {
            ctx.upsert_part(card);
        }
    }
    let _ = ctx.flush();
    Ok(())
}

/// ZCode's own wording for a turn that found no model says nothing about why.
const NO_MODEL: &str = "Select a model before continuing";

/// What a turn outside the ZCode app can use, for failures that stem from
/// the account rather than the chat.
const COMMAND_LINE_ACCESS: &str = "Outside its app ZCode reaches models through its own \
     sign-in (`zcode login`, for a Z.ai Individual or Team Coding Plan) or a provider with an \
     API key, such as Z.ai API billed from your balance. Start Plan works only inside the \
     ZCode app.";

fn explain_failure(message: String, log_path: &Path) -> String {
    if message.contains(NO_MODEL) {
        return format!(
            "ZCode found no model it can use from the command line ({NO_MODEL}). \
             {COMMAND_LINE_ACCESS} If you signed in to the app with another account since \
             `zcode login`, run `zcode login` again. Log: {}",
            log_path.display()
        );
    }
    // Z.ai's "Insufficient balance or no resource package": the chosen
    // provider has no plan or balance behind it for this model.
    if message.contains("[1113]") {
        return format!("{message}\n\nZ.ai has no plan quota or balance for this request. {COMMAND_LINE_ACCESS}");
    }
    message
}

#[derive(Default)]
struct TurnState {
    session_id: Option<String>,
    text_part: Option<(String, String)>,
    reasoning_part: Option<(String, String)>,
    completed: bool,
    failure: Option<String>,
}

fn apply_event(ctx: &mut TurnCtx, state: &mut TurnState, event: &Value) {
    if let Some(sid) = event.get("sessionId").and_then(Value::as_str) {
        if state.session_id.as_deref() != Some(sid) {
            state.session_id = Some(sid.to_string());
        }
    }
    let payload = event.get("payload").unwrap_or(&Value::Null);
    match event.get("type").and_then(Value::as_str) {
        Some("model.streaming") => apply_streaming(ctx, state, payload),
        Some("tool.updated") => apply_tool(ctx, payload),
        Some("permission.resolved") => {
            if payload.get("decision").and_then(Value::as_str) == Some("deny") {
                if let Some(call_id) = payload.get("toolCallId").and_then(Value::as_str) {
                    let reason = payload
                        .get("reason")
                        .and_then(Value::as_str)
                        .unwrap_or("denied")
                        .to_string();
                    if let Some(part_state) = find_part_mut(&mut ctx.assistant.parts, call_id)
                        .and_then(|part| part.state.as_mut())
                    {
                        part_state.status = "error".into();
                        part_state.error = Some(format!("ZCode denied this tool: {reason}"));
                    }
                }
            }
        }
        Some("session.titleUpdated") => {
            let pointer = super::acp::playbook_pointer();
            if let Some(title) = payload
                .get("title")
                .and_then(Value::as_str)
                .and_then(|title| super::acp::agent_title(title, &[&pointer, PLAN_NOTE]))
            {
                ctx.set_title(title);
            }
        }
        Some("turn.completed") => state.completed = true,
        Some("turn.failed") => {
            let message = payload
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("ZCode reported a failed turn");
            state.failure = Some(message.to_string());
        }
        _ => {}
    }
}

fn apply_streaming(ctx: &mut TurnCtx, state: &mut TurnState, payload: &Value) {
    let message = payload
        .get("assistantMessageId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let delta = payload.get("delta").and_then(Value::as_str).unwrap_or("");
    match payload.get("kind").and_then(Value::as_str) {
        Some("text_delta") if !delta.is_empty() => {
            state.reasoning_part = None;
            let id = match &state.text_part {
                Some((owner, id)) if *owner == message => id.clone(),
                _ => {
                    let id = format!("text-{message}-{}", ctx.assistant.parts.len());
                    ctx.upsert_part(WirePart::text(id.clone(), ""));
                    state.text_part = Some((message.clone(), id.clone()));
                    id
                }
            };
            ctx.append_part_text(&id, delta);
        }
        Some("reasoning_delta" | "thinking_delta") if !delta.is_empty() => {
            state.text_part = None;
            let id = match &state.reasoning_part {
                Some((owner, id)) if *owner == message => id.clone(),
                _ => {
                    let id = format!("reasoning-{message}-{}", ctx.assistant.parts.len());
                    ctx.upsert_part(WirePart::reasoning(id.clone(), ""));
                    state.reasoning_part = Some((message.clone(), id.clone()));
                    id
                }
            };
            ctx.append_part_text(&id, delta);
        }
        Some("text_end") => state.text_part = None,
        Some("reasoning_end") => state.reasoning_part = None,
        Some("tool_call") => {
            state.text_part = None;
            state.reasoning_part = None;
            let Some(call_id) = payload.get("toolCallId").and_then(Value::as_str) else {
                return;
            };
            let name = payload
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or("Tool");
            ctx.upsert_part(WirePart {
                id: call_id.to_string(),
                kind: "tool".into(),
                text: None,
                tool: Some(name.to_string()),
                state: Some(WireToolState {
                    status: "running".into(),
                    input: payload.get("input").cloned(),
                    output: None,
                    error: None,
                    title: payload
                        .pointer("/input/description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                }),
                prompt: None,
                phase: None,
                children: Vec::new(),
            });
        }
        _ => {}
    }
}

fn apply_tool(ctx: &mut TurnCtx, payload: &Value) {
    if payload.get("kind").and_then(Value::as_str) != Some("result") {
        return;
    }
    let Some(call_id) = payload.get("toolCallId").and_then(Value::as_str) else {
        return;
    };
    let result = payload.get("result").unwrap_or(&Value::Null);
    let ok = result
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let content = match result.get("content").or_else(|| result.get("error")) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    };
    if find_part_mut(&mut ctx.assistant.parts, call_id).is_none() {
        ctx.upsert_part(WirePart::tool(call_id, "Tool", "running", None));
    }
    if let Some(part_state) =
        find_part_mut(&mut ctx.assistant.parts, call_id).and_then(|part| part.state.as_mut())
    {
        part_state.status = if ok { "completed" } else { "error" }.into();
        if ok {
            part_state.output = Some(content);
        } else {
            part_state.error = Some(content);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold(fixture: &str) -> (TurnCtx, TurnState) {
        let mut ctx = TurnCtx::test_stub();
        let mut state = TurnState::default();
        for line in fixture.lines() {
            let event: Value = serde_json::from_str(line).expect("fixture line is JSON");
            apply_event(&mut ctx, &mut state, &event);
        }
        (ctx, state)
    }

    #[test]
    fn folds_a_recorded_tool_turn() {
        let (ctx, state) = fold(include_str!("fixtures/zcode_stream_tool.jsonl"));
        assert!(state.completed);
        assert_eq!(
            state.session_id.as_deref(),
            Some("sess_475edcb4-9d96-4355-bba3-aca8b8960880")
        );
        let parts = &ctx.assistant.parts;
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].text.as_deref(), Some("Listing files."));
        let tool = &parts[1];
        assert_eq!(tool.tool.as_deref(), Some("Bash"));
        let tool_state = tool.state.as_ref().unwrap();
        assert_eq!(tool_state.status, "completed");
        assert_eq!(tool_state.output.as_deref(), Some("a.txt"));
        assert_eq!(
            tool_state.input,
            Some(serde_json::json!({"command": "ls", "description": "List files"}))
        );
        assert_eq!(parts[2].text.as_deref(), Some("Done: OK"));
    }

    #[test]
    fn a_resumed_turn_keeps_its_session() {
        let (ctx, state) = fold(include_str!("fixtures/zcode_stream_resume.jsonl"));
        assert!(state.completed);
        assert_eq!(
            state.session_id.as_deref(),
            Some("sess_475edcb4-9d96-4355-bba3-aca8b8960880")
        );
        assert_eq!(ctx.assistant.parts[0].text.as_deref(), Some("Done: OK"));
    }

    #[test]
    fn a_failed_turn_reports_the_provider_error() {
        let (_, state) = fold(include_str!("fixtures/zcode_stream_failed.jsonl"));
        assert!(!state.completed);
        assert!(state
            .failure
            .as_deref()
            .is_some_and(|message| message.contains("Insufficient balance")));
    }

    #[test]
    fn a_turn_without_a_model_says_what_zcode_needs() {
        let (_, state) = fold(include_str!("fixtures/zcode_stream_no_model.jsonl"));
        let message = explain_failure(state.failure.unwrap(), Path::new("agent-zcode-x.log"));
        assert!(
            message.contains("Select a model before continuing"),
            "{message}"
        );
        assert!(message.contains("`zcode login`"), "{message}");
        assert!(
            message.contains("Start Plan works only inside the ZCode app"),
            "{message}"
        );
        assert!(message.ends_with("Log: agent-zcode-x.log"), "{message}");
        let quota = explain_failure(
            "[1113][Insufficient balance or no resource package. Please recharge.][r1]".into(),
            Path::new("x"),
        );
        assert!(quota.starts_with("[1113][Insufficient balance"), "{quota}");
        assert!(
            quota.contains("Start Plan works only inside the ZCode app"),
            "{quota}"
        );
        let other = explain_failure("Model creation failed".into(), Path::new("x"));
        assert_eq!(other, "Model creation failed");
    }

    #[test]
    fn a_denied_tool_shows_the_reason() {
        let (ctx, state) = fold(include_str!("fixtures/zcode_stream_denied.jsonl"));
        assert!(state.completed);
        let tool = ctx
            .assistant
            .parts
            .iter()
            .find(|part| part.kind == "tool")
            .unwrap();
        let tool_state = tool.state.as_ref().unwrap();
        assert_eq!(tool_state.status, "error");
        assert!(tool_state
            .error
            .as_deref()
            .unwrap()
            .contains("No permission client configured"));
    }

    #[test]
    fn composer_modes_map_onto_zcode_modes() {
        // A chat with no mode chosen gets Edit, never YOLO.
        assert_eq!(zcode_mode(None, false), "edit");
        assert_eq!(
            ZCode.options().default_permission_mode,
            Some("accept-edits")
        );
        assert_eq!(zcode_mode(Some(PermissionMode::AcceptEdits), false), "edit");
        assert_eq!(zcode_mode(Some(PermissionMode::Bypass), false), "yolo");
        // Plan turns run in `build`, whatever mode the composer had.
        assert_eq!(zcode_mode(Some(PermissionMode::Bypass), true), "build");
        assert_eq!(zcode_mode(Some(PermissionMode::Plan), false), "build");
    }

    #[test]
    fn bundled_runtime_runs_the_desktop_binary_as_node() {
        let dir = std::env::temp_dir().join(format!("orx-zcode-{}", uuid::Uuid::new_v4()));
        let resources = dir.join("resources");
        std::fs::create_dir_all(resources.join("glm")).unwrap();
        std::fs::create_dir_all(resources.join("config").join("provider")).unwrap();
        let exe = dir.join("ZCode.exe");
        std::fs::write(&exe, "").unwrap();
        assert!(bundled_launch_in(&exe, &resources).is_none());
        std::fs::write(resources.join("glm").join("zcode.cjs"), "").unwrap();
        std::fs::write(
            resources
                .join("config")
                .join("provider")
                .join("zcode-builtin.json"),
            "{}",
        )
        .unwrap();
        let launch = bundled_launch_in(&exe, &resources).unwrap();
        assert!(launch.bundled);
        assert_eq!(
            launch.prefix,
            vec![resources.join("glm").join("zcode.cjs").into_os_string()]
        );
        assert!(launch
            .env
            .iter()
            .any(|(k, v)| *k == "ELECTRON_RUN_AS_NODE" && v == "1"));
        assert!(launch
            .env
            .iter()
            .any(|(k, _)| *k == "ZCODE_BUILTIN_PROVIDER_CONFIG_FILE"));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// `enc:v1:` value written by ZCode's cipher (Node's AES-256-GCM, base64url)
    /// for "1234567" under the key SHA-256("orx-test-seed").
    const SEALED_IDENTITY: &str = "enc:v1:BwcHBwcHBwcHBwcH.d-LIcdY2dLLGlwsdtoEuJQ.aNUKjW6rJg";

    fn test_key() -> [u8; 32] {
        use sha2::Digest;
        sha2::Sha256::digest(b"orx-test-seed").into()
    }

    #[test]
    fn credentials_decrypt_like_zcode() {
        assert_eq!(
            decrypt_credential(SEALED_IDENTITY, &test_key()).as_deref(),
            Some("1234567")
        );
        assert_eq!(
            decrypt_credential("plain", &test_key()).as_deref(),
            Some("plain")
        );
        assert_eq!(decrypt_credential(SEALED_IDENTITY, &[0; 32]), None);
        assert_eq!(decrypt_credential("enc:v1:a.b.c", &test_key()), None);
    }

    #[test]
    fn a_plan_key_from_another_account_is_not_access() {
        let store = |account: &str| {
            serde_json::json!({
                "account-provider:account:zai-individual-coding-plan:identity": SEALED_IDENTITY,
                format!("account-provider:coding-plan:account:zai-individual-coding-plan:account:{account}:api-key"): "enc:v1:x.y.z",
            })
            .as_object()
            .unwrap()
            .clone()
        };
        let key = test_key();
        assert_eq!(plan_access(&store("1234567"), Some(&key)), Access::Ready);
        assert_eq!(
            plan_access(&store("7654321"), Some(&key)),
            Access::OtherAccount
        );
        // Without a readable identity both keys being there is all orx can tell.
        assert_eq!(plan_access(&store("7654321"), None), Access::Ready);
        assert_eq!(
            plan_access(&serde_json::Map::new(), Some(&key)),
            Access::None
        );
    }

    #[test]
    fn the_catalog_lists_signed_in_plans_and_keyed_providers() {
        let dir = std::env::temp_dir().join(format!("orx-zcode-models-{}", uuid::Uuid::new_v4()));
        let v2 = dir.join("v2");
        std::fs::create_dir_all(&v2).unwrap();
        let builtin = dir.join("zcode-builtin.json");
        std::fs::write(
            &builtin,
            r#"{"config":{"providerConfigRules":{
                "providerRules":[
                    {"providerId":"account:zai-individual-coding-plan","providerName":"Z.AI Individual Coding Plan","config":{"builtinModelIds":["GLM-5.3","GLM-5.3-Flash"]}},
                    {"providerId":"account:zai-start-plan","providerName":"Start Plan","config":{"builtinModelIds":["GLM-5.2"]}}],
                "templateRules":[{"templateId":"zai-api","config":{"builtinModelIds":["GLM-5.3"]}}]}}}"#,
        )
        .unwrap();
        assert!(catalog(&dir, Some(&builtin)).is_empty());
        std::fs::write(
            v2.join("credentials.json"),
            r#"{"account-provider:account:zai-individual-coding-plan:identity":"enc:v1:a.b.c",
                "account-provider:coding-plan:account:zai-individual-coding-plan:account:u1:api-key":"enc:v1:a.b.c"}"#,
        )
        .unwrap();
        std::fs::write(
            v2.join("provider_config.json"),
            r#"{"config":{"providerConfigRules":{"providerRules":[
                {"providerId":"zai-api","providerName":"Z.ai Coding Plan","enabled":true,"config":{"access":{"apiKey":"id.secret"}}},
                {"providerId":"off","enabled":false,"config":{"access":{"apiKey":"k"},"personalModelIds":["x"]}}]}}}"#,
        )
        .unwrap();
        let models: Vec<_> = catalog(&dir, Some(&builtin))
            .into_iter()
            .map(|model| {
                (
                    model.id,
                    model.display_name.unwrap(),
                    model.description.unwrap(),
                )
            })
            .collect();
        assert_eq!(
            models,
            vec![
                (
                    "account:zai-individual-coding-plan/GLM-5.3".to_string(),
                    "GLM-5.3".to_string(),
                    "Z.AI Individual Coding Plan".to_string()
                ),
                (
                    "account:zai-individual-coding-plan/GLM-5.3-Flash".to_string(),
                    "GLM-5.3-Flash".to_string(),
                    "Z.AI Individual Coding Plan".to_string()
                ),
                (
                    "zai-api/GLM-5.3".to_string(),
                    "GLM-5.3".to_string(),
                    "Z.ai Coding Plan".to_string()
                ),
            ]
        );

        select_model(&dir, "account:zai-individual-coding-plan/GLM-5.3-Flash").unwrap();
        let config = super::super::detect::read_json(v2.join("provider_config.json")).unwrap();
        assert_eq!(
            config.pointer("/config/defaultModelSelection"),
            Some(&serde_json::json!({
                "providerId": "account:zai-individual-coding-plan",
                "modelId": "GLM-5.3-Flash"
            }))
        );
        // The rest of the file is kept.
        assert_eq!(
            config.pointer("/config/providerConfigRules/providerRules/0/providerId"),
            Some(&serde_json::json!("zai-api"))
        );
        assert!(select_model(&dir, "no-provider").is_err());
        // The write goes through a per-call temp file that does not stay behind.
        let names: Vec<_> = std::fs::read_dir(&v2)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("provider_config"))
            .collect();
        assert_eq!(names, vec!["provider_config.json"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_model_settles_when_the_runtime_reports_it_or_the_turn_ends() {
        let first_settled = |fixture: &str| {
            fixture
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .find(model_settled)
                .and_then(|event| {
                    event
                        .get("type")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
        };
        // Title and turn start come before the runtime has read its model.
        assert_eq!(
            first_settled(include_str!("fixtures/zcode_stream_tool.jsonl")).as_deref(),
            Some("session.updated")
        );
        assert_eq!(
            first_settled(include_str!("fixtures/zcode_stream_no_model.jsonl")).as_deref(),
            Some("turn.failed")
        );
    }

    #[test]
    fn access_comes_from_a_login_or_a_keyed_provider() {
        let dir = std::env::temp_dir().join(format!("orx-zcode-home-{}", uuid::Uuid::new_v4()));
        let v2 = dir.join("v2");
        std::fs::create_dir_all(&v2).unwrap();
        assert!(!(access(&dir) == Access::Ready));
        // The desktop app's sign-in alone: OAuth tokens and plan keys, but no
        // identity the command-line runtime can pair them with.
        let app_only = r#"{"oauth:zai:access_token":"enc:v1:a.b.c","zcodejwttoken":"enc:v1:a.b.c",
            "account-provider:coding-plan:account:zai-team-coding-plan:account:u1:api-key":"enc:v1:a.b.c"}"#;
        std::fs::write(v2.join("credentials.json"), app_only).unwrap();
        assert!(!(access(&dir) == Access::Ready));
        let disabled = r#"{"config":{"providerConfigRules":{"providerRules":[{"providerId":"zai-api","enabled":false,"config":{"access":{"type":"zhipu-coding-plan-api-key","apiKey":"id.secret"}}}]}}}"#;
        std::fs::write(v2.join("provider_config.json"), disabled).unwrap();
        assert!(!(access(&dir) == Access::Ready));
        std::fs::write(
            v2.join("provider_config.json"),
            r#"{"config":{"providerConfigRules":{"providerRules":[{"providerId":"c","config":{"access":{"type":"api-key","apiKey":"sk"}}}]}}}"#,
        )
        .unwrap();
        assert!((access(&dir) == Access::Ready));
        std::fs::remove_file(v2.join("provider_config.json")).unwrap();
        // An identity whose plan has no API key does not reach a model either.
        let identity_only =
            r#"{"account-provider:account:zai-individual-coding-plan:identity":"enc:v1:a.b.c"}"#;
        std::fs::write(v2.join("credentials.json"), identity_only).unwrap();
        assert!(!(access(&dir) == Access::Ready));
        // `zcode login` adds the identity for its plan.
        let cli = app_only.replace(
            "}",
            r#","account-provider:account:zai-individual-coding-plan:identity":"enc:v1:a.b.c",
            "account-provider:coding-plan:account:zai-individual-coding-plan:account:u1:api-key":"enc:v1:a.b.c"}"#,
        );
        std::fs::write(v2.join("credentials.json"), cli).unwrap();
        assert!((access(&dir) == Access::Ready));
        let _ = std::fs::remove_dir_all(dir);
    }
}
