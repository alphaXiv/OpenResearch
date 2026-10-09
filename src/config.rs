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

/// Base URL for NCBI E-utilities (PubMed search and records). Public, no
/// token. Backs PubMed discovery and `orx paper <pmid>`. Override with
/// `PUBMED_API_URL`.
pub fn pubmed_api_url() -> String {
    std::env::var("PUBMED_API_URL")
        .unwrap_or_else(|_| "https://eutils.ncbi.nlm.nih.gov/entrez/eutils".to_string())
}

/// Contact address sent to NCBI as `email=` alongside `tool=orx`, which NCBI
/// asks E-utilities clients to include. Override with `NCBI_EMAIL`.
pub fn ncbi_email() -> String {
    std::env::var("NCBI_EMAIL").unwrap_or_else(|_| "orx@alphaxiv.org".to_string())
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

/// Persists credentials as pretty JSON with a trailing newline, mode 0600.
pub async fn save_credentials(creds: &Credentials) -> Result<()> {
    let path = credentials_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let body = format!("{}\n", serde_json::to_string_pretty(creds)?);
    fs::write(&path, body).await?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        fs::set_permissions(&path, perms).await?;
    }

    Ok(())
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
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(credentials)?;
    std::fs::write(&path, format!("{body}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
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

/// All vars in one synced env file, in file order (same format as
/// `synced_env_var`). Malformed lines are skipped.
fn list_synced_env_from(path: &std::path::Path) -> Vec<(String, String)> {
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

fn list_synced_env() -> Vec<(String, String)> {
    let Some(path) = dirs::home_dir().map(|h| h.join(".openresearch").join("env")) else {
        return Vec::new();
    };
    list_synced_env_from(&path)
}

/// The saved variables shown in Settings. Callers must still mask secret
/// values before returning them to a user-facing surface.
pub fn synced_env_for_display() -> Vec<(String, String)> {
    list_synced_env()
}

/// Saved variables inherited by the local research agent. Compute backends
/// must use [`run_env`] instead, so Trackio reachability cannot be bypassed by
/// forwarding this file wholesale.
pub fn agent_env() -> Vec<(String, String)> {
    list_synced_env()
}

/// Drop `key`'s line from the synced env file. Missing file/key is a no-op.
pub fn remove_synced_env_var(key: &str) -> Result<()> {
    let Some(path) = dirs::home_dir().map(|h| h.join(".openresearch").join("env")) else {
        return Ok(());
    };
    remove_synced_env_var_at(&path, key)
}

fn remove_synced_env_var_at(path: &std::path::Path, key: &str) -> Result<()> {
    let Ok(existing) = std::fs::read_to_string(path) else {
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
    std::fs::write(path, body)?;
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
    write_synced_env_vars_at(&dir.join("env"), values)
}

fn write_synced_env_vars_at(path: &std::path::Path, values: &[(&str, &str)]) -> Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
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
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600); // applies on create only
        }
        opts.open(path)?.write_all(body.as_bytes())?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SshHostSettings {
    pub container: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshSettings {
    pub default_host: Option<String>,
    #[serde(default)]
    pub hosts: std::collections::BTreeMap<String, SshHostSettings>,
}

pub fn ssh_settings() -> Result<SshSettings> {
    crate::telemetry::ssh_settings()
}

pub fn set_ssh_host(host: String, options: SshHostSettings) -> Result<()> {
    crate::jobs::ssh::validate_host_options(&options)?;
    crate::telemetry::set_ssh_host(host, options)?;
    Ok(())
}

pub fn set_ssh_default(host: Option<String>) -> Result<()> {
    crate::telemetry::set_ssh_default(host)?;
    Ok(())
}

// --- trackio ------------------------------------------------------------------
//
// Optional experiment tracking against a Trackio server the user already runs
// (`trackio show`, self-hosted — orx never starts, tunnels or supervises one).
// Configuration is three variables in the synced env file, so it reaches every
// backend and the research agent through the existing `list_synced_env` path
// with no new plumbing.

/// Base URL of a self-hosted Trackio server. Trackio's own Python client reads
/// this variable, so a run needs nothing else to log there.
pub const TRACKIO_SERVER_URL: &str = "TRACKIO_SERVER_URL";
/// Write token for that server. Trackio's Python client reads this variable and
/// sends it as `X-Trackio-Write-Token`; ingestion is rejected without it unless
/// the server runs on a Space.
pub const TRACKIO_WRITE_TOKEN: &str = "TRACKIO_WRITE_TOKEN";
/// Default project name. Trackio's *Python* client does **not** read this — it
/// is the convention Trackio's own Rust/Go/JS contrib clients use, and the name
/// orx puts in the agent's instructions and in the dashboard link.
pub const TRACKIO_PROJECT: &str = "TRACKIO_PROJECT";
/// Per-run name, same convention as [`TRACKIO_PROJECT`]. Orx stamps the run id
/// so a Trackio run can be traced back to the run that produced it.
pub const TRACKIO_RUN: &str = "TRACKIO_RUN";
/// TensorBoard event-file directory for this run. Training frameworks can use
/// it directly as `logging_dir`; no server, credential or package mutation is
/// implied by ORX setting it.
pub const TENSORBOARD_LOGDIR: &str = "TENSORBOARD_LOGDIR";

/// A var from the process environment, falling back to the synced env file.
/// Same precedence the dashboard states: orx's own environment wins.
fn env_or_synced(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| synced_env_var(key))
}

/// The Trackio address and its token, resolved together from whichever source
/// won, or `None` when tracking is switched off.
///
/// Address and token are one credential pair, so they are read from the same
/// source rather than independently: overriding `TRACKIO_SERVER_URL` in the
/// process environment with a new server's write-access URL must not keep
/// sending the *old* server's stored token to it. A standalone
/// `TRACKIO_WRITE_TOKEN` from that same source still wins over one embedded in
/// the URL, since it is the more explicit of the two.
///
/// The address comes back bare. Saving it through the dashboard already splits
/// an embedded `write_token` out, but a variable exported straight into the
/// environment never passed through that path, and Trackio's own documentation
/// hands out exactly that form of URL — so normalise here too, or the token ends
/// up pasted into the middle of every probe and dashboard link.
fn trackio_connection() -> Option<(String, Option<String>)> {
    trackio_connection_from(&process_env, &synced_env_var)
}

/// A non-empty var from orx's own process environment.
fn process_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// [`trackio_connection`] with its two sources passed in: `process` stands for
/// orx's own environment and `synced` for the synced env file.
fn trackio_connection_from(
    process: &dyn Fn(&str) -> Option<String>,
    synced: &dyn Fn(&str) -> Option<String>,
) -> Option<(String, Option<String>)> {
    if let Some(raw) = process(TRACKIO_SERVER_URL) {
        return Some(pair_trackio_connection(&raw, process(TRACKIO_WRITE_TOKEN)));
    }
    let raw = synced(TRACKIO_SERVER_URL)?;
    Some(pair_trackio_connection(&raw, synced(TRACKIO_WRITE_TOKEN)))
}

/// The pairing [`trackio_connection`] applies once it has picked a source, with
/// that source's two values passed in so it is testable without touching the
/// process environment.
fn pair_trackio_connection(
    raw_url: &str,
    same_source_token: Option<String>,
) -> (String, Option<String>) {
    let (url, embedded) = split_trackio_write_token(raw_url);
    (url, same_source_token.or(embedded))
}

/// The configured Trackio server, or `None` when tracking is switched off.
/// Everything Trackio-shaped in orx keys off this being set.
pub fn trackio_server_url() -> Option<String> {
    Some(trackio_connection()?.0)
}

/// The configured default Trackio project, if any.
pub fn trackio_project() -> Option<String> {
    env_or_synced(TRACKIO_PROJECT)
}

/// The write token belonging to the resolved server, if any. Never log, print
/// or persist the result.
pub fn trackio_write_token() -> Option<String> {
    trackio_connection()?.1
}

/// Split a `write_token` query parameter out of a Trackio server URL, returning
/// the bare URL and the token.
///
/// `trackio show` prints exactly one write-access URL — `http://host:7860/?write_token=…`
/// — so that is what users paste. Keeping the token inside the URL would leak it
/// into the unmasked URL field, into every dashboard link and into anything that
/// echoes the server address, so it is stored under [`TRACKIO_WRITE_TOKEN`]
/// instead.
///
/// The whole query string is dropped, not just the token. Trackio documents no
/// other parameter on a server address, and everything downstream — `/version`,
/// `/api/*`, the `?project=` dashboard link — is built by appending to this
/// base, which a surviving query would turn into `…?project=demo/version`.
pub fn split_trackio_write_token(url: &str) -> (String, Option<String>) {
    let url = url.trim();
    let Some((base, query)) = url.split_once('?') else {
        return (url.to_string(), None);
    };
    let mut token = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        // A bare or empty `write_token` carries nothing; it is dropped with the
        // rest rather than read as a configured token.
        if key == "write_token" && !value.is_empty() {
            token = Some(
                urlencoding::decode(value)
                    .unwrap_or(value.into())
                    .into_owned(),
            );
        }
    }
    (base.to_string(), token)
}

/// Save a Trackio address entered in Settings, keeping the saved write token
/// paired with the server it was issued for.
///
/// An embedded `write_token` is split out and replaces the saved token. With
/// no embedded token, the saved one is kept for the same server (a trailing
/// slash is not a different server) and for a first address saved after its
/// token, but dropped when the address moves to a different server: the
/// Settings probe would otherwise send it to that server as
/// `X-Trackio-Write-Token`.
pub fn save_trackio_server_url(raw_url: &str) -> Result<()> {
    use anyhow::anyhow;
    let dir = dirs::home_dir()
        .ok_or_else(|| anyhow!("no home directory"))?
        .join(".openresearch");
    std::fs::create_dir_all(&dir)?;
    save_trackio_server_url_at(&dir.join("env"), raw_url)
}

fn save_trackio_server_url_at(path: &std::path::Path, raw_url: &str) -> Result<()> {
    let (url, embedded_token) = split_trackio_write_token(raw_url);
    let server = |url: &str| url.trim_end_matches('/').to_string();
    let moved = list_synced_env_from(path)
        .into_iter()
        .find(|(key, _)| key == TRACKIO_SERVER_URL)
        .is_some_and(|(_, previous)| {
            server(&split_trackio_write_token(&previous).0) != server(&url)
        });
    match embedded_token {
        Some(token) => write_synced_env_vars_at(
            path,
            &[(TRACKIO_SERVER_URL, &url), (TRACKIO_WRITE_TOKEN, &token)],
        ),
        None => {
            write_synced_env_vars_at(path, &[(TRACKIO_SERVER_URL, &url)])?;
            if moved {
                remove_synced_env_var_at(path, TRACKIO_WRITE_TOKEN)?;
            }
            Ok(())
        }
    }
}

/// The dashboard URL for a project on `server_url`.
///
/// Trackio's dashboard supports `project`, `metrics`, `sidebar`, `footer`,
/// `xmin`, `xmax`, `smoothing` and `accordion` as query parameters — there is no
/// per-run parameter, so this link is project-scoped by design.
pub fn trackio_dashboard_url(server_url: &str, project: Option<&str>) -> String {
    let base = server_url.trim().trim_end_matches('/');
    match project.map(str::trim).filter(|p| !p.is_empty()) {
        Some(project) => format!("{base}/?project={}", urlencoding::encode(project)),
        None => format!("{base}/"),
    }
}

/// The Trackio decision made for one launch. The write token is deliberately
/// absent so this value is safe to persist as run provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TrackioLaunch {
    Off,
    Injected {
        server_url: String,
        project: Option<String>,
        run: String,
    },
    Skipped {
        reason: String,
    },
}

/// Where an implicit TensorBoard log directory is safe for this backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TensorboardDefault {
    /// A local run can use ORX's own data directory.
    LocalData,
    /// Slurm/SSH-style jobs run from `~/.orx/runs/<run>/repo`.
    RemoteRunDirectory,
    /// The backend has no stable filesystem layout; require an explicit path.
    ExplicitOnly,
    /// The backend cannot preserve event files after the run completes.
    Unavailable(&'static str),
}

impl TensorboardDefault {
    /// Modal sandboxes mount no storage that outlives them.
    pub const MODAL: Self = Self::Unavailable(
        "Modal sandboxes are ephemeral and their event files are deleted at teardown",
    );
    /// Hugging Face Jobs mount only the read-only source volume.
    pub const HF_JOBS: Self = Self::Unavailable(
        "Hugging Face Jobs are ephemeral and their event files are deleted when the job ends",
    );

    /// SSH runs write under the host's `~/.orx`, which the recorded viewer
    /// command reads over SSH. A container run writes inside the container
    /// instead, where that host-side command cannot see the files.
    pub fn ssh(in_container: bool) -> Self {
        if in_container {
            Self::Unavailable(
                "an SSH container run keeps its event files inside the container, where the recorded viewer command cannot read them",
            )
        } else {
            Self::RemoteRunDirectory
        }
    }
}

impl TrackioLaunch {
    /// Persist only the injected public destination. Off/skipped launches have
    /// no tracking link, and the write token is never part of the record.
    pub fn descriptor(&self) -> Option<crate::jobs::TrackingDescriptor> {
        match self {
            Self::Injected {
                server_url,
                project,
                run,
            } => Some(crate::jobs::TrackingDescriptor::Trackio {
                server_url: server_url.clone(),
                project: project.clone(),
                run: run.clone(),
            }),
            Self::Off | Self::Skipped { .. } => None,
        }
    }
}

/// The Trackio variables a run should be launched with, plus the decision.
///
/// Empty when no server is configured, so an unconfigured project launches
/// exactly the environment it did before.
///
/// The three settings usually arrive in the run env anyway, via
/// [`list_synced_env`], but only with the values in the file. Resolving them
/// here means a variable exported into orx's own environment — which already
/// wins for the dashboard link and the reachability probe — reaches the run too,
/// instead of the run silently logging somewhere else. The address is the
/// normalised one, with any embedded write token moved to its own variable.
pub fn trackio_run_env_with_launch(
    run_id: &str,
    remote: bool,
) -> (Vec<(&'static str, String)>, TrackioLaunch) {
    trackio_run_env_from(&process_env, &synced_env_var, run_id, remote)
}

/// [`trackio_run_env_with_launch`] with its two sources passed in, as for
/// [`trackio_connection_from`].
fn trackio_run_env_from(
    process: &dyn Fn(&str) -> Option<String>,
    synced: &dyn Fn(&str) -> Option<String>,
    run_id: &str,
    remote: bool,
) -> (Vec<(&'static str, String)>, TrackioLaunch) {
    let Some((server_url, token)) = trackio_connection_from(process, synced) else {
        return (Vec::new(), TrackioLaunch::Off);
    };
    let project = process(TRACKIO_PROJECT).or_else(|| synced(TRACKIO_PROJECT));
    trackio_run_env_for(
        &server_url,
        project.as_deref(),
        token.as_deref(),
        run_id,
        remote,
    )
}

fn trackio_run_env_for(
    server_url: &str,
    project: Option<&str>,
    token: Option<&str>,
    run_id: &str,
    remote: bool,
) -> (Vec<(&'static str, String)>, TrackioLaunch) {
    if remote && is_local_only_trackio_url(server_url) {
        return (
            Vec::new(),
            TrackioLaunch::Skipped {
                reason: format!(
                    "configured server {server_url} is local-only and cannot be reached by a remote backend; submitting without Trackio"
                ),
            },
        );
    }
    // The rest of the Settings verdict's static checks. Reachability and write
    // access need a network probe, which a launch does not wait on; Test in
    // Settings and `orx trackio` cover those.
    if project.is_none_or(|project| project.trim().is_empty()) {
        return (
            Vec::new(),
            TrackioLaunch::Skipped {
                reason: format!("{TRACKIO_PROJECT} is not set; submitting without Trackio"),
            },
        );
    }
    let mut out = vec![
        (TRACKIO_SERVER_URL, server_url.to_string()),
        (TRACKIO_RUN, run_id.to_string()),
    ];
    if let Some(project) = project {
        out.push((TRACKIO_PROJECT, project.to_string()));
    }
    if let Some(token) = token {
        out.push((TRACKIO_WRITE_TOKEN, token.to_string()));
    }
    (
        out,
        TrackioLaunch::Injected {
            server_url: server_url.to_string(),
            project: project.map(str::to_string),
            run: run_id.to_string(),
        },
    )
}

pub(crate) fn is_local_only_trackio_url(server_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(server_url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host
        .trim_end_matches('.')
        .trim_start_matches('[')
        .trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback() || address.is_unspecified())
}

pub fn announce_trackio_launch(launch: &TrackioLaunch) {
    if let TrackioLaunch::Skipped { reason } = launch {
        eprintln!("Trackio: {reason}.");
    }
}

fn align_trackio_launch_with_env(
    launch: &mut TrackioLaunch,
    env: &std::collections::HashMap<String, String>,
) {
    if let TrackioLaunch::Injected { run, .. } = launch {
        if let Some(final_run) = env.get(TRACKIO_RUN) {
            *run = final_run.clone();
        }
    }
}

fn run_env_from(
    synced: Vec<(String, String)>,
    trackio: (Vec<(&'static str, String)>, TrackioLaunch),
    run_id: &str,
    project_id: &str,
    tensorboard: bool,
    remote: bool,
    tensorboard_default: TensorboardDefault,
) -> Result<(
    std::collections::HashMap<String, String>,
    Vec<crate::jobs::TrackingDescriptor>,
)> {
    let mut env = synced.into_iter().collect();
    let (vars, mut launch) = trackio;
    apply_trackio_vars(&mut env, &vars, &launch, remote);
    align_trackio_launch_with_env(&mut launch, &env);
    announce_trackio_launch(&launch);
    let mut tracking: Vec<_> = launch.descriptor().into_iter().collect();
    if let Some(record) = apply_tensorboard(
        &mut env,
        project_id,
        run_id,
        tensorboard,
        tensorboard_default,
    )? {
        tracking.push(record);
    }
    Ok((env, tracking))
}

/// Resolve the complete environment for a run. This is the only public path
/// from the synced env file into compute backends, so a new backend cannot
/// accidentally bypass the Trackio decision.
pub fn run_env(
    run_id: &str,
    project_id: &str,
    tensorboard: bool,
    remote: bool,
    tensorboard_default: TensorboardDefault,
) -> Result<(
    std::collections::HashMap<String, String>,
    Vec<crate::jobs::TrackingDescriptor>,
)> {
    run_env_from(
        list_synced_env(),
        trackio_run_env_with_launch(run_id, remote),
        run_id,
        project_id,
        tensorboard,
        remote,
        tensorboard_default,
    )
}

/// [`run_env`] with the synced file at `path` as its only source: neither
/// orx's own environment nor the real `~/.openresearch/env` is read.
#[cfg(test)]
pub(crate) fn run_env_from_file(
    path: &std::path::Path,
    run_id: &str,
    project_id: &str,
    tensorboard: bool,
    remote: bool,
) -> Result<(
    std::collections::HashMap<String, String>,
    Vec<crate::jobs::TrackingDescriptor>,
)> {
    let synced = list_synced_env_from(path);
    let saved = |key: &str| {
        synced
            .iter()
            .find(|(saved_key, _)| saved_key == key)
            .map(|(_, value)| value.clone())
    };
    let trackio = trackio_run_env_from(&|_: &str| None, &saved, run_id, remote);
    run_env_from(
        synced.clone(),
        trackio,
        run_id,
        project_id,
        tensorboard,
        remote,
        TensorboardDefault::RemoteRunDirectory,
    )
}

fn tracking_segment(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    cleaned.trim_matches('-').to_string()
}

/// Add TensorBoard's file-only launch contract. An explicitly synced
/// `TENSORBOARD_LOGDIR` opts in even without `--tracking tensorboard`; the CLI
/// flag supplies the stable project/run default.
pub fn apply_tensorboard(
    env: &mut std::collections::HashMap<String, String>,
    project_id: &str,
    run_id: &str,
    requested: bool,
    default: TensorboardDefault,
) -> Result<Option<crate::jobs::TrackingDescriptor>> {
    let configured = env.get(TENSORBOARD_LOGDIR).cloned();
    if let TensorboardDefault::Unavailable(reason) = default {
        if requested || configured.is_some() {
            return Err(crate::error::anyhow!(
                "TensorBoard is unavailable: {reason}."
            ));
        }
        return Ok(None);
    }
    if !requested && configured.is_none() {
        return Ok(None);
    }
    let project = tracking_segment(project_id);
    let run = tracking_segment(run_id);
    let (log_dir, root_log_dir) = match configured {
        Some(log_dir) => {
            if !std::path::Path::new(&log_dir).is_absolute() && !log_dir.starts_with("~/") {
                return Err(crate::error::anyhow!(
                    "TENSORBOARD_LOGDIR must be an absolute path or start with ~/ so its recorded viewer command is stable."
                ));
            }
            let root_log_dir = std::path::Path::new(&log_dir)
                .parent()
                .map(|path| path.to_string_lossy().into_owned())
                .filter(|path| !path.is_empty())
                .unwrap_or_else(|| log_dir.clone());
            (log_dir, root_log_dir)
        }
        None => match default {
            TensorboardDefault::LocalData => {
                let root = crate::store::data_dir().join("tensorboard").join(&project);
                let log_dir = root.join(&run).to_string_lossy().into_owned();
                (log_dir, root.to_string_lossy().into_owned())
            }
            TensorboardDefault::RemoteRunDirectory => (
                format!("../../../tensorboard/{project}/{run}"),
                format!("~/.orx/tensorboard/{project}"),
            ),
            TensorboardDefault::ExplicitOnly => {
                return Err(crate::error::anyhow!(
                    "TensorBoard was requested, but this backend has no stable run-directory default. Set TENSORBOARD_LOGDIR explicitly before submitting."
                ));
            }
            TensorboardDefault::Unavailable(_) => unreachable!("handled above"),
        },
    };
    env.insert(TENSORBOARD_LOGDIR.to_string(), log_dir.clone());
    Ok(Some(crate::jobs::TrackingDescriptor::Tensorboard {
        log_dir,
        root_log_dir,
        run: run_id.to_string(),
    }))
}

/// Reconstruct a delayed launch from its persisted public destinations. This
/// is used by provision-then-launch backends so a settings change cannot
/// repoint an existing run. A Trackio token is reattached only when the current
/// secret still belongs to the recorded server.
pub fn run_env_for_tracking(
    tracking: &[crate::jobs::TrackingDescriptor],
    remote: bool,
) -> std::collections::HashMap<String, String> {
    let mut env: std::collections::HashMap<_, _> = list_synced_env().into_iter().collect();
    for key in [
        TRACKIO_SERVER_URL,
        TRACKIO_PROJECT,
        TRACKIO_RUN,
        TRACKIO_WRITE_TOKEN,
        TENSORBOARD_LOGDIR,
    ] {
        env.remove(key);
    }
    for record in tracking {
        match record {
            crate::jobs::TrackingDescriptor::Trackio {
                server_url,
                project,
                run,
            } if !(remote && is_local_only_trackio_url(server_url)) => {
                env.insert(TRACKIO_SERVER_URL.to_string(), server_url.clone());
                env.insert(TRACKIO_RUN.to_string(), run.clone());
                if let Some(project) = project {
                    env.insert(TRACKIO_PROJECT.to_string(), project.clone());
                }
                if trackio_server_url().as_deref() == Some(server_url.as_str()) {
                    if let Some(token) = trackio_write_token() {
                        env.insert(TRACKIO_WRITE_TOKEN.to_string(), token);
                    }
                }
            }
            crate::jobs::TrackingDescriptor::Tensorboard { log_dir, .. } => {
                env.insert(TENSORBOARD_LOGDIR.to_string(), log_dir.clone());
            }
            _ => {}
        }
    }
    env
}

/// Merge the resolved Trackio variables from [`trackio_run_env_for`] into a
/// run's environment map, which already holds the synced file's values.
///
/// An explicit `TRACKIO_RUN` already in the map is left alone on a local run;
/// the rest are overwritten, because the resolved value is the one the link and
/// the probe used and a run pointed somewhere else is worse than no tracking at
/// all. The saved address and token are dropped before the merge: they are one
/// credential pair, so a resolved address with no token of its own must not
/// inherit the token saved for another server. A skipped connection reaches
/// the run in no form.
fn apply_trackio_vars(
    env: &mut std::collections::HashMap<String, String>,
    vars: &[(&'static str, String)],
    launch: &TrackioLaunch,
    remote: bool,
) {
    let stale: &[&str] = match launch {
        _ if remote => &[
            TRACKIO_SERVER_URL,
            TRACKIO_PROJECT,
            TRACKIO_RUN,
            TRACKIO_WRITE_TOKEN,
        ],
        TrackioLaunch::Skipped { .. } => &[
            TRACKIO_SERVER_URL,
            TRACKIO_PROJECT,
            TRACKIO_RUN,
            TRACKIO_WRITE_TOKEN,
        ],
        TrackioLaunch::Injected { .. } => &[TRACKIO_SERVER_URL, TRACKIO_WRITE_TOKEN],
        TrackioLaunch::Off => &[],
    };
    for key in stale {
        env.remove(*key);
    }
    for (key, value) in vars {
        if *key == TRACKIO_RUN {
            env.entry(key.to_string()).or_insert_with(|| value.clone());
        } else {
            env.insert(key.to_string(), value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_trackio_write_token_extracts_the_token_and_bares_the_url() {
        let (url, token) = split_trackio_write_token("http://127.0.0.1:7860/?write_token=s3cret");
        assert_eq!(url, "http://127.0.0.1:7860/");
        assert_eq!(token.as_deref(), Some("s3cret"));
    }

    #[test]
    fn split_trackio_write_token_drops_the_whole_query_string() {
        let (url, token) =
            split_trackio_write_token("https://trackio.example/?project=demo&write_token=abc%2F1");
        // Not `…/?project=demo`: every endpoint is built by appending to this
        // base, so a surviving query yields `…?project=demo/version`.
        assert_eq!(url, "https://trackio.example/");
        // Percent-encoded token values are decoded before storage.
        assert_eq!(token.as_deref(), Some("abc/1"));
    }

    #[test]
    fn split_trackio_write_token_leaves_a_plain_url_alone() {
        let (url, token) = split_trackio_write_token("  http://127.0.0.1:7860  ");
        assert_eq!(url, "http://127.0.0.1:7860");
        assert_eq!(token, None);
    }

    #[test]
    fn split_trackio_write_token_drops_an_empty_token_parameter() {
        let (url, token) = split_trackio_write_token("http://h:7860/?write_token=");
        assert_eq!(url, "http://h:7860/");
        assert_eq!(token, None);
    }

    #[test]
    fn trackio_dashboard_url_is_project_scoped_and_encodes_the_name() {
        assert_eq!(
            trackio_dashboard_url("http://127.0.0.1:7860/", Some("my exp")),
            "http://127.0.0.1:7860/?project=my%20exp"
        );
        assert_eq!(
            trackio_dashboard_url("http://127.0.0.1:7860", None),
            "http://127.0.0.1:7860/"
        );
        // A blank project is the same as none, not `?project=`.
        assert_eq!(
            trackio_dashboard_url("http://127.0.0.1:7860", Some("  ")),
            "http://127.0.0.1:7860/"
        );
    }

    #[test]
    fn a_connection_pairs_its_url_with_its_own_source_token() {
        // A standalone token from the same source is the more explicit of the
        // two, so it wins over one embedded in that source's URL.
        assert_eq!(
            pair_trackio_connection("http://h:7860/?write_token=embedded", Some("own".into())),
            ("http://h:7860/".to_string(), Some("own".to_string()))
        );
        // With no standalone token, the URL's own token is the connection's.
        assert_eq!(
            pair_trackio_connection("http://h:7860/?write_token=embedded", None),
            ("http://h:7860/".to_string(), Some("embedded".to_string()))
        );
        // A bare URL and no token is an unauthenticated connection, not an
        // opportunity to reach for some other source's credential.
        assert_eq!(
            pair_trackio_connection("http://h:7860", None),
            ("http://h:7860".to_string(), None)
        );
    }

    fn configured() -> Vec<(&'static str, String)> {
        vec![
            (TRACKIO_SERVER_URL, "http://127.0.0.1:7860".to_string()),
            (TRACKIO_RUN, "run-1".to_string()),
            (TRACKIO_PROJECT, "demo".to_string()),
            (TRACKIO_WRITE_TOKEN, "tok".to_string()),
        ]
    }

    fn configured_launch() -> TrackioLaunch {
        TrackioLaunch::Injected {
            server_url: "http://127.0.0.1:7860".to_string(),
            project: Some("demo".to_string()),
            run: "run-1".to_string(),
        }
    }

    #[test]
    fn an_unconfigured_run_env_is_left_exactly_as_it_was() {
        let mut env =
            std::collections::HashMap::from([("HF_TOKEN".to_string(), "hf_x".to_string())]);
        apply_trackio_vars(&mut env, &[], &TrackioLaunch::Off, false);
        assert_eq!(
            env,
            std::collections::HashMap::from([("HF_TOKEN".to_string(), "hf_x".to_string())])
        );
    }

    #[test]
    fn a_configured_run_env_gets_every_resolved_variable() {
        let mut env = std::collections::HashMap::new();
        apply_trackio_vars(&mut env, &configured(), &configured_launch(), false);
        assert_eq!(
            env,
            std::collections::HashMap::from([
                (
                    TRACKIO_SERVER_URL.to_string(),
                    "http://127.0.0.1:7860".to_string()
                ),
                (TRACKIO_RUN.to_string(), "run-1".to_string()),
                (TRACKIO_PROJECT.to_string(), "demo".to_string()),
                (TRACKIO_WRITE_TOKEN.to_string(), "tok".to_string()),
            ])
        );
    }

    #[test]
    fn trackio_umbrella_tensorboard_defaults_to_a_project_root_and_run_directory() {
        let mut env = std::collections::HashMap::new();
        let record = apply_tensorboard(
            &mut env,
            "project 1",
            "run/1",
            true,
            TensorboardDefault::RemoteRunDirectory,
        )
        .expect("TensorBoard default should resolve")
        .unwrap();
        assert_eq!(
            env,
            std::collections::HashMap::from([(
                TENSORBOARD_LOGDIR.to_string(),
                "../../../tensorboard/project-1/run-1".to_string(),
            )])
        );
        assert_eq!(
            record,
            crate::jobs::TrackingDescriptor::Tensorboard {
                log_dir: "../../../tensorboard/project-1/run-1".to_string(),
                root_log_dir: "~/.orx/tensorboard/project-1".to_string(),
                run: "run/1".to_string(),
            }
        );
    }

    #[test]
    fn trackio_umbrella_tensorboard_preserves_an_explicit_log_directory() {
        let mut env = std::collections::HashMap::from([(
            TENSORBOARD_LOGDIR.to_string(),
            "/shared/tensorboard/demo/run-1".to_string(),
        )]);
        let record = apply_tensorboard(
            &mut env,
            "project",
            "run-1",
            false,
            TensorboardDefault::ExplicitOnly,
        )
        .expect("explicit TensorBoard path should resolve")
        .unwrap();
        assert_eq!(
            record,
            crate::jobs::TrackingDescriptor::Tensorboard {
                log_dir: "/shared/tensorboard/demo/run-1".to_string(),
                root_log_dir: "/shared/tensorboard/demo".to_string(),
                run: "run-1".to_string(),
            }
        );
    }

    #[test]
    fn trackio_umbrella_tensorboard_rejects_a_relative_explicit_log_directory() {
        let mut env = std::collections::HashMap::from([(
            TENSORBOARD_LOGDIR.to_string(),
            "logs/tensorboard/run-1".to_string(),
        )]);
        let error = apply_tensorboard(
            &mut env,
            "project",
            "run-1",
            false,
            TensorboardDefault::ExplicitOnly,
        )
        .expect_err("relative paths cannot produce stable viewer commands");
        assert!(error.to_string().contains("must be an absolute path"));
    }

    #[test]
    fn resolved_settings_win_over_the_synced_file_but_an_explicit_run_name_does_not() {
        // The synced file's values are already in the map when this runs. The
        // address, project and token are replaced by the resolved ones — the
        // values the dashboard link and the probe used — so a run cannot end up
        // logging somewhere other than where the UI points. TRACKIO_RUN is the
        // exception: a name the user set deliberately is theirs.
        let mut env = std::collections::HashMap::from([
            (
                TRACKIO_SERVER_URL.to_string(),
                "http://stale:7860".to_string(),
            ),
            (TRACKIO_PROJECT.to_string(), "stale".to_string()),
            (TRACKIO_RUN.to_string(), "mine".to_string()),
        ]);
        apply_trackio_vars(&mut env, &configured(), &configured_launch(), false);
        assert_eq!(
            env,
            std::collections::HashMap::from([
                (
                    TRACKIO_SERVER_URL.to_string(),
                    "http://127.0.0.1:7860".to_string()
                ),
                (TRACKIO_RUN.to_string(), "mine".to_string()),
                (TRACKIO_PROJECT.to_string(), "demo".to_string()),
                (TRACKIO_WRITE_TOKEN.to_string(), "tok".to_string()),
            ])
        );
    }

    #[test]
    fn trackio_umbrella_provenance_uses_the_final_explicit_run_name() {
        let mut env =
            std::collections::HashMap::from([(TRACKIO_RUN.to_string(), "mine".to_string())]);
        let (vars, mut launch) = trackio_run_env_for(
            "https://trackio.example",
            Some("demo"),
            Some("tok"),
            "generated",
            false,
        );
        apply_trackio_vars(&mut env, &vars, &launch, false);
        align_trackio_launch_with_env(&mut launch, &env);
        assert_eq!(env.get(TRACKIO_RUN).map(String::as_str), Some("mine"));
        assert_eq!(
            launch.descriptor(),
            Some(crate::jobs::TrackingDescriptor::Trackio {
                server_url: "https://trackio.example".to_string(),
                project: Some("demo".to_string()),
                run: "mine".to_string(),
            })
        );
    }

    #[test]
    fn trackio_umbrella_tensorboard_requires_an_explicit_path_without_a_stable_layout() {
        let mut env = std::collections::HashMap::new();
        let error = apply_tensorboard(
            &mut env,
            "project",
            "run-1",
            true,
            TensorboardDefault::ExplicitOnly,
        )
        .expect_err("an explicit-only backend must reject a missing path");
        assert!(error
            .to_string()
            .contains("Set TENSORBOARD_LOGDIR explicitly"));
        assert!(!env.contains_key(TENSORBOARD_LOGDIR));
    }

    #[test]
    fn trackio_umbrella_tensorboard_rejects_ephemeral_backends() {
        let mut env = std::collections::HashMap::new();
        let error = apply_tensorboard(
            &mut env,
            "project",
            "run-1",
            true,
            TensorboardDefault::Unavailable("event files are deleted at teardown"),
        )
        .expect_err("ephemeral backend must reject TensorBoard");
        assert_eq!(
            error.to_string(),
            "TensorBoard is unavailable: event files are deleted at teardown."
        );
        assert!(!env.contains_key(TENSORBOARD_LOGDIR));
    }

    #[test]
    fn a_local_only_trackio_connection_is_not_sent_to_a_remote_run() {
        for url in [
            "http://127.0.0.1:7860",
            "http://[::1]:7860",
            "http://0.0.0.0:7860",
            "http://[::]:7860",
        ] {
            let (vars, launch) = trackio_run_env_for(url, Some("demo"), Some("tok"), "run-1", true);
            assert!(vars.is_empty());
            assert!(matches!(launch, TrackioLaunch::Skipped { .. }));
        }
    }

    #[test]
    fn a_remote_loopback_run_removes_preloaded_trackio_variables() {
        let mut env = std::collections::HashMap::from([
            ("HF_TOKEN".to_string(), "hf_x".to_string()),
            (
                TRACKIO_SERVER_URL.to_string(),
                "http://127.0.0.1:7860".to_string(),
            ),
            (TRACKIO_PROJECT.to_string(), "stale".to_string()),
            (TRACKIO_RUN.to_string(), "stale-run".to_string()),
            (TRACKIO_WRITE_TOKEN.to_string(), "stale-token".to_string()),
        ]);
        let (vars, launch) = trackio_run_env_for(
            "http://127.0.0.1:7860",
            Some("stale"),
            Some("stale-token"),
            "run-1",
            true,
        );
        apply_trackio_vars(&mut env, &vars, &launch, true);
        assert_eq!(
            env,
            std::collections::HashMap::from([("HF_TOKEN".to_string(), "hf_x".to_string())])
        );
    }

    #[test]
    fn a_local_run_never_sends_the_saved_token_to_an_overriding_server() {
        // The synced file holds server A and its token; the process environment
        // overrides the address with server B and supplies no token for it.
        let mut env = std::collections::HashMap::from([
            ("HF_TOKEN".to_string(), "hf_x".to_string()),
            (
                TRACKIO_SERVER_URL.to_string(),
                "https://a.example".to_string(),
            ),
            (TRACKIO_WRITE_TOKEN.to_string(), "token-a".to_string()),
        ]);
        let (vars, launch) =
            trackio_run_env_for("https://b.example", Some("demo"), None, "run-1", false);
        apply_trackio_vars(&mut env, &vars, &launch, false);
        assert_eq!(
            env,
            std::collections::HashMap::from([
                ("HF_TOKEN".to_string(), "hf_x".to_string()),
                (
                    TRACKIO_SERVER_URL.to_string(),
                    "https://b.example".to_string()
                ),
                (TRACKIO_PROJECT.to_string(), "demo".to_string()),
                (TRACKIO_RUN.to_string(), "run-1".to_string()),
            ])
        );
    }

    #[test]
    fn a_trackio_connection_without_a_project_is_skipped_and_kept_out_of_a_local_run() {
        for project in [None, Some("  ")] {
            let mut env = std::collections::HashMap::from([
                ("HF_TOKEN".to_string(), "hf_x".to_string()),
                (
                    TRACKIO_SERVER_URL.to_string(),
                    "https://trackio.example".to_string(),
                ),
                (TRACKIO_WRITE_TOKEN.to_string(), "tok".to_string()),
            ]);
            let (vars, launch) = trackio_run_env_for(
                "https://trackio.example",
                project,
                Some("tok"),
                "run-1",
                false,
            );
            apply_trackio_vars(&mut env, &vars, &launch, false);
            assert_eq!(
                launch,
                TrackioLaunch::Skipped {
                    reason: "TRACKIO_PROJECT is not set; submitting without Trackio".to_string(),
                }
            );
            assert_eq!(launch.descriptor(), None);
            assert_eq!(
                env,
                std::collections::HashMap::from([("HF_TOKEN".to_string(), "hf_x".to_string())])
            );
        }
    }

    #[test]
    fn trackio_umbrella_tensorboard_rejects_backends_whose_event_files_the_viewer_cannot_reach() {
        assert_eq!(
            TensorboardDefault::ssh(false),
            TensorboardDefault::RemoteRunDirectory
        );
        for default in [
            TensorboardDefault::MODAL,
            TensorboardDefault::HF_JOBS,
            TensorboardDefault::ssh(true),
        ] {
            // An absolute synced path is the opt-in an explicit-only backend
            // would accept; these backends must refuse it as well as the flag.
            for (requested, configured) in [(false, true), (true, false)] {
                let before: std::collections::HashMap<String, String> = configured
                    .then(|| {
                        (
                            TENSORBOARD_LOGDIR.to_string(),
                            "/shared/tensorboard/demo/run-1".to_string(),
                        )
                    })
                    .into_iter()
                    .collect();
                let mut env = before.clone();
                let error = apply_tensorboard(&mut env, "project", "run-1", requested, default)
                    .expect_err("TensorBoard must be refused on this backend");
                assert!(error
                    .to_string()
                    .starts_with("TensorBoard is unavailable: "));
                assert_eq!(env, before);
            }
        }
    }

    fn synced_env_file(contents: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("orx-trackio-save-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("env");
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn saved_after(contents: &str, raw_url: &str) -> Vec<(String, String)> {
        let path = synced_env_file(contents);
        save_trackio_server_url_at(&path, raw_url).unwrap();
        let saved = list_synced_env_from(&path);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        saved
    }

    fn pairs(values: &[(&str, &str)]) -> Vec<(String, String)> {
        values
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn saving_a_different_trackio_server_without_a_token_drops_the_old_token() {
        assert_eq!(
            saved_after(
                "export TRACKIO_SERVER_URL='https://a.example'\n\
                 export TRACKIO_WRITE_TOKEN='token-a'\n\
                 export TRACKIO_PROJECT='demo'\n",
                "https://b.example",
            ),
            pairs(&[
                (TRACKIO_SERVER_URL, "https://b.example"),
                (TRACKIO_PROJECT, "demo"),
            ])
        );
    }

    #[test]
    fn saving_the_same_trackio_server_keeps_its_token() {
        assert_eq!(
            saved_after(
                "export TRACKIO_SERVER_URL='https://a.example'\n\
                 export TRACKIO_WRITE_TOKEN='token-a'\n",
                "https://a.example/",
            ),
            pairs(&[
                (TRACKIO_SERVER_URL, "https://a.example/"),
                (TRACKIO_WRITE_TOKEN, "token-a"),
            ])
        );
    }

    #[test]
    fn saving_a_first_trackio_server_keeps_a_token_saved_before_it() {
        assert_eq!(
            saved_after(
                "export TRACKIO_WRITE_TOKEN='token-a'\n",
                "https://a.example"
            ),
            pairs(&[
                (TRACKIO_WRITE_TOKEN, "token-a"),
                (TRACKIO_SERVER_URL, "https://a.example"),
            ])
        );
    }

    #[test]
    fn saving_a_trackio_server_with_an_embedded_token_replaces_the_saved_token() {
        assert_eq!(
            saved_after(
                "export TRACKIO_SERVER_URL='https://a.example'\n\
                 export TRACKIO_WRITE_TOKEN='token-a'\n",
                "https://b.example/?write_token=token-b",
            ),
            pairs(&[
                (TRACKIO_SERVER_URL, "https://b.example/"),
                (TRACKIO_WRITE_TOKEN, "token-b"),
            ])
        );
    }
}
