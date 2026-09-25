//! MiniMax Code harness (MiniMax's `mcode` CLI).
//!
//! Chat: one `mcode acp` child per turn, driven by the shared ACP adapter
//! ([`super::acp`]). Permissions map onto the session's `permissionMode`
//! config option (`auto`, `bypassPermissions`); Plan is the session's `plan`
//! mode. The composer offers no "Ask": MiniMax's `default` mode is its own
//! risk policy, which ran out-of-project commands without asking.
//!
//! Detection: the official installer's launchers in `~/.minimax-code`, an npm
//! install into `~/.minimax-code/npm`, or `mcode` on PATH. The installer's
//! managed Node.js (`~/.minimax-code/runtime/node-*`) is put first on the
//! child's PATH, because the npm launcher needs `node` and the machine may
//! have none. The MiniMax Code desktop app shares `~/.minimax`, whose
//! `auth/<env>/<region>/mcode-public` holds the CLI login.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use super::acp::{AcpAgent, AcpLaunch, AcpSettings};
use super::detect::{resolve_symlinks, HarnessAuthState, HarnessInfo, ModelInfo};
use super::options::{HarnessOptions, OptionChoice, PermissionMode, PlanActivation};
use super::{Harness, ResumeAction, TurnFailure, TurnOutcome, TurnResult};
use crate::error::{anyhow, Result};
use crate::local::chat::{PromptAnswer, ResumeCtx, TurnCtx, WirePrompt};
use crate::local::shell_env::{find_in_dir, find_on_path};

const KEY: &str = "minimax-code";
const INSTALL_HINT: &str = "Install MiniMax Code CLI (Windows: `irm https://filecdn.minimax.chat/public/install.ps1 | iex`; macOS/Linux: `curl -fsSL https://filecdn.minimax.chat/public/install.sh | bash`), then sign in with `mcode login --region global` (or `--region cn` for a mainland account).";
const LOGIN_HINT: &str = "Run `mcode login --region global` (or `--region cn` for a mainland account) in a terminal, then re-check this harness.";

pub struct MiniMaxCode;

pub(crate) const AGENT: AcpAgent = AcpAgent {
    harness_id: KEY,
    display: "MiniMax Code",
    login_hint: LOGIN_HINT,
    skills_dir: Some(".agents/skills"),
    settings,
};

/// Composer state → MiniMax session mode and `permissionMode` option.
fn settings(mode: Option<PermissionMode>, plan: bool) -> AcpSettings {
    let permission = match mode.unwrap_or(PermissionMode::Auto) {
        PermissionMode::Ask | PermissionMode::Plan => "default",
        PermissionMode::Bypass => "bypassPermissions",
        PermissionMode::Auto | PermissionMode::AcceptEdits => "auto",
    };
    let plan = plan || mode == Some(PermissionMode::Plan);
    AcpSettings {
        mode: Some(if plan { "plan" } else { "default" }),
        config: vec![("permissionMode", if plan { "default" } else { permission })],
        auto_allow: !plan && permission == "bypassPermissions",
    }
}

fn install_home() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".minimax-code"))
}

fn data_home() -> Option<PathBuf> {
    crate::local::shell_env::var("MINIMAX_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| Some(dirs::home_dir()?.join(".minimax")))
}

fn candidates() -> Vec<PathBuf> {
    let mut dirs_to_search = Vec::new();
    if let Some(home) = install_home() {
        dirs_to_search.push(home.clone());
        dirs_to_search.push(home.join("current"));
        dirs_to_search.push(home.join("npm"));
        dirs_to_search.push(home.join("npm").join("bin"));
    }
    if let Some(home) = dirs::home_dir() {
        dirs_to_search.push(home.join(".local").join("bin"));
    }
    let installed = dirs_to_search
        .iter()
        .filter_map(|dir| find_in_dir(dir, "mcode"))
        .collect::<Vec<_>>();
    let found = installed
        .into_iter()
        .chain(find_on_path("mcode"))
        .map(resolve_symlinks)
        .collect();
    super::detect::unique(found)
}

pub(crate) fn find_mcode() -> Option<PathBuf> {
    super::detect::selected_bin(KEY, candidates())
}

async fn find_mcode_working() -> Option<(PathBuf, super::detect::BinProbe)> {
    // npm's `mcode.cmd` runs `node` from PATH; the installer's Node is not on it.
    super::detect::select_working_with_path(KEY, candidates(), None, child_path()).await
}

/// The newest Node.js the MiniMax installer provisioned, if any.
fn managed_node_dir() -> Option<PathBuf> {
    let runtime = install_home()?.join("runtime");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runtime)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("node-"))
        })
        .collect();
    dirs.sort();
    let dir = dirs.pop()?;
    // Windows archives keep node.exe at the top; Unix ones under bin/.
    if dir.join("node.exe").is_file() {
        Some(dir)
    } else {
        Some(dir.join("bin")).filter(|bin| bin.join("node").is_file())
    }
}

/// PATH for a child that runs `mcode`: the managed Node.js first, when there is one.
pub(crate) fn child_path() -> Option<std::ffi::OsString> {
    let node = managed_node_dir()?;
    let current = crate::local::shell_env::search_path().unwrap_or_default();
    std::env::join_paths(std::iter::once(node).chain(std::env::split_paths(&current))).ok()
}

/// Whether an `mcode login` was saved for any region.
fn has_login(data: &Path) -> bool {
    let Ok(envs) = std::fs::read_dir(data.join("auth")) else {
        return false;
    };
    envs.flatten().any(|env| {
        std::fs::read_dir(env.path()).is_ok_and(|regions| {
            regions.flatten().any(|region| {
                let dir = region.path().join("mcode-public");
                ["auth.json", "credentials.enc", "auth-state.json"]
                    .iter()
                    .any(|name| dir.join(name).metadata().is_ok_and(|meta| meta.len() > 2))
            })
        })
    })
}

/// Whether `config.yaml` configures a custom provider with a key — added by
/// `mcode provider add`, which needs no MiniMax login.
fn has_custom_provider(config: &str) -> bool {
    let mut in_section = false;
    for line in config.lines() {
        let top_level = !line.starts_with(char::is_whitespace);
        if top_level {
            in_section = line.trim_end() == "custom_provider:";
            continue;
        }
        if !in_section {
            continue;
        }
        let entry = line.trim();
        if let Some(key) = entry.strip_prefix("apiKey:") {
            if !key.trim().trim_matches(['"', '\'']).is_empty() {
                return true;
            }
        }
        if let Some(var) = entry.strip_prefix("apiKeyEnv:") {
            let var = var.trim().trim_matches(['"', '\'']);
            if !var.is_empty() && crate::local::shell_env::var(var).is_some() {
                return true;
            }
        }
    }
    false
}

/// MiniMax Code's managed models (as advertised by `session/new`).
fn models() -> Vec<ModelInfo> {
    [
        ("m:minimax:MiniMax-M3:v:thinking", "MiniMax-M3 · thinking"),
        ("m:minimax:MiniMax-M3:v:", "MiniMax-M3"),
        (
            "m:minimax:MiniMax-M2.7-highspeed:v:thinking",
            "MiniMax-M2.7-highspeed · thinking",
        ),
        (
            "m:minimax:MiniMax-M2.7:v:thinking",
            "MiniMax-M2.7 · thinking",
        ),
    ]
    .into_iter()
    .map(|(id, label)| ModelInfo::new(id).with_label(Some(label), None))
    .collect()
}

impl MiniMaxCode {
    async fn detect_at(&self, snapshot: bool) -> Option<HarnessInfo> {
        let mut info = HarnessInfo::new(self.id(), self.name());
        super::detect::record_selected(&mut info, snapshot, KEY, find_mcode, find_mcode_working())
            .await;
        if info.installed && !info.install_broken {
            let custom_provider = data_home().is_some_and(|data| {
                std::fs::read_to_string(data.join("config.yaml"))
                    .is_ok_and(|config| has_custom_provider(&config))
            });
            if crate::local::shell_env::var("MCODE_PROVIDER_API_KEY").is_some() || custom_provider {
                info.authenticated = true;
                info.auth_state = HarnessAuthState::Ready;
                info.auth_method = Some("apiKey");
            } else if data_home().is_some_and(|data| has_login(&data)) {
                info.authenticated = true;
                info.auth_state = HarnessAuthState::Ready;
                info.auth_method = Some("oauth");
            } else {
                info.auth_state = HarnessAuthState::NeedsLogin;
                info.agent_note = Some(LOGIN_HINT.to_string());
            }
        }
        info = info.with_models(models());
        info.agent_ready = info.ready();
        if info.install_broken {
            info.agent_note = Some(info.broken_note(INSTALL_HINT));
        } else if !info.installed {
            info.agent_note = Some(INSTALL_HINT.to_string());
        }
        Some(info)
    }
}

#[async_trait]
impl Harness for MiniMaxCode {
    fn id(&self) -> &'static str {
        KEY
    }

    fn name(&self) -> &'static str {
        "MiniMax Code"
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
        let result = match find_mcode() {
            Some(program) => {
                super::acp::run_turn(
                    ctx,
                    &AGENT,
                    AcpLaunch {
                        program,
                        args: vec!["acp".into()],
                        env: Vec::new(),
                        path_prepend: managed_node_dir().into_iter().collect(),
                    },
                )
                .await
            }
            None => Err(anyhow!("MiniMax Code CLI not found. {INSTALL_HINT}")),
        };
        result
            .map(|()| TurnOutcome::Completed)
            .map_err(|error| TurnFailure::adapter(error, ctx.delivery_state()))
    }

    fn options(&self) -> HarnessOptions {
        HarnessOptions::none().with_permission_choices(
            vec![
                // No "Ask": MiniMax's `default` mode is its own risk policy and
                // ran out-of-project commands without a permission request.
                OptionChoice::described(
                    "auto",
                    "Auto",
                    "Let MiniMax Code decide what needs approval",
                ),
                OptionChoice::described("bypass", "Full access", "Approve everything"),
            ],
            "auto",
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
        data_home()
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
        AGENT.skills_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::harness::acp::advertised_values;
    use crate::local::harness::acp::tests::opened_session;

    #[test]
    fn permission_modes_map_onto_minimax_options() {
        let ask = settings(Some(PermissionMode::Ask), false);
        assert_eq!(ask.mode, Some("default"));
        assert_eq!(ask.config, vec![("permissionMode", "default")]);
        let bypass = settings(Some(PermissionMode::Bypass), false);
        assert_eq!(bypass.config, vec![("permissionMode", "bypassPermissions")]);
        assert!(bypass.auto_allow);
        let plan = settings(Some(PermissionMode::Bypass), true);
        assert_eq!(plan.mode, Some("plan"));
        assert_eq!(plan.config, vec![("permissionMode", "default")]);
        assert!(!plan.auto_allow);
        assert_eq!(
            settings(None, false).config,
            vec![("permissionMode", "auto")]
        );
    }

    #[test]
    fn composer_offers_no_ask_mode() {
        let ids: Vec<_> = MiniMaxCode
            .options()
            .permission_modes
            .into_iter()
            .map(|choice| choice.id)
            .collect();
        assert_eq!(ids, vec!["auto", "bypass"]);
    }

    #[test]
    fn mapped_values_are_advertised_by_the_recorded_session() {
        let opened = opened_session(include_str!("fixtures/minimax_acp_ok.jsonl"));
        let permission: Vec<_> = advertised_values(&opened, "_permission")
            .into_iter()
            .map(|(value, _)| value)
            .collect();
        for mode in [
            PermissionMode::Ask,
            PermissionMode::Auto,
            PermissionMode::Bypass,
        ] {
            let wanted = settings(Some(mode), false).config[0].1;
            assert!(
                permission.iter().any(|v| v == wanted),
                "{wanted} not advertised"
            );
        }
        let mut advertised: Vec<_> = advertised_values(&opened, "model")
            .into_iter()
            .map(|(value, _)| value)
            .collect();
        let mut ours: Vec<_> = models().into_iter().map(|m| m.id).collect();
        advertised.sort();
        ours.sort();
        assert_eq!(ours, advertised);
    }

    #[test]
    fn a_keyed_custom_provider_counts_as_access() {
        let config =
            "logLevel: info\ncustom_provider:\n  mock:\n    options:\n      apiKey: sk-mock\n";
        assert!(has_custom_provider(config));
        assert!(!has_custom_provider(
            "custom_provider:\n  mock:\n    options:\n      apiKey: \"\"\n"
        ));
        assert!(!has_custom_provider("other:\n  apiKey: sk\n"));
    }

    #[test]
    fn saved_login_under_any_region_counts() {
        let dir = std::env::temp_dir().join(format!("orx-minimax-{}", uuid::Uuid::new_v4()));
        let region = dir
            .join("auth")
            .join("prod")
            .join("en")
            .join("mcode-public");
        std::fs::create_dir_all(&region).unwrap();
        std::fs::write(region.join("auth.lock"), "").unwrap();
        assert!(!has_login(&dir));
        std::fs::write(region.join("auth.json"), r#"{"access":"x"}"#).unwrap();
        assert!(has_login(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }
}
