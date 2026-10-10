//! Kimi Code harness (Moonshot AI's `kimi` CLI).
//!
//! Chat: one `kimi acp` child per turn, driven by the shared ACP adapter
//! ([`super::acp`]). The native session is resumed with `session/resume`; the
//! composer's permission mode maps onto Kimi's session modes (`default` asks,
//! `auto` approves safe operations, `yolo` approves everything, `plan` is
//! read-only).
//!
//! Detection: `kimi` under `~/.kimi-code/bin` (the official installer's
//! location) or on PATH. The Kimi desktop app (`Kimi.exe`) is an Electron
//! bundle, not this CLI, and is never selected. A saved login lives in
//! `~/.kimi-code/credentials/`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use super::acp::{AcpAgent, AcpLaunch, AcpSettings};
use super::detect::{resolve_symlinks, HarnessAuthState, HarnessInfo, ModelInfo};
use super::options::{HarnessOptions, OptionChoice, PermissionMode, PlanActivation};
use super::{Harness, ResumeAction, TurnFailure, TurnOutcome, TurnResult};
use crate::error::{anyhow, Result};
use crate::local::chat::{PromptAnswer, ResumeCtx, TurnCtx, WirePrompt};
use crate::local::shell_env::{find_in_dir, find_on_path};

const KEY: &str = "kimi-code";
const INSTALL_HINT: &str = "Install Kimi Code CLI (Windows: `irm https://code.kimi.com/kimi-code/install.ps1 | iex`; macOS/Linux: `curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash`), then sign in with `kimi login`.";
const LOGIN_HINT: &str = "Run `kimi login` in a terminal, then re-check this harness.";

pub struct KimiCode;

pub(crate) const AGENT: AcpAgent = AcpAgent {
    harness_id: KEY,
    display: "Kimi Code",
    login_hint: LOGIN_HINT,
    skills_dir: Some(".agents/skills"),
    settings,
};

/// Composer state → Kimi session mode.
fn settings(mode: Option<PermissionMode>, plan: bool) -> AcpSettings {
    if plan || mode == Some(PermissionMode::Plan) {
        return AcpSettings {
            mode: Some("plan"),
            ..Default::default()
        };
    }
    match mode.unwrap_or(PermissionMode::Auto) {
        PermissionMode::Ask => AcpSettings {
            mode: Some("default"),
            ..Default::default()
        },
        PermissionMode::Bypass => AcpSettings {
            mode: Some("yolo"),
            auto_allow: true,
            ..Default::default()
        },
        PermissionMode::Auto | PermissionMode::AcceptEdits | PermissionMode::Plan => AcpSettings {
            mode: Some("auto"),
            ..Default::default()
        },
    }
}

fn kimi_home() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".kimi-code"))
}

fn candidates() -> Vec<PathBuf> {
    let installed = kimi_home().and_then(|home| find_in_dir(&home.join("bin"), "kimi"));
    let local =
        dirs::home_dir().and_then(|home| find_in_dir(&home.join(".local").join("bin"), "kimi"));
    let found = installed
        .into_iter()
        .chain(find_on_path("kimi"))
        .chain(local)
        .filter(|path| !super::acp::is_desktop_app(path))
        .map(resolve_symlinks)
        .collect();
    super::detect::unique(found)
}

pub(crate) fn find_kimi() -> Option<PathBuf> {
    super::detect::selected_bin(KEY, candidates())
}

async fn find_kimi_working() -> Option<(PathBuf, super::detect::BinProbe)> {
    super::detect::select_working(KEY, candidates(), None).await
}

/// Whether a Kimi login was saved: a credentials file from `kimi login`.
fn has_credentials(home: &Path) -> bool {
    std::fs::read_dir(home.join("credentials")).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry.path().extension().is_some_and(|ext| ext == "json")
                && entry.metadata().is_ok_and(|meta| meta.len() > 2)
        })
    })
}

/// The models Kimi Code's managed provider offers (as advertised by
/// `session/new`); the live session reports its own list at turn time.
fn models() -> Vec<ModelInfo> {
    [
        ("kimi-code/kimi-for-coding", "K2.8 Preview"),
        ("kimi-code/kimi-for-coding-highspeed", "K2.7 Code Highspeed"),
        ("kimi-code/k3-256k", "K3-256k"),
        ("kimi-code/k3", "K3"),
    ]
    .into_iter()
    .map(|(id, label)| ModelInfo::new(id).with_label(Some(label), None))
    .collect()
}

impl KimiCode {
    async fn detect_at(&self, snapshot: bool) -> Option<HarnessInfo> {
        let mut info = HarnessInfo::new(self.id(), self.name());
        super::detect::record_selected(&mut info, snapshot, KEY, find_kimi, find_kimi_working())
            .await;
        if info.installed && !info.install_broken {
            if kimi_home().is_some_and(|home| has_credentials(&home)) {
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
impl Harness for KimiCode {
    fn id(&self) -> &'static str {
        KEY
    }

    fn name(&self) -> &'static str {
        "Kimi Code"
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
        let result = match find_kimi() {
            Some(program) => {
                super::acp::run_turn(
                    ctx,
                    &AGENT,
                    AcpLaunch {
                        program,
                        args: vec!["acp".into()],
                        env: Vec::new(),
                        path_prepend: Vec::new(),
                    },
                )
                .await
            }
            None => Err(anyhow!("Kimi Code CLI not found. {INSTALL_HINT}")),
        };
        result
            .map(|()| TurnOutcome::Completed)
            .map_err(|error| TurnFailure::adapter(error, ctx.delivery_state()))
    }

    fn options(&self) -> HarnessOptions {
        HarnessOptions::none()
            .with_permission_choices(
                vec![
                    OptionChoice::described("ask", "Ask", "Ask before tools run"),
                    OptionChoice::described(
                        "auto",
                        "Auto",
                        "Approve safe operations automatically",
                    ),
                    OptionChoice::described("bypass", "YOLO", "Approve everything"),
                ],
                "auto",
                PlanActivation::Command,
            )
            .with_reasoning_levels(&["low", "high", "max"])
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
        kimi_home()
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
    use crate::local::harness::acp::tests::opened_session;

    #[test]
    fn permission_modes_map_onto_kimi_session_modes() {
        assert_eq!(
            settings(Some(PermissionMode::Ask), false).mode,
            Some("default")
        );
        assert_eq!(settings(None, false).mode, Some("auto"));
        let bypass = settings(Some(PermissionMode::Bypass), false);
        assert_eq!(bypass.mode, Some("yolo"));
        assert!(bypass.auto_allow);
        let plan = settings(Some(PermissionMode::Bypass), true);
        assert_eq!(plan.mode, Some("plan"));
        assert!(!plan.auto_allow);
    }

    #[test]
    fn advertised_modes_cover_every_mapping() {
        let opened = opened_session(include_str!("fixtures/kimi_acp_ok.jsonl"));
        let modes: Vec<_> = opened["modes"]["availableModes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["id"].as_str())
            .collect();
        for mode in [
            PermissionMode::Ask,
            PermissionMode::Auto,
            PermissionMode::Bypass,
            PermissionMode::Plan,
        ] {
            let wanted = settings(Some(mode), false).mode.unwrap();
            assert!(modes.contains(&wanted), "{wanted} not advertised");
        }
    }

    #[test]
    fn static_models_match_the_advertised_catalog() {
        let opened = opened_session(include_str!("fixtures/kimi_acp_ok.jsonl"));
        let advertised: Vec<_> = super::super::acp::advertised_values(&opened, "model")
            .into_iter()
            .map(|(value, _)| value)
            .collect();
        let ours: Vec<_> = models().into_iter().map(|m| m.id).collect();
        assert_eq!(ours, advertised);
        let levels: Vec<_> = super::super::acp::advertised_values(&opened, "thought_level")
            .into_iter()
            .map(|(value, _)| value)
            .collect();
        assert_eq!(levels, vec!["low", "high", "max"]);
    }

    #[test]
    fn credentials_file_means_signed_in() {
        let dir = std::env::temp_dir().join(format!("orx-kimi-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("credentials")).unwrap();
        assert!(!has_credentials(&dir));
        std::fs::write(
            dir.join("credentials").join("kimi-code.json"),
            r#"{"token":"x"}"#,
        )
        .unwrap();
        assert!(has_credentials(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }
}
