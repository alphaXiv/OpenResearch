//! ZCode harness (Z.ai's coding agent).
//!
//! Chat: one `zcode app-server --stdio` child per turn, speaking the ZCode
//! Protocol (JSON-RPC-shaped NDJSON without the `jsonrpc` field). The turn
//! creates or resumes the native session, moves it onto the composer's mode
//! and model, subscribes, and sends one message. The session's events
//! (`model.streaming`, `tool.updated`, `permission.resolved`, …) arrive as
//! `session/event` notifications until `turn.completed` or `turn.failed`.
//!
//! The composer's modes are ZCode's own: Ask is `build`, Edit is `edit`, Auto
//! is `guarded` (ask when there is risk), Full access is `yolo`, and Plan is
//! ZCode's read-only `plan`. ZCode's approvals, AskUserQuestion and plan
//! approval (ExitPlanMode) come back as requests that orx shows as cards while
//! the turn waits ([`crate::local::chat::ChatHost::request_permission`]).
//!
//! Detection: a `zcode` CLI (official `~/.zcode/runtime` install, npm, PATH),
//! else the runtime bundled with the ZCode desktop app (`resources/glm/zcode.cjs`)
//! run through the app's own Electron binary as Node (`ELECTRON_RUN_AS_NODE=1`).
//! The desktop login in `~/.zcode/v2` is shared with both.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::process::Command;

use super::acp::{Connection, Inbound, RpcError, RpcResult};
use super::detect::{HarnessAuthState, HarnessInfo, ModelInfo};
use super::options::{HarnessOptions, OptionChoice, PermissionMode, PlanActivation};
use super::{Harness, ResumeAction, TurnFailure, TurnOutcome, TurnResult, TURN_WATCHDOG};
use crate::error::{anyhow, Result};
use crate::local::chat::{
    find_part_mut, harness_log, prepare_env, set_chat_session_env, DeliveryState,
    PermissionDecision, PromptAnswer, ResumeCtx, TurnCtx, WirePart, WirePrompt, WireToolState,
};
use crate::local::opencode::ensure_playbook;
use crate::local::shell_env::{find_in_dir, find_on_path};

const KEY: &str = "zcode";
const INSTALL_HINT: &str =
    "Install the ZCode desktop app from https://zcode.z.ai and sign in there, then re-check this harness.";
const LOGIN_HINT: &str =
    "Sign in for command-line use with `zcode login` (Z.ai Individual or Team Coding Plan), or use an API key: add a provider with an API key in the ZCode app (Settings → Providers). Then re-check this harness. Start Plan works only inside the ZCode app.";
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

/// What a sign-in leaves for the runtime: the plan's identity,
/// `account-provider:<id>:identity`, plus its credential. Since ZCode 3.15
/// `account-provider:<id>:auth-source` names which one: `manual` is the
/// identity's own API key, `account-provider:coding-plan:<id>:account:<identity>:api-key`,
/// anything else the account's OAuth token, `oauth:<zai|bigmodel>:access_token`.
/// Earlier versions wrote no source and used the API key. Returns the plans
/// the runtime can use, and whether some plan's key is another account's.
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
        let source = store
            .get(&format!("account-provider:{provider}:auth-source"))
            .and_then(Value::as_str)
            // Unreadable counts as unknown, as before 3.15.
            .and_then(|source| match cipher_key {
                Some(cipher_key) => decrypt_credential(source, cipher_key),
                None => (!source.starts_with("enc:")).then(|| source.to_string()),
            });
        let manual = source.as_deref().map(str::trim) == Some("manual");
        // The account family, as the built-in catalog's `accountType` gives it
        // for every `account:zai-*` and `account:bigmodel-*` plan.
        let family = if provider.contains("bigmodel") {
            "bigmodel"
        } else {
            "zai"
        };
        let token = store
            .get(&format!("oauth:{family}:access_token"))
            .and_then(Value::as_str)
            .is_some_and(|token| !token.trim().is_empty());
        if token && !manual {
            plans.push(provider.to_string());
            continue;
        }
        if source.is_some() && !manual {
            continue;
        }
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

/// How ZCode's runtime can reach a model: through a signed-in plan, or only
/// through providers with an API key.
fn access(home: &Path) -> (Access, bool) {
    let plan = super::detect::read_json(home.join("v2").join("credentials.json"))
        .and_then(|store| {
            store
                .as_object()
                .map(|store| plan_access(store, Some(&credential_cipher_key())))
        })
        .unwrap_or(Access::None);
    if plan == Access::Ready {
        return (plan, false);
    }
    let keyed = provider_config(home).is_some_and(|config| !keyed_providers(&config).is_empty());
    if keyed {
        (Access::Ready, true)
    } else {
        (plan, false)
    }
}

/// The built-in template `id`.
fn template<'a>(builtin: Option<&'a Value>, id: &str) -> Option<&'a Value> {
    builtin?
        .pointer("/config/providerConfigRules/templateRules")?
        .as_array()?
        .iter()
        .find(|rule| rule.get("templateId").and_then(Value::as_str) == Some(id))
}

/// The ZCode app's provider configuration: the providers set up in the app,
/// with their API keys (ZCode keeps them there; orx stores none).
fn provider_config(home: &Path) -> Option<Value> {
    super::detect::read_json(home.join("v2").join("provider_config.json"))
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

/// The composer's provider switch for ZCode: a plan's sign-in, or providers
/// with an API key.
fn provider_group(api_key: bool) -> OptionChoice {
    if api_key {
        OptionChoice::new("api-key", "API key")
    } else {
        OptionChoice::new("coding-plan", "Coding Plan")
    }
}

/// The models ZCode's runtime can use, from the same places it reads them: the
/// built-in models of each plan `zcode login` signed in (from the app's
/// built-in provider catalog), and the models of each enabled API-key provider
/// (its own list, else the models its template enables). Ids are
/// `<provider>/<model>`, the form of ZCode's own model selection.
fn catalog(home: &Path, builtin: Option<&Value>) -> Vec<ModelInfo> {
    let rules = |pointer: &str| {
        builtin
            .and_then(|config| config.pointer(pointer))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let providers = rules("/config/providerConfigRules/providerRules");
    let template_models = rules("/config/modelConfigRules/templateModelRules");
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
    // (provider, model, provider name, reached with an API key)
    let mut entries: Vec<(String, String, String, bool)> = Vec::new();
    let plans = super::detect::read_json(home.join("v2").join("credentials.json"))
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
                entries.push((plan.clone(), model, name.clone(), false));
            }
        }
    }
    if let Some(config) = provider_config(home) {
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
                let template_id = rule
                    .get("templateId")
                    .and_then(Value::as_str)
                    .unwrap_or(provider);
                // The models the template enables, as the runtime lists them;
                // a catalog without model rules offers all of its models.
                let enabled: Vec<String> = template_models
                    .iter()
                    .filter(|rule| {
                        rule.get("templateId").and_then(Value::as_str) == Some(template_id)
                            && rule.pointer("/config/enabled").and_then(Value::as_bool)
                                == Some(true)
                    })
                    .filter_map(|rule| rule.get("modelId").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect();
                models = if enabled.is_empty() {
                    template(builtin, template_id)
                        .map(|template| models_of(template, "builtinModelIds"))
                        .unwrap_or_default()
                } else {
                    enabled
                };
            }
            for model in models {
                entries.push((provider.to_string(), model, name.clone(), true));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    entries
        .into_iter()
        .filter(|(provider, model, _, _)| seen.insert(format!("{provider}/{model}")))
        .map(|(provider, model, name, api_key)| {
            ModelInfo::new(format!("{provider}/{model}"))
                .with_label(Some(&model), Some(&name))
                .with_provider_group(provider_group(api_key))
        })
        .collect()
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
        let launch = find_launch();
        let builtin = launch
            .as_ref()
            .and_then(builtin_catalog)
            .and_then(super::detect::read_json);
        if let Some(launch) = launch.as_ref() {
            info.installed = true;
            info.bin_path = Some(launch.program.to_string_lossy().into_owned());
            if !snapshot {
                match version(launch).await {
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
                let (access, api_key) =
                    zcode_home().map_or((Access::None, false), |home| access(&home));
                match access {
                    Access::Ready => {
                        info.authenticated = true;
                        info.auth_state = HarnessAuthState::Ready;
                        info.auth_method = Some(if api_key { "apiKey" } else { "oauth" });
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
        if let (Some(home), Some(_)) = (zcode_home(), launch.as_ref()) {
            info = info.with_models(catalog(&home, builtin.as_ref()));
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
        HarnessOptions::none()
            .with_permission_choices(
                vec![
                    OptionChoice::described("ask", "Ask", "Ask before changes and commands"),
                    OptionChoice::described(
                        "accept-edits",
                        "Edit",
                        "Allow file edits; ask before commands",
                    ),
                    OptionChoice::described("auto", "Auto", "Ask only when there is risk"),
                    OptionChoice::described("bypass", "Full access", "Allow every tool"),
                ],
                "auto",
                PlanActivation::Command,
            )
            .with_provider_groups(vec![provider_group(false), provider_group(true)])
    }

    async fn resume_from_prompt(
        &self,
        ctx: &ResumeCtx,
        prompt: &WirePrompt,
        answer: &PromptAnswer,
    ) -> Result<ResumeAction> {
        resume_from_prompt(ctx, prompt, answer).await
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

/// ZCode's mode for the composer's permission mode. Plan is ZCode's own
/// read-only mode, left through its plan approval (ExitPlanMode).
fn zcode_mode(mode: Option<PermissionMode>, plan: bool) -> &'static str {
    if plan || mode == Some(PermissionMode::Plan) {
        return "plan";
    }
    match mode.unwrap_or(PermissionMode::Auto) {
        PermissionMode::Ask => "build",
        PermissionMode::AcceptEdits => "edit",
        PermissionMode::Auto | PermissionMode::Plan => "guarded",
        PermissionMode::Bypass => "yolo",
    }
}

/// ZCode before 3.15 has no `guarded` mode (its `auto` is a stub that denies
/// every tool). `edit` is the nearest: edits run, anything else asks.
const GUARDED_FALLBACK: &str = "edit";

fn turn_prompt(text: &str, first: bool) -> String {
    if first {
        format!("{}\n\n{text}", super::acp::playbook_pointer())
    } else {
        text.to_string()
    }
}

/// What ZCode's runtime asks the client while it waits for preferences;
/// the defaults of ZCode's own desktop app, except that questions always
/// reach the user.
fn runtime_preferences() -> Value {
    json!({
        "nativeSearchEnhancementsEnabled": false,
        "memoryEnabled": false,
        "askUserQuestionAutoResolutionEnabled": false,
    })
}

/// Something ZCode asks the user mid-turn, as orx shows it.
#[derive(Debug, Clone, PartialEq)]
enum Ask {
    /// A tool approval, answered with one of ZCode's own options.
    Permission {
        tool: String,
        input: Value,
        options: Vec<Value>,
    },
    /// AskUserQuestion, answered with the chosen option labels.
    Question { input: Value, question: String },
    /// ExitPlanMode: approve the plan, revise it, or stay in plan mode.
    Plan { plan: String },
}

impl Ask {
    /// The request ZCode sent, if orx can show it.
    fn of(method: &str, params: &Value) -> Option<Self> {
        let input = params.get("input").cloned().unwrap_or_else(|| json!({}));
        let tool = params.get("toolName").and_then(Value::as_str);
        match method {
            "interaction/requestPermission" => Some(Ask::Permission {
                tool: tool.unwrap_or("Tool").to_string(),
                input,
                options: params
                    .get("options")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            }),
            "interaction/requestUserInput"
                if params
                    .pointer("/schema/interaction")
                    .and_then(Value::as_str)
                    == Some("plan_approval") =>
            {
                Some(Ask::Plan {
                    plan: input
                        .get("plan")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                })
            }
            "interaction/requestUserInput" if tool == Some("AskUserQuestion") => {
                let question = input
                    .pointer("/questions/0/question")
                    .and_then(Value::as_str)?
                    .to_string();
                Some(Ask::Question { input, question })
            }
            _ => None,
        }
    }

    /// The tool name and input of orx's card (`ChatHost::request_permission`
    /// turns ExitPlanMode into a plan card and AskUserQuestion into a
    /// question card).
    fn card(&self) -> (&str, Value) {
        match self {
            Ask::Permission { tool, input, .. } => (tool, input.clone()),
            Ask::Question { input, .. } => ("AskUserQuestion", input.clone()),
            Ask::Plan { plan } => ("ExitPlanMode", json!({ "plan": plan })),
        }
    }

    /// ZCode's answer for the user's decision. A plan card's Allow carries
    /// `{"feedback": …}` when the user asked for a revision, and a question
    /// card's carries `{"answers": {question: answer}}` (see
    /// `resume_from_prompt`).
    fn reply(&self, decision: &PermissionDecision) -> Value {
        match (self, decision) {
            (Ask::Permission { options, .. }, PermissionDecision::Allow { .. }) => {
                let allows = |option: &&Value| {
                    option.pointer("/response/decision").and_then(Value::as_str) == Some("allow")
                };
                options
                    .iter()
                    .filter(allows)
                    .find(|option| option.get("kind").and_then(Value::as_str) == Some("allow_once"))
                    .or_else(|| options.iter().find(allows))
                    .and_then(|option| option.get("response").cloned())
                    .unwrap_or_else(|| json!({ "decision": "allow" }))
            }
            (Ask::Permission { .. }, PermissionDecision::Deny { message }) => {
                json!({ "decision": "deny", "reason": message })
            }
            (Ask::Question { .. }, PermissionDecision::Allow { updated_input }) => {
                match updated_input
                    .as_ref()
                    .and_then(|input| input.get("answers"))
                {
                    Some(answers) => {
                        json!({ "action": "accept", "content": { "answers": answers } })
                    }
                    None => json!({ "action": "decline" }),
                }
            }
            (Ask::Plan { .. }, PermissionDecision::Allow { updated_input }) => {
                let answer = updated_input
                    .as_ref()
                    .and_then(|input| input.get("feedback"))
                    .and_then(Value::as_str)
                    .unwrap_or(PLAN_APPROVE);
                json!({ "action": "accept", "content": { "answer": answer } })
            }
            (Ask::Question { .. } | Ask::Plan { .. }, PermissionDecision::Deny { .. }) => {
                json!({ "action": "decline" })
            }
        }
    }
}

/// The answer that approves a plan; any other answer is feedback on it.
const PLAN_APPROVE: &str = "approve";

/// ZCode re-announces a request it is still waiting on (a new protocol id,
/// the same `requestId`). One card answers all of them.
#[derive(Default)]
struct Asks {
    waiting: HashMap<String, Vec<Value>>,
    answered: HashMap<String, Value>,
}

#[derive(Default)]
struct TurnState {
    session_id: Option<String>,
    text_part: Option<(String, String)>,
    reasoning_part: Option<(String, String)>,
    completed: bool,
    failure: Option<String>,
    /// ZCode asked for plan approval this turn, so its plan has a card.
    plan_asked: bool,
}

/// One turn's connection to `zcode app-server --stdio`.
struct Turn<'a> {
    conn: &'a Connection,
    inbound: &'a mut tokio::sync::mpsc::UnboundedReceiver<Inbound>,
    state: TurnState,
    asks: Arc<Mutex<Asks>>,
    gate_token: String,
    /// The mode a plan approval hands the session to.
    after_plan: &'static str,
}

impl Turn<'_> {
    /// Send a request and wait for its answer, handling ZCode's own messages
    /// meanwhile.
    async fn call(&mut self, ctx: &mut TurnCtx, method: &str, params: Value) -> Result<RpcResult> {
        let mut rx = self.conn.send(method, params).await?;
        loop {
            tokio::select! {
                biased;
                message = self.inbound.recv() => {
                    let Some(message) = message else {
                        return Ok(rx.try_recv().unwrap_or_else(|_| Err(exited())));
                    };
                    self.handle(ctx, message).await?;
                    ctx.maybe_flush();
                }
                response = &mut rx => return Ok(response.unwrap_or_else(|_| Err(exited()))),
                _ = tokio::time::sleep(TURN_WATCHDOG) => {
                    if !ctx.host.has_pending_permission(&ctx.session_id) {
                        return Err(silent());
                    }
                }
            }
        }
    }

    /// Handle ZCode's messages until the turn completes or fails.
    async fn finish(&mut self, ctx: &mut TurnCtx) -> Result<()> {
        while !self.state.completed && self.state.failure.is_none() {
            match tokio::time::timeout(TURN_WATCHDOG, self.inbound.recv()).await {
                Ok(Some(message)) => {
                    self.handle(ctx, message).await?;
                    ctx.maybe_flush();
                }
                Ok(None) => return Err(anyhow!("ZCode exited before the turn finished")),
                Err(_) if ctx.host.has_pending_permission(&ctx.session_id) => {}
                Err(_) => return Err(silent()),
            }
        }
        Ok(())
    }

    async fn handle(&mut self, ctx: &mut TurnCtx, message: Inbound) -> Result<()> {
        match message {
            Inbound::Notification { method, params } if method == "session/event" => {
                ctx.mark_delivery(DeliveryState::Accepted);
                apply_event(ctx, &mut self.state, &params);
                if let Some(sid) = self.state.session_id.as_deref() {
                    ctx.set_native_session_id(sid);
                }
            }
            Inbound::Notification { .. } => {}
            // Only a host that owns the account answers this; orx runs the
            // runtime signed in by itself, so a request here has no answer.
            Inbound::Request { id, method, .. }
                if method == "interaction/requestProviderRuntimeHeaders" =>
            {
                self.conn
                    .respond(
                        id,
                        json!({
                            "headersApplied": false,
                            "errorMessage": HOST_CREDENTIALS,
                        }),
                    )
                    .await?;
            }
            Inbound::Request { id, method, .. }
                if method == "session/requestRuntimePreferences" =>
            {
                self.conn.respond(id, runtime_preferences()).await?;
            }
            Inbound::Request { id, method, params } => match Ask::of(&method, &params) {
                Some(ask) => self.ask(ctx, id, &params, ask).await?,
                None => {
                    // An unknown interaction answered "no" lets ZCode go on.
                    self.conn
                        .respond_error(
                            id,
                            -32601,
                            &format!("{method} is not supported by OpenResearch"),
                        )
                        .await?
                }
            },
        }
        Ok(())
    }

    /// Show the user a card for `ask`; its answer arrives on a background task
    /// while the turn keeps streaming.
    async fn ask(&mut self, ctx: &TurnCtx, id: Value, params: &Value, ask: Ask) -> Result<()> {
        let request = params
            .get("requestId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| id.to_string());
        let answered = {
            let mut asks = self.asks.lock().unwrap();
            match asks.answered.get(&request).cloned() {
                Some(reply) => Some((id, reply)),
                None => {
                    let waiting = asks.waiting.entry(request.clone()).or_default();
                    waiting.push(id);
                    if waiting.len() > 1 {
                        return Ok(());
                    }
                    None
                }
            }
        };
        if let Some((id, reply)) = answered {
            return self.conn.respond(id, reply).await;
        }
        if matches!(ask, Ask::Plan { .. }) {
            self.state.plan_asked = true;
        }
        let host = ctx.host.clone();
        let session_id = ctx.session_id.clone();
        let native_id = params
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let token = self.gate_token.clone();
        let conn = self.conn.clone();
        let asks = self.asks.clone();
        let after_plan = self.after_plan;
        tokio::spawn(async move {
            let (tool, input) = ask.card();
            let decision = host
                .request_permission(&session_id, &token, tool, input)
                .await
                .unwrap_or_else(|error| PermissionDecision::Deny {
                    message: error.to_string(),
                });
            let reply = ask.reply(&decision);
            let ids = {
                let mut asks = asks.lock().unwrap();
                asks.answered.insert(request.clone(), reply.clone());
                asks.waiting.remove(&request).unwrap_or_default()
            };
            for id in ids {
                let _ = conn.respond(id, reply.clone()).await;
            }
            // An approved plan leaves ZCode in `build`; continue in the
            // composer's own mode instead.
            let approved = matches!(ask, Ask::Plan { .. })
                && reply.pointer("/content/answer").and_then(Value::as_str) == Some(PLAN_APPROVE);
            if let (true, Some(native_id)) = (approved, native_id) {
                let _ = conn
                    .send(
                        "session/setMode",
                        json!({"sessionId": native_id, "mode": after_plan}),
                    )
                    .await;
            }
        });
        Ok(())
    }

    /// Set the session's mode, falling back from `guarded` on a ZCode that
    /// lacks it.
    async fn set_mode(&mut self, ctx: &mut TurnCtx, session: &str, mode: &str) -> Result<()> {
        let mut result = self
            .call(
                ctx,
                "session/setMode",
                json!({"sessionId": session, "mode": mode}),
            )
            .await?;
        if result.is_err() && mode == "guarded" {
            result = self
                .call(
                    ctx,
                    "session/setMode",
                    json!({"sessionId": session, "mode": GUARDED_FALLBACK}),
                )
                .await?;
        }
        result.map(|_| ()).map_err(|error| {
            anyhow!(
                "ZCode could not switch to the chosen mode, so the message was not sent: {error}"
            )
        })
    }
}

fn exited() -> RpcError {
    RpcError {
        code: 0,
        message: "ZCode exited".into(),
    }
}

fn silent() -> crate::error::Error {
    anyhow!(
        "ZCode went silent for {} minutes and was interrupted.",
        TURN_WATCHDOG.as_secs() / 60
    )
}

/// The `session/setModel` selection that moves a session onto the composer's
/// model (`provider/model`) at `reasoning`, else the model's default level;
/// `None` when the session already runs it. `settings` is the session
/// snapshot's.
fn model_selection(settings: &Value, wanted: &str, reasoning: Option<&str>) -> Option<Value> {
    let (provider, model) = wanted.split_once('/')?;
    let offered = settings
        .pointer("/model/available")
        .and_then(Value::as_array)
        .and_then(|all| {
            all.iter().find(|entry| {
                entry.pointer("/ref/providerId").and_then(Value::as_str) == Some(provider)
                    && entry.pointer("/ref/modelId").and_then(Value::as_str) == Some(model)
            })
        });
    let levels: Vec<&str> = offered
        .and_then(|entry| entry.pointer("/reasoning/levels"))
        .and_then(Value::as_array)
        .map(|levels| {
            levels
                .iter()
                .filter_map(|level| level.get("value").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    let level = reasoning
        .filter(|level| levels.contains(level))
        .or_else(|| {
            offered
                .and_then(|entry| entry.pointer("/reasoning/defaultLevel"))
                .and_then(Value::as_str)
        });
    let mut selection = json!({"providerId": provider, "modelId": model});
    if let Some(level) = level {
        selection["options"] = json!({ "reasoningLevel": level });
    }
    let current = settings.pointer("/model/current");
    let same = current.and_then(|c| c.get("providerId")) == selection.get("providerId")
        && current.and_then(|c| c.get("modelId")) == selection.get("modelId")
        && (level.is_none()
            || current
                .and_then(|c| c.pointer("/options/reasoningLevel"))
                .and_then(Value::as_str)
                == level);
    (!same).then_some(selection)
}

/// Runs the desktop app's runtime through orx's loader, which starts its
/// app-server signed in the way `zcode -p` is (see `zcode_app_server.cjs`).
const APP_SERVER_LOADER: &str = include_str!("zcode_app_server.cjs");

/// What the loader prints when this ZCode build cannot be signed in that way.
const STANDALONE_UNAVAILABLE: &str = "orx: ZCode standalone sign-in unavailable";

/// orx's answer when ZCode asks its host for account credentials, which the
/// turn then fails with: the runtime is not signed in by itself.
const HOST_CREDENTIALS: &str = "ZCode asked OpenResearch for its Coding Plan credentials";

/// Why a Coding Plan model cannot run with this ZCode build, when that is the
/// cause (from the turn's message and the loader's word in the log).
fn sign_in_failure(message: &str, log: &str) -> Option<String> {
    let unavailable = log.contains(STANDALONE_UNAVAILABLE);
    if !unavailable && !message.contains(HOST_CREDENTIALS) {
        return None;
    }
    let status = log
        .lines()
        .find_map(|line| line.strip_prefix("orx: ZCode app-server sign-in: "))
        .unwrap_or("not reported");
    Some(format!(
        "OpenResearch could not use ZCode's Coding Plan sign-in with this ZCode version \
         (sign-in loader: {status}). Choose a model of a provider with an API key, or update \
         OpenResearch."
    ))
}

/// The app-server launch: the desktop runtime goes through the loader; a
/// standalone CLI (the npm build signs in by itself) runs as it is.
fn app_server_launch(launch: Launch) -> Result<Launch> {
    let [runtime] = launch.prefix.as_slice() else {
        return Ok(launch);
    };
    if !launch.bundled {
        return Ok(launch);
    }
    let loader = crate::store::data_dir().join("zcode-app-server.cjs");
    if std::fs::read_to_string(&loader).ok().as_deref() != Some(APP_SERVER_LOADER) {
        std::fs::create_dir_all(crate::store::data_dir())?;
        let tmp = loader.with_extension(format!("cjs.{}", uuid::Uuid::new_v4()));
        std::fs::write(&tmp, APP_SERVER_LOADER)?;
        std::fs::rename(&tmp, &loader)?;
    }
    let mut env = launch.env.clone();
    env.push(("ORX_ZCODE_RUNTIME", runtime.clone()));
    Ok(Launch {
        program: launch.program,
        prefix: vec![loader.into_os_string()],
        env,
        bundled: true,
    })
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

    let log_name = format!("zcode-{}", uuid::Uuid::new_v4());
    let launch = app_server_launch(launch)?;
    let mut cmd = launch.command();
    cmd.args(["app-server", "--stdio"])
        .current_dir(&repo)
        .stdin(Stdio::piped())
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
    let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let (conn, mut inbound, reader) = super::acp::connect(stdin, stdout, false);
    let log_path = crate::store::data_dir().join(format!("agent-{log_name}.log"));
    let outcome = drive(ctx, &conn, &mut inbound, &repo).await;
    reader.abort();
    // Closing stdin ends the app-server.
    drop(conn);
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    match outcome {
        Ok(failure) => {
            if let Some(message) = failure {
                ctx.mark_terminal_failure("zcode_turn_failed", explain_failure(message, &log_path));
            } else {
                let _ = std::fs::remove_file(&log_path);
            }
            let _ = ctx.flush();
            Ok(())
        }
        Err(error) => {
            let log = std::fs::read_to_string(&log_path).unwrap_or_default();
            if let Some(why) = sign_in_failure(&error.to_string(), &log) {
                return Err(anyhow!("{error}\n\n{why} (log: {})", log_path.display()));
            }
            Err(anyhow!("{error} (log: {})", log_path.display()))
        }
    }
}

/// Open (or resume) the session, bring it to the composer's mode and model,
/// and run one message. Returns ZCode's own failure message, if the turn
/// failed.
async fn drive(
    ctx: &mut TurnCtx,
    conn: &Connection,
    inbound: &mut tokio::sync::mpsc::UnboundedReceiver<Inbound>,
    repo: &Path,
) -> Result<Option<String>> {
    let plan = ctx.plan_mode || ctx.permission_mode == Some(PermissionMode::Plan);
    let mode = zcode_mode(ctx.permission_mode, plan);
    let bypass = mode == "yolo";
    let mut turn = Turn {
        conn,
        inbound,
        state: TurnState::default(),
        asks: Arc::default(),
        gate_token: ctx.host.mint_gate_token(&ctx.session_id, plan, bypass),
        after_plan: zcode_mode(
            ctx.permission_mode.filter(|m| *m != PermissionMode::Plan),
            false,
        ),
    };
    let path = repo.to_string_lossy().into_owned();
    let workspace = json!({"workspacePath": path, "workspaceKey": path});

    let mut opened = None;
    if let Some(native) = ctx.native_session_id.clone() {
        // A session ZCode no longer has starts over; orx keeps the transcript.
        if let Ok(snapshot) = turn
            .call(
                ctx,
                "session/resume",
                json!({"sessionId": native, "workspace": workspace}),
            )
            .await?
        {
            turn.set_mode(ctx, &native, mode).await?;
            opened = Some((native, snapshot));
        }
    }
    let resumed = opened.is_some();
    let (session, snapshot) = match opened {
        Some(opened) => opened,
        None => {
            let create = |mode: &str| json!({"workspace": workspace, "mode": mode});
            let mut created = turn.call(ctx, "session/create", create(mode)).await?;
            if created.is_err() && mode == "guarded" {
                created = turn
                    .call(ctx, "session/create", create(GUARDED_FALLBACK))
                    .await?;
            }
            let snapshot =
                created.map_err(|error| anyhow!("ZCode could not start a session: {error}"))?;
            let session = snapshot
                .pointer("/session/sessionId")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("ZCode started a session without an id"))?
                .to_string();
            (session, snapshot)
        }
    };
    ctx.set_native_session_id(&session);

    if let Some(model) = ctx.model.clone() {
        let settings = snapshot.get("settings").cloned().unwrap_or(Value::Null);
        if let Some(selection) = model_selection(&settings, &model, ctx.reasoning_level.as_deref())
        {
            turn.call(
                ctx,
                "session/setModel",
                json!({"sessionId": session, "model": selection, "persistAsWorkspaceLastUsed": false}),
            )
            .await?
            .map_err(|error| anyhow!("ZCode could not switch to {model}, so the message was not sent: {error}"))?;
        }
    }
    turn.call(
        ctx,
        "session/subscribe",
        json!({"sessionId": session, "deliveryKind": "desktop-continuous"}),
    )
    .await?
    .map_err(|error| anyhow!("ZCode did not stream the session: {error}"))?;
    turn.call(
        ctx,
        "session/send",
        json!({"sessionId": session, "content": turn_prompt(&ctx.text, !resumed)}),
    )
    .await?
    .map_err(|error| {
        ctx.mark_delivery(DeliveryState::NotSent);
        anyhow!("ZCode did not accept the message: {error}")
    })?;
    ctx.mark_delivery(DeliveryState::Accepted);
    turn.finish(ctx).await?;

    // ZCode's plan mode ends a turn without asking for approval when the
    // model writes its plan as text; that plan still needs a card.
    if plan && !turn.state.plan_asked {
        if let Some(card) =
            super::acp::synthesized_plan_card(&ctx.assistant.parts, &ctx.assistant.id)
        {
            ctx.upsert_part(card);
        }
    }
    Ok(turn.state.failure.take())
}

/// Answer a card ZCode is waiting on inside the running turn: a tool
/// approval, a plan approval (ExitPlanMode) or a question (AskUserQuestion).
/// End-turn plan cards (a plan written as text) continue with a new message.
async fn resume_from_prompt(
    ctx: &ResumeCtx,
    prompt: &WirePrompt,
    answer: &PromptAnswer,
) -> Result<ResumeAction> {
    let Some(native_id) = &prompt.native_id else {
        return super::acp::resume_from_prompt(ctx, prompt, answer).await;
    };
    if !ctx.is_busy().await {
        ctx.host
            .resolve_zombie_prompt(&ctx.session_id, &answer.prompt_id);
        return Err(anyhow!("this approval is no longer pending"));
    }
    let note = answer
        .note
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty());
    let (decision, plan_mode) = match (prompt.kind.as_str(), answer.approve) {
        ("permission", true) => (
            PermissionDecision::Allow {
                updated_input: prompt.tool_input.clone(),
            },
            None,
        ),
        ("permission", false) => (
            PermissionDecision::Deny {
                message: match note {
                    Some(note) => format!(
                        "The user denied this action: {note}. Do not retry it; adjust course."
                    ),
                    None => "The user denied this action. Do not retry it; adjust course.".into(),
                },
            },
            None,
        ),
        // ZCode leaves plan mode itself and implements in the same turn.
        ("plan", true) => (
            PermissionDecision::Allow {
                updated_input: None,
            },
            Some(false),
        ),
        // A note asks ZCode to revise the plan; none rejects it.
        ("plan", false) => match note {
            Some(note) => (
                PermissionDecision::Allow {
                    updated_input: Some(json!({ "feedback": note })),
                },
                None,
            ),
            None => (
                PermissionDecision::Deny {
                    message: "The user rejected this plan.".into(),
                },
                None,
            ),
        },
        ("question", _) => {
            let text = answer.contextualized_answer(answer.plain_answer_text());
            if text.trim().is_empty() {
                return Err(anyhow!("select an option (or add a note) to answer"));
            }
            let question = prompt.question.clone().unwrap_or_default();
            (
                PermissionDecision::Allow {
                    updated_input: Some(json!({ "answers": { question: text } })),
                },
                None,
            )
        }
        _ => return Err(anyhow!("unsupported prompt kind for a ZCode card")),
    };
    ctx.host.settle_permission(native_id, decision)?;
    Ok(ResumeAction::Handled { plan_mode })
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
    let log = std::fs::read_to_string(log_path).unwrap_or_default();
    if let Some(why) = sign_in_failure(&message, &log) {
        return format!("{message}\n\n{why} Log: {}", log_path.display());
    }
    if message.contains(NO_MODEL) {
        return format!(
            "ZCode found no model it can use from the command line ({NO_MODEL}). \
             {COMMAND_LINE_ACCESS} If you signed in to the app with another account since \
             `zcode login`, run `zcode login` again. Log: {}",
            log_path.display()
        );
    }
    // The runtime's own Coding Plan sign-in: Z.ai refused the token `zcode
    // login` saved (it lapsed, or another sign-in replaced it), or it is gone.
    if message.contains("project_token_login_failed")
        || message.contains("project_token_login_required")
    {
        return format!(
            "{message}\n\nZ.ai did not accept ZCode's command-line sign-in (`zcode login`): it may \
             have expired. Sign in again under Settings → Agents → ZCode → Sign in again, then \
             continue. Log: {}",
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
                        part_state.error = Some(format!("Denied: {reason}"));
                    }
                }
            }
        }
        Some("session.titleUpdated") => {
            let pointer = super::acp::playbook_pointer();
            if let Some(title) = payload
                .get("title")
                .and_then(Value::as_str)
                .and_then(|title| super::acp::agent_title(title, &[&pointer]))
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
        // A chat with no mode chosen gets ZCode's risk-based mode, never YOLO.
        assert_eq!(zcode_mode(None, false), "guarded");
        assert_eq!(ZCode.options().default_permission_mode, Some("auto"));
        assert_eq!(zcode_mode(Some(PermissionMode::Ask), false), "build");
        assert_eq!(zcode_mode(Some(PermissionMode::AcceptEdits), false), "edit");
        assert_eq!(zcode_mode(Some(PermissionMode::Auto), false), "guarded");
        assert_eq!(zcode_mode(Some(PermissionMode::Bypass), false), "yolo");
        // Plan is ZCode's own plan mode, whatever mode the composer had.
        assert_eq!(zcode_mode(Some(PermissionMode::Bypass), true), "plan");
        assert_eq!(zcode_mode(Some(PermissionMode::Plan), false), "plan");
    }

    /// A recorded `zcode app-server --stdio` exchange: what orx sent (`>>`)
    /// and what ZCode sent (`<<`).
    fn exchange(fixture: &str) -> Vec<(bool, Value)> {
        fixture
            .lines()
            .map(|line| {
                let (sent, body) = match line.split_at(3) {
                    (">> ", body) => (true, body),
                    ("<< ", body) => (false, body),
                    _ => panic!("fixture line without direction: {line}"),
                };
                (
                    sent,
                    serde_json::from_str(body).expect("fixture line is JSON"),
                )
            })
            .collect()
    }

    /// ZCode's first request of `method`, and the answer recorded for it.
    fn request_and_answer(fixture: &str, method: &str) -> (Value, Value) {
        let messages = exchange(fixture);
        let request = messages
            .iter()
            .find(|(sent, m)| !sent && m.get("method").and_then(Value::as_str) == Some(method))
            .map(|(_, m)| m.clone())
            .expect("fixture has the request");
        let answer = messages
            .iter()
            .find(|(sent, m)| {
                *sent && m.get("id") == request.get("id") && m.get("result").is_some()
            })
            .map(|(_, m)| m["result"].clone())
            .expect("fixture has the answer");
        (request["params"].clone(), answer)
    }

    #[test]
    fn folds_an_app_server_turn() {
        let fixture = include_str!("fixtures/zcode_app_server_permission.jsonl");
        let mut ctx = TurnCtx::test_stub();
        let mut state = TurnState::default();
        for (sent, message) in exchange(fixture) {
            if !sent && message.get("method").and_then(Value::as_str) == Some("session/event") {
                apply_event(&mut ctx, &mut state, &message["params"]);
            }
        }
        assert!(state.completed);
        assert!(state.failure.is_none());
        assert!(state
            .session_id
            .as_deref()
            .is_some_and(|id| id.starts_with("sess_")));
        let tool = ctx
            .assistant
            .parts
            .iter()
            .find(|part| part.kind == "tool")
            .expect("tool part");
        assert_eq!(tool.tool.as_deref(), Some("Bash"));
        assert_eq!(tool.state.as_ref().unwrap().status, "completed");
        let texts: Vec<_> = ctx
            .assistant
            .parts
            .iter()
            .filter(|part| part.kind == "text")
            .filter_map(|part| part.text.as_deref())
            .collect();
        assert_eq!(texts.len(), 2);
        assert!(texts[1].starts_with("Done."));
    }

    #[test]
    fn a_tool_approval_answers_with_zcodes_own_options() {
        let (params, recorded) = request_and_answer(
            include_str!("fixtures/zcode_app_server_permission.jsonl"),
            "interaction/requestPermission",
        );
        let ask = Ask::of("interaction/requestPermission", &params).unwrap();
        let (tool, input) = ask.card();
        assert_eq!(tool, "Bash");
        assert!(input["command"]
            .as_str()
            .unwrap()
            .starts_with("rm -rf build"));
        let allow = ask.reply(&PermissionDecision::Allow {
            updated_input: Some(input),
        });
        assert_eq!(allow, recorded);
        assert_eq!(allow["decision"], "allow");
        let deny = ask.reply(&PermissionDecision::Deny {
            message: "The user denied this action.".into(),
        });
        assert_eq!(
            deny,
            json!({"decision": "deny", "reason": "The user denied this action."})
        );
    }

    #[test]
    fn a_plan_approval_becomes_a_plan_card() {
        let (params, recorded) = request_and_answer(
            include_str!("fixtures/zcode_app_server_plan.jsonl"),
            "interaction/requestUserInput",
        );
        let ask = Ask::of("interaction/requestUserInput", &params).unwrap();
        assert_eq!(
            ask,
            Ask::Plan {
                plan: "1. Create hello.txt\n2. Run git status".into()
            }
        );
        assert_eq!(ask.card().0, "ExitPlanMode");
        let approve = ask.reply(&PermissionDecision::Allow {
            updated_input: None,
        });
        assert_eq!(approve, recorded);
        // A note is feedback on the plan; a plain rejection declines it.
        assert_eq!(
            ask.reply(&PermissionDecision::Allow {
                updated_input: Some(json!({"feedback": "Use Rust"})),
            }),
            json!({"action": "accept", "content": {"answer": "Use Rust"}})
        );
        assert_eq!(
            ask.reply(&PermissionDecision::Deny {
                message: "the turn was interrupted".into(),
            }),
            json!({"action": "decline"})
        );
    }

    #[test]
    fn a_question_is_answered_by_its_text() {
        let (params, recorded) = request_and_answer(
            include_str!("fixtures/zcode_app_server_question.jsonl"),
            "interaction/requestUserInput",
        );
        let ask = Ask::of("interaction/requestUserInput", &params).unwrap();
        let (tool, input) = ask.card();
        assert_eq!(tool, "AskUserQuestion");
        let card = crate::local::harness::question_prompt(tool, Some(&input)).unwrap();
        assert_eq!(card.question.as_deref(), Some("Which language?"));
        assert_eq!(card.options.len(), 2);
        let answer = ask.reply(&PermissionDecision::Allow {
            updated_input: Some(json!({"answers": {"Which language?": "Rust"}})),
        });
        assert_eq!(answer, recorded);
        // The tool reports the answer back to the model.
        let result = exchange(include_str!("fixtures/zcode_app_server_question.jsonl"))
            .into_iter()
            .find_map(|(_, m)| {
                (m.pointer("/params/payload/kind").and_then(Value::as_str) == Some("result"))
                    .then(|| m["params"]["payload"]["result"]["content"].clone())
            })
            .unwrap();
        assert!(result
            .as_str()
            .unwrap()
            .contains("\"Which language?\"=\"Rust\""));
        // Other interactions are not shown.
        assert_eq!(
            Ask::of(
                "interaction/requestUserInput",
                &json!({"toolName": "Other"})
            ),
            None
        );
    }

    #[test]
    fn the_session_switches_to_the_composers_model() {
        let fixture = include_str!("fixtures/zcode_app_server_permission.jsonl");
        let snapshot = exchange(fixture)
            .into_iter()
            .find_map(|(sent, m)| (!sent && m.pointer("/result/settings").is_some()).then_some(m))
            .unwrap();
        let settings = &snapshot["result"]["settings"];
        // Already on the model at its default level: nothing to switch.
        assert_eq!(model_selection(settings, "custom/mock-model", None), None);
        assert_eq!(
            model_selection(settings, "custom/mock-model", Some("disabled")),
            Some(json!({
                "providerId": "custom",
                "modelId": "mock-model",
                "options": {"reasoningLevel": "disabled"}
            }))
        );
        // A level the model lacks keeps its default.
        assert_eq!(
            model_selection(settings, "custom/mock-model", Some("max")),
            None
        );
        // A model the session does not list is still asked for, so ZCode
        // says why it cannot use it.
        assert_eq!(
            model_selection(settings, "account:zai-individual-coding-plan/GLM-5.3", None),
            Some(json!({"providerId": "account:zai-individual-coding-plan", "modelId": "GLM-5.3"}))
        );
        assert_eq!(model_selection(settings, "no-provider", None), None);
    }

    /// The loader makes both of the npm repack's edits on the app's runtime
    /// (standalone registry, and its credentials for each session), leaves a
    /// build that already chooses its sign-in alone, and refuses a build it
    /// does not know. It then runs the patched runtime as the main module.
    #[test]
    fn the_loader_signs_the_app_server_in_like_the_cli() {
        let Some(node) = crate::local::shell_env::find_on_path("node") else {
            eprintln!("node not found; skipping the loader test");
            return;
        };
        let dir = std::env::temp_dir().join(format!("orx-zcode-loader-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let loader = dir.join("loader.cjs");
        std::fs::write(&loader, APP_SERVER_LOADER).unwrap();
        let script = r#"
const { standalone } = require(process.argv[1]);
const names = 'r(x$C,"startProcessProviderRegistryRuntime");r(gDe,"createCliProviderRefreshReporter");';
const ready = 'u.info("Worker Provider Registry \\u5DF2\\u5C31\\u7EEA");';
const call = 'let z=e.env??process.env;U=await FUe({signal:s,create:r(()=>x$C(z),"create"),disposeLate:r(e=>e.dispose(),"disposeLate")}),' + ready;
const factory = 'let Fe=U,bt=1;new JEt({createZCodeApp:r((ut={})=>INn({...q0c($0c(ut,n),Fe.runtime.registryService,Fe.configuredDefaultModelSelection),env:{}}),"createZCodeApp")});';
const app = standalone(names + call + factory);
const npm = standalone(names + 'create:r(()=>x$C(z,require("cli-config.cjs").appServerRegistryOptions(z,gDe)),"create"),' + ready + factory);
const noFactory = standalone(names + call);
const overridden = standalone(names + call + factory.replace('env:{}', 'providerRuntimeHeadersPort:h'));
const unknown = standalone(names + 'create:r(()=>other(z),"create"),' + ready + factory);
console.log(JSON.stringify({ app, npm: npm.status, noFactory: noFactory.status, overridden: overridden.status, unknown: unknown.status }));
"#;
        let output = std::process::Command::new(&node)
            .arg("-e")
            .arg(script)
            .arg(&loader)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["app"]["status"], "patched");
        let patched = result["app"]["source"].as_str().unwrap();
        assert!(
            patched.contains(r#"create:r(()=>x$C(z,{standalone:gDe(process.stderr)}),"create")"#)
        );
        assert!(patched.contains(
            "Fe.configuredDefaultModelSelection),...Fe.providerRuntimeHeadersPort?{providerRuntimeHeadersPort:Fe.providerRuntimeHeadersPort}:{},env:{}"
        ));
        assert_eq!(result["npm"], "native");
        assert_eq!(result["noFactory"], "unavailable");
        assert_eq!(result["overridden"], "unavailable");
        assert_eq!(result["unknown"], "unavailable");

        // Run as the loader: the runtime is the main module, with its own argv.
        let runtime = dir.join("zcode.cjs");
        std::fs::write(
            &runtime,
            "#!/usr/bin/env node\nconsole.log(JSON.stringify({main: require.main === module, file: __filename, args: process.argv.slice(2)}));\n",
        )
        .unwrap();
        let output = std::process::Command::new(&node)
            .arg(&loader)
            .args(["app-server", "--stdio"])
            .env("ORX_ZCODE_RUNTIME", &runtime)
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let ran: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(ran["main"], true);
        assert_eq!(ran["file"], runtime.to_str().unwrap());
        assert_eq!(ran["args"], json!(["app-server", "--stdio"]));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("orx: ZCode app-server sign-in: unavailable"),
            "{stderr}"
        );
        assert!(stderr.contains(STANDALONE_UNAVAILABLE), "{stderr}");
    }

    /// A turn that asked orx for account credentials, or ran on a build the
    /// loader could not sign in, says so with the loader's status.
    #[test]
    fn a_host_credentials_request_explains_the_sign_in() {
        let why = sign_in_failure(
            HOST_CREDENTIALS,
            "orx: ZCode app-server sign-in: patched\nother\n",
        )
        .unwrap();
        assert!(why.contains("sign-in loader: patched"), "{why}");
        let why = sign_in_failure(
            "model_request_failed",
            &format!("orx: ZCode app-server sign-in: unavailable\n{STANDALONE_UNAVAILABLE}\n"),
        )
        .unwrap();
        assert!(why.contains("sign-in loader: unavailable"), "{why}");
        assert!(sign_in_failure(
            "project_token_login_failed",
            "orx: ZCode app-server sign-in: patched"
        )
        .is_none());
    }

    /// A refused `zcode login` token says how to sign in again.
    #[test]
    fn a_refused_sign_in_asks_to_sign_in_again() {
        let explained = explain_failure(
            "project_token_login_failed".into(),
            Path::new("/tmp/zcode.log"),
        );
        assert!(explained.contains("Sign in again"), "{explained}");
        assert!(explained.contains("/tmp/zcode.log"), "{explained}");
    }

    /// The built-in catalog's Z.ai API template, as ZCode 3.15.3 ships it.
    fn zai_api_catalog() -> Value {
        json!({"config": {
            "providerConfigRules": {
                "providerRules": [
                    {"providerId": "account:zai-individual-coding-plan", "providerName": "Z.AI Individual Coding Plan",
                     "config": {"builtinModelIds": ["GLM-5.3"]}}],
                "templateRules": [
                    {"templateId": "zai-standard-api", "templateNameMap": {"en-US": "Z.ai API"},
                     "config": {"access": {"type": "api-key"}, "builtinModelIds": ["GLM-5.3", "GLM-5.3-Flash", "GLM-5"]}}]},
            "modelConfigRules": {"templateModelRules": [
                {"modelId": "GLM-5.3", "templateId": "zai-standard-api", "config": {"enabled": true}},
                {"modelId": "GLM-5.3-Flash", "templateId": "zai-standard-api", "config": {"enabled": true}},
                {"modelId": "GLM-5", "templateId": "zai-standard-api", "config": {"enabled": false}}]}}})
    }

    #[test]
    fn the_catalog_groups_models_by_how_zcode_reaches_them() {
        let dir = std::env::temp_dir().join(format!("orx-zcode-groups-{}", uuid::Uuid::new_v4()));
        let v2 = dir.join("v2");
        std::fs::create_dir_all(&v2).unwrap();
        let identity = "account-provider:account:zai-individual-coding-plan:identity";
        std::fs::write(
            v2.join("credentials.json"),
            json!({identity: "enc:v1:a.b.c", "oauth:zai:access_token": "enc:v1:a.b.c"}).to_string(),
        )
        .unwrap();
        std::fs::write(
            v2.join("provider_config.json"),
            json!({"config": {"providerConfigRules": {"providerRules": [
                {"providerId": "zai-standard-api", "templateId": "zai-standard-api", "providerName": "Z.ai API",
                 "config": {"group": "standard-personal", "access": {"type": "api-key", "apiKey": "sk"}}}]}}})
            .to_string(),
        )
        .unwrap();
        let builtin = zai_api_catalog();
        let models: Vec<_> = catalog(&dir, Some(&builtin))
            .into_iter()
            .map(|model| (model.id, model.provider_group.unwrap().id))
            .collect();
        assert_eq!(
            models,
            [
                ("account:zai-individual-coding-plan/GLM-5.3", "coding-plan"),
                // Only the models the template enables, as the runtime lists them.
                ("zai-standard-api/GLM-5.3", "api-key"),
                ("zai-standard-api/GLM-5.3-Flash", "api-key"),
            ]
            .map(|(id, group)| (id.to_string(), group.to_string()))
        );
        // A provider the app has without a key, or switched off, offers nothing.
        for rule in [
            json!({"providerId": "zai-standard-api", "templateId": "zai-standard-api",
                   "config": {"group": "standard-personal", "access": {"type": "api-key", "apiKey": null}}}),
            json!({"providerId": "zai-standard-api", "templateId": "zai-standard-api", "enabled": false,
                   "config": {"group": "standard-personal", "access": {"type": "api-key", "apiKey": "sk"}}}),
        ] {
            let config = json!({"config": {"providerConfigRules": {"providerRules": [rule]}}});
            assert!(keyed_providers(&config).is_empty());
        }
        // The plan's sign-in makes ZCode ready, not the key.
        assert_eq!(access(&dir), (Access::Ready, false));
        std::fs::remove_file(v2.join("credentials.json")).unwrap();
        assert_eq!(access(&dir), (Access::Ready, true));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn questions_always_reach_the_user() {
        let preferences = runtime_preferences();
        assert_eq!(preferences["askUserQuestionAutoResolutionEnabled"], false);
        assert_eq!(preferences["memoryEnabled"], false);
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
    fn the_auth_source_picks_the_oauth_token_or_the_api_key() {
        let identity = "account-provider:account:zai-individual-coding-plan:identity";
        let source = "account-provider:account:zai-individual-coding-plan:auth-source";
        let api_key =
            "account-provider:coding-plan:account:zai-individual-coding-plan:account:1234567:api-key";
        let store = |entries: &[(&str, &str)]| {
            entries
                .iter()
                .map(|(key, value)| (key.to_string(), Value::from(*value)))
                .collect::<serde_json::Map<_, _>>()
        };
        let key = test_key();
        // ZCode 3.15's browser sign-in: identity and OAuth token, no plan key.
        let oauth = [
            (identity, SEALED_IDENTITY),
            (source, "oauth"),
            ("oauth:zai:access_token", "enc:v1:a.b.c"),
        ];
        assert_eq!(plan_access(&store(&oauth), Some(&key)), Access::Ready);
        assert_eq!(plan_access(&store(&oauth), None), Access::Ready);
        // Without a source (before 3.15) the token is enough too.
        assert_eq!(
            plan_access(&store(&[oauth[0], oauth[2]]), Some(&key)),
            Access::Ready
        );
        // The token's family is the plan's.
        let bigmodel = [
            (
                "account-provider:account:bigmodel-individual-coding-plan:identity",
                SEALED_IDENTITY,
            ),
            ("oauth:zai:access_token", "enc:v1:a.b.c"),
        ];
        assert_eq!(plan_access(&store(&bigmodel), Some(&key)), Access::None);
        // An OAuth source with no token, or a key source with no key.
        assert_eq!(
            plan_access(&store(&[oauth[0], oauth[1], (api_key, "k")]), Some(&key)),
            Access::None
        );
        let manual = [
            (identity, SEALED_IDENTITY),
            (source, "manual"),
            ("oauth:zai:access_token", "enc:v1:a.b.c"),
        ];
        assert_eq!(plan_access(&store(&manual), Some(&key)), Access::None);
        let with_key = [manual[0], manual[1], manual[2], (api_key, "k")];
        assert_eq!(plan_access(&store(&with_key), Some(&key)), Access::Ready);
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
        let builtin = super::super::detect::read_json(builtin).unwrap();
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

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn access_comes_from_a_login_or_a_keyed_provider() {
        let dir = std::env::temp_dir().join(format!("orx-zcode-home-{}", uuid::Uuid::new_v4()));
        let v2 = dir.join("v2");
        std::fs::create_dir_all(&v2).unwrap();
        assert!(!(access(&dir).0 == Access::Ready));
        // The desktop app's sign-in alone: OAuth tokens and plan keys, but no
        // identity the command-line runtime can pair them with.
        let app_only = r#"{"oauth:zai:access_token":"enc:v1:a.b.c","zcodejwttoken":"enc:v1:a.b.c",
            "account-provider:coding-plan:account:zai-team-coding-plan:account:u1:api-key":"enc:v1:a.b.c"}"#;
        std::fs::write(v2.join("credentials.json"), app_only).unwrap();
        assert!(!(access(&dir).0 == Access::Ready));
        let disabled = r#"{"config":{"providerConfigRules":{"providerRules":[{"providerId":"zai-api","enabled":false,"config":{"access":{"type":"zhipu-coding-plan-api-key","apiKey":"id.secret"}}}]}}}"#;
        std::fs::write(v2.join("provider_config.json"), disabled).unwrap();
        assert!(!(access(&dir).0 == Access::Ready));
        std::fs::write(
            v2.join("provider_config.json"),
            r#"{"config":{"providerConfigRules":{"providerRules":[{"providerId":"c","config":{"access":{"type":"api-key","apiKey":"sk"}}}]}}}"#,
        )
        .unwrap();
        assert!((access(&dir).0 == Access::Ready));
        std::fs::remove_file(v2.join("provider_config.json")).unwrap();
        // An identity whose plan has no API key does not reach a model either.
        let identity_only =
            r#"{"account-provider:account:zai-individual-coding-plan:identity":"enc:v1:a.b.c"}"#;
        std::fs::write(v2.join("credentials.json"), identity_only).unwrap();
        assert!(!(access(&dir).0 == Access::Ready));
        // ZCode 3.15's sign-in pairs the identity with the OAuth token instead.
        let oauth = r#"{"account-provider:account:zai-individual-coding-plan:identity":"enc:v1:a.b.c",
            "account-provider:account:zai-individual-coding-plan:auth-source":"oauth",
            "oauth:zai:access_token":"enc:v1:a.b.c"}"#;
        std::fs::write(v2.join("credentials.json"), oauth).unwrap();
        assert!((access(&dir).0 == Access::Ready));
        // `zcode login` adds the identity for its plan.
        let cli = app_only.replace(
            "}",
            r#","account-provider:account:zai-individual-coding-plan:identity":"enc:v1:a.b.c",
            "account-provider:coding-plan:account:zai-individual-coding-plan:account:u1:api-key":"enc:v1:a.b.c"}"#,
        );
        std::fs::write(v2.join("credentials.json"), cli).unwrap();
        assert!((access(&dir).0 == Access::Ready));
        let _ = std::fs::remove_dir_all(dir);
    }
}
