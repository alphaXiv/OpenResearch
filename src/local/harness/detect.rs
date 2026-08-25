//! Shared detection primitives for the harness registry — the wire types every
//! harness reports (`HarnessInfo`, `ModelInfo`) and the best-effort probes
//! (`--version`, auth-file reads, JWT decode) the per-harness impls build on.
//!
//! Detection is read-only and best-effort: missing files or unparseable JSON
//! just mean "not detected", never an error.

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

pub(super) const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HarnessAuthState {
    Ready,
    NeedsLogin,
    Unknown,
    Unsupported,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    /// Reasoning/effort choices this *specific* model accepts, led by the
    /// `Default` sentinel. `None` means "this model has no list of its own" —
    /// the composer then falls back to the harness-wide
    /// [`HarnessOptions::reasoning_levels`](super::HarnessOptions).
    ///
    /// `Some(vec![])` is meaningfully different from `None`: it means the model
    /// was *checked* and genuinely exposes no reasoning control (an OpenCode
    /// model with an empty `variants` map), so the picker is hidden entirely
    /// rather than falling back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_levels: Option<Vec<super::options::OptionChoice>>,
    /// The catalog's own human name for the model (`Opus`, `GPT-5.6 Sol`,
    /// `Big Pickle`). Absent for statically-listed fallback models, where the
    /// UI derives a label from the id instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The catalog's one-line blurb — for Claude this is where the resolved
    /// version lives (`Opus 4.8 with 1M context · Best for everyday, complex
    /// tasks`), since its picker aliases (`opus[1m]`) are unversioned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The tier that actually runs when the user picks nothing — set only when
    /// the CLI reports it (codex's `defaultReasoningEffort`, resolved against a
    /// `config.toml` override). When present, `reasoning_levels` carries no
    /// `default` sentinel and the composer preselects this concrete tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_reasoning_level: Option<String>,
}

impl ModelInfo {
    /// A model with no per-model reasoning metadata (falls back to the
    /// harness-wide list).
    pub(super) fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            reasoning_levels: None,
            display_name: None,
            description: None,
            default_reasoning_level: None,
        }
    }

    /// Attach reasoning choices *with a known concrete default*: no sentinel
    /// row, the default tier preselected instead. For catalogs that report
    /// which tier runs when nothing is chosen (codex).
    pub(super) fn with_reasoning_default(mut self, ids: &[&str], default: &str) -> Self {
        self.reasoning_levels = Some(super::options::reasoning_tiers(ids));
        // A default outside the advertised tiers would be unselectable — leave
        // it unset then, and the composer preselects the first tier.
        self.default_reasoning_level = ids.contains(&default).then(|| default.to_string());
        self
    }

    /// Attach the catalog's display name / description, when it has them.
    pub(super) fn with_label(
        mut self,
        display_name: Option<&str>,
        description: Option<&str>,
    ) -> Self {
        self.display_name = display_name.map(str::to_string);
        self.description = description.map(str::to_string);
        self
    }

    /// Attach this model's own reasoning choices, from native ids. An empty
    /// `ids` yields an empty (not absent) list — "checked, none supported".
    pub(super) fn with_reasoning(mut self, ids: &[&str]) -> Self {
        self.reasoning_levels = Some(if ids.is_empty() {
            Vec::new()
        } else {
            super::options::reasoning_choices(ids)
        });
        self
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bin_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// A signed-in setup was found (auth file / OAuth account).
    pub authenticated: bool,
    /// Live credential readiness. Account metadata never implies this state.
    pub auth_state: HarnessAuthState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_method: Option<&'static str>, // "oauth" | "apiKey"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Usable as a chat backend right now (installed + signed in).
    pub agent_ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_note: Option<String>,
    /// Whether a running turn accepts further user input, which is what lets
    /// the composer steer instead of parking the message until the turn ends.
    pub supports_steering: bool,
    pub models: Vec<ModelInfo>,
    /// Composer toggle vocabulary (permission modes, reasoning levels).
    pub options: super::HarnessOptions,
}

impl HarnessInfo {
    pub(super) fn new(id: &'static str, name: &'static str) -> Self {
        Self {
            id,
            name,
            installed: false,
            bin_path: None,
            version: None,
            authenticated: false,
            auth_state: HarnessAuthState::Unknown,
            auth_method: None,
            account: None,
            org: None,
            plan: None,
            agent_ready: false,
            agent_note: None,
            supports_steering: false,
            models: Vec::new(),
            options: super::HarnessOptions::none(),
        }
    }

    /// Attach the chat model list. Each `ModelInfo` carries its own reasoning
    /// choices where the harness knows them (issue #123).
    pub(super) fn with_models(mut self, models: Vec<ModelInfo>) -> Self {
        self.models = models;
        self
    }
}

/// Dereference symlinks to the real installed binary. Installers commonly drop
/// a lone symlink into `~/.local/bin`, but some CLIs locate sibling helper
/// executables relative to the path they were *invoked as*, without resolving
/// symlinks — codex >= 0.144 launches `codex-code-mode-host` this way and every
/// command fails with "No such file or directory" when codex is spawned via the
/// symlink. Spawning the resolved path keeps helpers real siblings. Best-effort:
/// a path that can't be resolved is returned unchanged.
pub(super) fn resolve_symlinks(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

/// `<bin> --version`, first line, with a timeout (node CLIs can be slow).
pub(super) async fn bin_version(bin: &PathBuf) -> Option<String> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("--version").stdin(std::process::Stdio::null());
    // A node-shebang install needs `node` on PATH to answer at all, and an
    // unparseable version downgrades a signed-in harness to `Unknown` — which
    // is also why a synced `FORCE_COLOR` must not reach the version line.
    crate::local::chat::prepare_env(&mut cmd);
    cmd.env("NO_COLOR", "1");
    let fut = cmd.output();
    let out = tokio::time::timeout(VERSION_TIMEOUT, fut)
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .to_string();
    (!line.is_empty()).then_some(line)
}

/// An API key from the process env, else orx's own synced env file — the two
/// sources `prepare_env` actually hands the harness child. Detecting only the
/// former would report a working setup as signed out.
pub(super) fn api_key(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .or_else(|| crate::config::synced_env_var(key))
}

pub(super) fn read_json(path: PathBuf) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub(super) fn nonempty_str(v: &Value, key: &str) -> Option<String> {
    v.get(key)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Decode a JWT's payload without verifying — we only surface the account
/// email and plan the user is already signed in as, locally.
pub(super) fn jwt_payload(token: &str) -> Option<Value> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Parse a `major.minor.patch` triple out of a `--version` line. The first
/// whitespace-separated token that parses wins, so `"codex-cli 0.144.0"`,
/// `"2.1.197 (Claude Code)"`, and a bare `"0.144.0"` all resolve; a `-suffix`
/// on the patch is tolerated. `None` when no token has the shape, which each
/// caller treats as "assume the older behaviour".
pub(super) fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    version.split_whitespace().find_map(|token| {
        let mut parts = token.splitn(3, '.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts
            .next()?
            .split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()?;
        Some((major, minor, patch))
    })
}

pub(super) fn title_case(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn resolve_symlinks_dereferences_to_real_binary() {
        let dir = std::env::temp_dir().join(format!("orx-detect-test-{}", std::process::id()));
        let install = dir.join("install");
        let bin = dir.join("bin");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let real = install.join("codex");
        std::fs::write(&real, "").unwrap();
        let link = bin.join("codex");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert_eq!(resolve_symlinks(link), real.canonicalize().unwrap());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_symlinks_keeps_unresolvable_path() {
        let missing = PathBuf::from("/nonexistent/orx-detect-test/codex");
        assert_eq!(resolve_symlinks(missing.clone()), missing);
    }
}
