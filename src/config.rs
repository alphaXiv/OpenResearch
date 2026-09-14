//! Credential storage, XDG paths, and the default API URL.
//!
//! Credentials live at
//! `$XDG_CONFIG_HOME/openresearch/credentials.json` (falling back to
//! `~/.config/openresearch/credentials.json`), written owner-only (mode 0600).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::error::Result;

/// Base URL of the API. Defaults to prod (`https://api.openresearch.sh`), so a
/// plain `orx login` just works. Override per-invocation with `--api-url` (handled
/// in the `login` command) or set the `OPENRESEARCH_API_URL` env var to point at
/// local dev, e.g. `OPENRESEARCH_API_URL=http://localhost:4000 orx login`.
pub fn default_api_url() -> String {
    std::env::var("OPENRESEARCH_API_URL")
        .unwrap_or_else(|_| "https://api.openresearch.sh".to_string())
}

/// Base URL for the alphaXiv JSON API (full-text literature search). Unlike the
/// OpenResearch API these endpoints are public — no token — and live on a
/// different host. Override with `ALPHAXIV_API_URL`.
pub fn alphaxiv_api_url() -> String {
    std::env::var("ALPHAXIV_API_URL").unwrap_or_else(|_| "https://api.alphaxiv.org".to_string())
}

/// Base URL for the alphaXiv web app, which serves the per-paper `.md` routes
/// (`/overview/<id>.md` report, `/abs/<id>.md` full text). Default is the `www.`
/// host so we skip the apex→www 301. Override with `ALPHAXIV_WEB_URL`.
pub fn alphaxiv_web_url() -> String {
    std::env::var("ALPHAXIV_WEB_URL").unwrap_or_else(|_| "https://www.alphaxiv.org".to_string())
}

/// Base URL for the OpenAlex REST API (scholarly works search). Public, no
/// token. Backs OpenAlex and bioRxiv discovery. Override with `OPENALEX_API_URL`.
pub fn openalex_api_url() -> String {
    std::env::var("OPENALEX_API_URL").unwrap_or_else(|_| "https://api.openalex.org".to_string())
}

/// Base URL for the bioRxiv API (per-DOI preprint details). Public, no token.
/// Backs `orx paper <biorxiv-doi>`. Override with `BIORXIV_API_URL`.
pub fn biorxiv_api_url() -> String {
    std::env::var("BIORXIV_API_URL").unwrap_or_else(|_| "https://api.biorxiv.org".to_string())
}

/// Contact address sent to OpenAlex as `mailto=` to enter its faster "polite
/// pool". OpenAlex asks API users to identify themselves this way. Override with
/// `OPENALEX_MAILTO`.
pub fn openalex_mailto() -> String {
    std::env::var("OPENALEX_MAILTO").unwrap_or_else(|_| "orx@alphaxiv.org".to_string())
}

/// Stored credentials: the API base URL and the bearer token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    #[serde(rename = "apiUrl")]
    pub api_url: String,
    pub token: String,
}

pub(crate) fn config_dir() -> PathBuf {
    let base = crate::local::shell_env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config")
        });
    base.join("openresearch")
}

fn credentials_path() -> PathBuf {
    config_dir().join("credentials.json")
}

/// Whether `orx login` credentials exist on disk. A cheap sync fs probe for
/// summary views — whether the token still *works* is a network question
/// (`load_credentials` + an API call).
pub fn credentials_present() -> bool {
    credentials_path().exists()
}

/// Reads stored credentials. Returns `Ok(None)` when the file is missing,
/// unreadable, malformed, or missing required fields — matching the TS
/// `loadCredentials`, which swallows all errors and returns `null`.
pub async fn load_credentials() -> Result<Option<Credentials>> {
    let path = credentials_path();
    let raw = match fs::read_to_string(&path).await {
        Ok(raw) => raw,
        Err(_) => return Ok(None),
    };
    match serde_json::from_str::<Credentials>(&raw) {
        Ok(creds) if !creds.api_url.is_empty() && !creds.token.is_empty() => Ok(Some(creds)),
        _ => Ok(None),
    }
}

/// Creates `dir` (recursively) and makes sure it isn't group/world-traversable.
/// Idempotent and safe to call on a dir that already exists with looser
/// permissions — it tightens those too, since `~/.config` itself is
/// world-traversable by convention and a stale dir predating this check
/// would otherwise stay that way forever.
///
/// Directory-level, not just file-level: a 0600 file inside a 0755 directory
/// stops other users from *reading* it, but not from noticing it exists or
/// racing its creation.
async fn ensure_private_dir(dir: &std::path::Path) -> Result<()> {
    fs::create_dir_all(dir).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).await?;
    }
    Ok(())
}

/// Writes `body` to `path`, owner-only (mode 0600) from the moment the file
/// is created — never briefly world/group-readable under the process umask,
/// the way write-then-chmod would leave it. The trailing `set_permissions`
/// is a second pass for the case the file already existed (`mode()` only
/// applies when a new file is created), so a stale, looser-permissioned file
/// from before this fix also gets tightened on the next write.
async fn write_owner_only(path: &std::path::Path, body: &[u8]) -> Result<()> {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    {
        use tokio::io::AsyncWriteExt;
        options.open(path).await?.write_all(body).await?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
    }

    Ok(())
}

/// Persists credentials as pretty JSON with a trailing newline, mode 0600.
pub async fn save_credentials(creds: &Credentials) -> Result<()> {
    let path = credentials_path();
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent).await?;
    }
    let body = format!("{}\n", serde_json::to_string_pretty(creds)?);
    write_owner_only(&path, body.as_bytes()).await
}

/// Removes the credentials file. Succeeds even if it does not exist (`force`).
pub async fn clear_credentials() -> Result<()> {
    match fs::remove_file(credentials_path()).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Where the Overleaf Git authentication token and session cookie live. Deliberately *not*
/// `~/.openresearch/env`: `list_synced_env` fans that file out to every compute
/// backend and into the agent's environment, and nothing off this machine
/// pushes to Overleaf.
fn overleaf_credentials_path() -> PathBuf {
    config_dir().join("overleaf.json")
}

#[derive(Default, Serialize, Deserialize)]
struct OverleafCredentials {
    #[serde(default)]
    token: String,
    /// The browser session cookie the live editor channel authenticates with,
    /// as `name=value`. Separate from the token: the git bridge is a paid
    /// feature and the cookie is not, so either can be present alone.
    #[serde(default)]
    session: String,
    /// The Overleaf host the cookie belongs to; it is sent nowhere else.
    #[serde(default)]
    session_host: String,
}

fn overleaf_credentials() -> OverleafCredentials {
    std::fs::read_to_string(overleaf_credentials_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Sync counterpart of `ensure_private_dir`/`write_owner_only` (this module's
/// callers, chiefly the Overleaf credential setters, are sync — the git
/// bridge they back predates this module's async credential path). Same
/// atomic-mode-at-creation reasoning, blocking `std::fs` instead of `tokio::fs`.
fn ensure_private_dir_sync(dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_owner_only_sync(path: &std::path::Path, body: &[u8]) -> Result<()> {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600); // applies on create only
    }
    {
        use std::io::Write;
        options.open(path)?.write_all(body)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Writes owner-only, like `save_credentials` beside it. An empty file goes
/// away rather than lingering with nothing in it.
fn save_overleaf_credentials(credentials: &OverleafCredentials) -> Result<()> {
    let path = overleaf_credentials_path();
    if credentials.token.is_empty() && credentials.session.is_empty() {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        };
    }
    if let Some(parent) = path.parent() {
        ensure_private_dir_sync(parent)?;
    }
    let body = serde_json::to_string_pretty(credentials)?;
    write_owner_only_sync(&path, format!("{body}\n").as_bytes())
}

fn non_empty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

pub fn overleaf_token() -> Option<String> {
    non_empty(overleaf_credentials().token)
}

pub fn set_overleaf_token(token: &str) -> Result<()> {
    let mut credentials = overleaf_credentials();
    credentials.token = token.to_string();
    save_overleaf_credentials(&credentials)
}

pub fn clear_overleaf_token() -> Result<()> {
    let mut credentials = overleaf_credentials();
    credentials.token.clear();
    save_overleaf_credentials(&credentials)
}

/// The session cookie and the host it was issued by.
pub fn overleaf_session() -> Option<(String, String)> {
    let credentials = overleaf_credentials();
    let session = non_empty(credentials.session)?;
    let host = non_empty(credentials.session_host)?;
    Some((host, session))
}

pub fn set_overleaf_session(host: &str, session: &str) -> Result<()> {
    let mut credentials = overleaf_credentials();
    credentials.session = session.to_string();
    credentials.session_host = host.to_string();
    save_overleaf_credentials(&credentials)
}

pub fn clear_overleaf_session() -> Result<()> {
    let mut credentials = overleaf_credentials();
    credentials.session.clear();
    credentials.session_host.clear();
    save_overleaf_credentials(&credentials)
}

/// The user-chosen data dir, if one is persisted and non-empty. Consumed by
/// `store::data_dir()` between the `$ORX_DATA_DIR` override and the XDG default.
///
/// `settings.json` (in `config_dir()`, deliberately *outside* the data dir so it
/// can point *at* it without a chicken-and-egg) is owned by `crate::telemetry`,
/// which already guards it with an in-process mutex + cross-process flock +
/// atomic temp-and-rename RMW. We delegate rather than add a second writer — a
/// naive whole-object write here would clobber `installId`/`telemetryDisabled`
/// (and vice-versa). Sync because `store::data_dir()` calls it on every open.
pub fn settings_data_dir() -> Option<PathBuf> {
    crate::telemetry::persisted_data_dir().map(PathBuf::from)
}

pub fn settings_cache_dir() -> Option<PathBuf> {
    crate::telemetry::persisted_cache_dir().map(PathBuf::from)
}

/// Set or clear the persisted data dir, preserving every other settings field.
/// Delegates to `telemetry::set_persisted_data_dir` (the locked, atomic RMW).
/// `None` clears it (revert to the env/XDG/default chain).
pub fn set_settings_data_dir(data_dir: Option<String>) -> Result<()> {
    crate::telemetry::set_persisted_data_dir(data_dir)?;
    Ok(())
}

/// The persisted default compute target for local-mode launches, if any:
/// `(backend, flavor)`. Lives in the telemetry-owned `settings.json` for the
/// same single-writer reason as the data dir above.
pub fn compute_default() -> Option<(String, Option<String>)> {
    crate::telemetry::compute_default()
}

/// Set or clear the default compute target, preserving every other settings
/// field. Delegates to `telemetry::set_compute_default` (the locked, atomic
/// RMW). `None` backend clears both backend and flavor.
pub fn set_compute_default(backend: Option<String>, flavor: Option<String>) -> Result<()> {
    crate::telemetry::set_compute_default(backend, flavor)?;
    Ok(())
}

/// Literature sources the user disabled in Settings (their `LitSource::as_str()`
/// names). Lives in the telemetry-owned `settings.json`; enforced by discovery,
/// `orx discover`, and `orx paper`. Empty = all enabled.
pub fn disabled_lit_sources() -> Vec<String> {
    crate::telemetry::disabled_lit_sources()
}

pub fn github_for_new_projects() -> bool {
    crate::telemetry::github_for_new_projects()
}

pub fn set_github_for_new_projects(enabled: bool) -> Result<()> {
    crate::telemetry::set_github_for_new_projects(enabled)?;
    Ok(())
}

/// Whether orx may install updates on its own (Settings → Updates). Lives in
/// the telemetry-owned `settings.json` for the same single-writer reason as the
/// data dir above. Read by `updates::auto_update_eligible`.
pub fn auto_update_enabled() -> bool {
    crate::telemetry::auto_update_enabled()
}

pub fn set_auto_update_enabled(enabled: bool) -> Result<()> {
    crate::telemetry::set_auto_update_enabled(enabled)?;
    Ok(())
}

pub fn github_default_prompt_seen() -> bool {
    crate::telemetry::github_default_prompt_seen()
}

pub fn set_github_default_prompt_seen(seen: bool) -> Result<()> {
    crate::telemetry::set_github_default_prompt_seen(seen)?;
    Ok(())
}

/// Look up a var from the box's synced env file (`~/.openresearch/env`, written
/// by the api's env sync). Needed because non-interactive shells never source
/// it via .bashrc (Ubuntu's interactive guard returns first), so an agent's
/// `orx` can't rely on the process environment alone. Parses only the exact
/// format the api writes: `export KEY='value'` with `\` doubled and `'`
/// written as `'\''`.
pub fn synced_env_var(key: &str) -> Option<String> {
    let path = dirs::home_dir()?.join(".openresearch").join("env");
    let content = std::fs::read_to_string(path).ok()?;
    let prefix = format!("export {key}='");
    for line in content.lines() {
        let Some(rest) = line.strip_prefix(&prefix) else {
            continue;
        };
        let Some(escaped) = rest.strip_suffix('\'') else {
            continue;
        };
        // Invert buildEnvFile's escaping (quotes first, then backslashes).
        let value = escaped.replace(r"'\''", "'").replace(r"\\", r"\");
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

/// All vars in the synced env file, in file order (same format as
/// `synced_env_var`). Malformed lines are skipped.
pub fn list_synced_env() -> Vec<(String, String)> {
    let Some(path) = dirs::home_dir().map(|h| h.join(".openresearch").join("env")) else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in content.lines() {
        let Some(rest) = line.strip_prefix("export ") else {
            continue;
        };
        let Some((key, quoted)) = rest.split_once('=') else {
            continue;
        };
        let Some(escaped) = quoted.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) else {
            continue;
        };
        let value = escaped.replace(r"'\''", "'").replace(r"\\", r"\");
        if !key.is_empty() && !value.is_empty() {
            out.push((key.to_string(), value));
        }
    }
    out
}

/// Drop `key`'s line from the synced env file. Missing file/key is a no-op.
pub fn remove_synced_env_var(key: &str) -> Result<()> {
    let Some(path) = dirs::home_dir().map(|h| h.join(".openresearch").join("env")) else {
        return Ok(());
    };
    let Ok(existing) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let prefix = format!("export {key}=");
    let lines: Vec<&str> = existing
        .lines()
        .filter(|l| !l.starts_with(&prefix))
        .collect();
    let body = if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    };
    std::fs::write(&path, body)?;
    Ok(())
}

/// Write `export KEY='value'` into `~/.openresearch/env` (the exact format
/// `synced_env_var` parses), replacing an existing line for `key` and keeping
/// every other line. File is owner-only (0600) on create and rewrite.
pub fn write_synced_env_var(key: &str, value: &str) -> Result<()> {
    write_synced_env_vars(&[(key, value)])
}

pub fn write_synced_env_vars(values: &[(&str, &str)]) -> Result<()> {
    use anyhow::anyhow;
    let dir = dirs::home_dir()
        .ok_or_else(|| anyhow!("no home directory"))?
        .join(".openresearch");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("env");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    for (key, value) in values {
        // Inverse of synced_env_var's unescaping: backslashes first, then quotes.
        let escaped = value.replace('\\', r"\\").replace('\'', r"'\''");
        let new_line = format!("export {key}='{escaped}'");
        let prefix = format!("export {key}=");
        let mut replaced = false;
        lines.retain_mut(|line| {
            if !line.starts_with(&prefix) {
                return true;
            }
            if replaced {
                return false;
            }
            *line = new_line.clone();
            replaced = true;
            true
        });
        if !replaced {
            lines.push(new_line);
        }
    }
    let body = format!("{}\n", lines.join("\n"));
    write_owner_only_sync(&path, body.as_bytes())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    // Each test uses its own throwaway dir under the OS temp dir (the
    // codebase-wide idiom for filesystem tests — see the `orx-tel-*` dirs in
    // telemetry.rs) rather than touching `$XDG_CONFIG_HOME`/`config_dir()`,
    // which real credential paths resolve through: mutating that env var
    // races other modules' tests under the parallel runner (see the
    // `ENV_LOCK` note in telemetry.rs) and, if a test ever forgot to
    // sandbox it, would silently clobber the developer's own
    // ~/.config/openresearch/credentials.json.
    fn scratch_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("orx-config-{label}-{}", uuid::Uuid::new_v4()))
    }

    fn mode(path: &std::path::Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[tokio::test]
    async fn write_owner_only_creates_file_mode_0600() {
        let dir = scratch_dir("write-owner-only");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secret.json");
        write_owner_only(&path, b"{\"token\":\"x\"}\n")
            .await
            .unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"token\":\"x\"}\n"
        );
    }

    #[tokio::test]
    async fn write_owner_only_tightens_a_preexisting_looser_file() {
        let dir = scratch_dir("write-owner-only-tighten");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secret.json");
        std::fs::write(&path, "stale").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        write_owner_only(&path, b"fresh").await.unwrap();

        assert_eq!(mode(&path), 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fresh");
    }

    #[tokio::test]
    async fn ensure_private_dir_creates_mode_0700() {
        let dir = scratch_dir("ensure-private-dir").join("nested");
        ensure_private_dir(&dir).await.unwrap();
        assert_eq!(mode(&dir), 0o700);
    }

    #[tokio::test]
    async fn ensure_private_dir_tightens_a_preexisting_looser_dir() {
        let dir = scratch_dir("ensure-private-dir-tighten");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        ensure_private_dir(&dir).await.unwrap();

        assert_eq!(mode(&dir), 0o700);
    }

    #[test]
    fn write_owner_only_sync_creates_file_mode_0600() {
        let dir = scratch_dir("write-owner-only-sync");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("overleaf.json");
        write_owner_only_sync(&path, b"{}\n").unwrap();
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn ensure_private_dir_sync_creates_mode_0700() {
        let dir = scratch_dir("ensure-private-dir-sync").join("nested");
        ensure_private_dir_sync(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
    }
}
