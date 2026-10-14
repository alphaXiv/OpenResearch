//! Shared compute settings operations for the dashboard and standalone CLI.

use super::*;

// --- HF token settings ------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HfSettings {
    configured: bool,
    source: Option<&'static str>,
    masked_token: Option<String>,
    validation_status: &'static str,
    validation_error: Option<String>,
    username: Option<String>,
    jobs_write: Option<bool>,
}

/// Never the full token: first 3 chars + ellipsis + last 4.
pub(super) fn mask_token(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() < 8 {
        return "…".to_string();
    }
    format!(
        "{}…{}",
        chars[..3].iter().collect::<String>(),
        chars[chars.len() - 4..].iter().collect::<String>()
    )
}

/// Re-resolve the token and check it against whoami-v2. Uncached — cheap, and
/// the UI calls it rarely.
pub(super) async fn hf_token_status() -> HfSettings {
    use crate::jobs::huggingface::{self, TokenSource};
    let Ok((token, source)) = huggingface::resolve_token_with_source() else {
        return HfSettings {
            configured: false,
            source: None,
            masked_token: None,
            validation_status: "missing",
            validation_error: None,
            username: None,
            jobs_write: None,
        };
    };
    let source = match source {
        TokenSource::Env => "env",
        TokenSource::OpenresearchEnv => "openresearchEnv",
        TokenSource::HfCache => "hfCache",
    };
    match huggingface::whoami_details(&token).await {
        Ok(details) => HfSettings {
            configured: true,
            source: Some(source),
            masked_token: Some(mask_token(&token)),
            validation_status: "valid",
            validation_error: None,
            username: Some(details.name),
            jobs_write: details.jobs_write,
        },
        Err(error) => {
            let validation_status = if error
                .downcast_ref::<reqwest::Error>()
                .is_some_and(|error| error.status() == Some(reqwest::StatusCode::UNAUTHORIZED))
            {
                "invalid"
            } else {
                "unreachable"
            };
            HfSettings {
                configured: true,
                source: Some(source),
                masked_token: Some(mask_token(&token)),
                validation_status,
                validation_error: Some(error.to_string()),
                username: None,
                jobs_write: None,
            }
        }
    }
}

pub(super) async fn hf_settings() -> Json<Value> {
    Json(json!(hf_token_status().await))
}

pub(super) async fn tinker_settings() -> ApiResult {
    use crate::jobs::tinker;
    let Ok((key, source)) = tinker::resolve_api_key_with_source() else {
        return Ok(Json(
            json!({ "validationStatus": tinker::KeyStatus::Missing, "processEnv": false, "maskedKey": null }),
        ));
    };
    let status = tinker::validate_api_key(&key).await.map_err(bad_request)?;
    Ok(Json(json!({
        "validationStatus": status,
        "processEnv": source == tinker::ApiKeySource::Env,
        "maskedKey": mask_token(&key),
    })))
}

#[derive(Deserialize)]
pub(super) struct SetTinkerKeyReq {
    key: String,
}

pub(super) async fn set_tinker_key(Json(req): Json<SetTinkerKeyReq>) -> ApiResult {
    use crate::jobs::tinker;
    let key = req.key.trim().to_string();
    if key.is_empty() {
        return Err(bad_request("API key is required"));
    }
    let status = tinker::validate_api_key(&key).await.map_err(bad_request)?;
    if status == tinker::KeyStatus::Invalid {
        return Err(bad_request(
            "Invalid Tinker API key. The existing key was not changed.",
        ));
    }
    let process_key = std::env::var(tinker::API_KEY_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty());
    let effective_status = if let Some(ref process_key) = process_key {
        if process_key.trim() == key {
            status
        } else {
            tinker::validate_api_key(process_key.trim())
                .await
                .map_err(bad_request)?
        }
    } else {
        status
    };
    let masked_key = mask_token(process_key.as_deref().map(str::trim).unwrap_or(&key));
    tokio::task::spawn_blocking(move || {
        crate::config::write_synced_env_var(tinker::API_KEY_ENV, &key)
    })
    .await
    .map_err(|e| anyhow!("env write task failed: {e}"))??;
    Ok(Json(
        json!({ "validationStatus": effective_status, "processEnv": process_key.is_some(), "maskedKey": masked_key }),
    ))
}

#[derive(Deserialize)]
pub(super) struct SetHfTokenReq {
    token: String,
}

pub(super) async fn set_hf_token(Json(req): Json<SetHfTokenReq>) -> ApiResult {
    let token = req.token.trim().to_string();
    if token.is_empty() {
        return Err(bad_request("token is required"));
    }
    crate::jobs::huggingface::whoami_details(&token)
        .await
        .map_err(bad_request)?;
    tokio::task::spawn_blocking(move || crate::config::write_synced_env_var("HF_TOKEN", &token))
        .await
        .map_err(|e| anyhow!("env write task failed: {e}"))??;
    // Freshly re-resolved: if HF_TOKEN is set in this process env, env still
    // wins over the file — source says "env" and the UI explains it.
    Ok(Json(json!(hf_token_status().await)))
}

// --- modal settings -----------------------------------------------------------

use crate::jobs::modal;

pub(super) fn modal_settings_json() -> crate::error::Result<Value> {
    let token = modal::resolve_token()?;
    Ok(json!({
        "tokenConfigured": token.is_some(),
        "tokenSource": token.as_ref().map(|token| token.source),
        "maskedTokenId": token.as_ref().map(|token| mask_token(&token.id)),
        "maskedTokenSecret": token.as_ref().map(|token| mask_token(&token.secret)),
        "processEnv": std::env::var_os("MODAL_TOKEN_ID").is_some() || std::env::var_os("MODAL_TOKEN_SECRET").is_some(),
    }))
}

pub(super) async fn modal_settings() -> ApiResult {
    Ok(Json(modal_settings_json().map_err(bad_request)?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetModalTokenReq {
    token_id: String,
    token_secret: String,
}

pub(super) async fn set_modal_token(Json(req): Json<SetModalTokenReq>) -> ApiResult {
    let id = req.token_id.trim().to_string();
    let secret = req.token_secret.trim().to_string();
    if id.is_empty() || secret.is_empty() {
        return Err(bad_request(
            "Both the Modal token ID and secret are required.",
        ));
    }
    if std::env::var_os("MODAL_TOKEN_ID").is_some()
        || std::env::var_os("MODAL_TOKEN_SECRET").is_some()
    {
        return Err(bad_request("Modal credentials are overridden by the process environment. Remove those overrides before replacing the token here."));
    }
    // Modal loads and validates its profile file even when credentials come from env.
    modal::resolve_token().map_err(bad_request)?;
    let masked_id = mask_token(&id);
    let masked_secret = mask_token(&secret);
    tokio::task::spawn_blocking(move || {
        crate::config::write_synced_env_vars(&[
            ("MODAL_TOKEN_ID", &id),
            ("MODAL_TOKEN_SECRET", &secret),
        ])
    })
    .await
    .map_err(|e| anyhow!("env write task failed: {e}"))??;
    Ok(Json(json!({
        "tokenConfigured": true, "tokenSource": "syncedEnv", "processEnv": false,
        "maskedTokenId": masked_id, "maskedTokenSecret": masked_secret,
    })))
}

// --- kubernetes settings ------------------------------------------------------

use crate::jobs::kubernetes as k8s;

/// One payload powers the whole settings card: stored config plus live
/// cluster health. Contexts come from the local kubeconfig. Resource shapes
/// live in each experiment's committed manifest, not in settings.
pub(super) async fn k8s_settings_json() -> Value {
    let settings = k8s::load_settings().ok().flatten();
    let configured = settings.is_some();
    let settings = settings.unwrap_or_default();
    let (contexts, current) = k8s::list_contexts().await.unwrap_or((Vec::new(), None));
    let preflight = k8s::preflight(settings.context.as_deref(), &settings.namespace).await;
    json!({
        "configured": configured,
        "contexts": contexts,
        "currentContext": current,
        "context": settings.context,
        "namespace": settings.namespace,
        "preflight": preflight,
    })
}

pub(super) async fn k8s_settings() -> ApiResult {
    Ok(Json(k8s_settings_json().await))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetK8sSettingsReq {
    /// `None` leaves the field alone; `Some("")` clears it (kubectl default).
    context: Option<String>,
    namespace: Option<String>,
}

pub(super) async fn set_k8s_settings(Json(req): Json<SetK8sSettingsReq>) -> ApiResult {
    let mut settings = k8s::load_settings()?.unwrap_or_default();
    if let Some(ctx) = req.context {
        settings.context = Some(ctx.trim().to_string()).filter(|c| !c.is_empty());
    }
    if let Some(ns) = req.namespace {
        let ns = ns.trim().to_string();
        settings.namespace = if ns.is_empty() {
            "default".to_string()
        } else {
            ns
        };
    }
    k8s::save_settings(&settings)?;
    Ok(Json(k8s_settings_json().await))
}

/// Concrete Host entries from `~/.ssh/config` and the files it `Include`s
/// (wildcard patterns skipped) — read-only groundwork for an SSH compute
/// backend. No keys are read.
pub(super) fn list_ssh_hosts() -> Vec<Value> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    ssh_hosts_in(&home)
}

/// OpenSSH's `READCONF_MAX_DEPTH`.
const SSH_INCLUDE_MAX_DEPTH: usize = 16;

fn ssh_hosts_in(home: &std::path::Path) -> Vec<Value> {
    let mut reader = SshHostReader {
        home,
        hosts: Vec::new(),
        current: Vec::new(),
        stack: Vec::new(),
    };
    reader.read_file(&home.join(".ssh").join("config"));
    reader.hosts
}

struct SshHostReader<'a> {
    home: &'a std::path::Path,
    hosts: Vec<Value>,
    // Indices into `hosts` for the Host block currently being filled.
    current: Vec<usize>,
    // Files being read, outermost first, to stop Include cycles.
    stack: Vec<std::path::PathBuf>,
}

impl SshHostReader<'_> {
    /// Unreadable or missing files are skipped, like ssh does for Include.
    fn read_file(&mut self, path: &std::path::Path) {
        let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if self.stack.len() > SSH_INCLUDE_MAX_DEPTH || self.stack.contains(&key) {
            return;
        }
        let Ok(raw) = std::fs::read_to_string(path) else {
            return;
        };
        self.stack.push(key);
        // Each file starts in the block that included it, and ssh restores
        // that block when the file ends.
        let enclosing = self.current.clone();
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let (key, value) = match line.split_once([' ', '\t', '=']) {
                Some((k, v)) => (
                    k.trim().to_ascii_lowercase(),
                    v.trim().trim_start_matches('=').trim(),
                ),
                None => continue,
            };
            let args = ssh_config_args(value);
            if args.is_empty() {
                continue;
            }
            match key.as_str() {
                "host" => {
                    let current = args
                        .into_iter()
                        .filter(|name| !name.contains(['*', '?', '!']))
                        .map(|name| self.host_index(&name))
                        .collect();
                    self.current = current;
                    continue;
                }
                // Match criteria aren't evaluated; its options belong to no listed host.
                "match" => {
                    self.current.clear();
                    continue;
                }
                "include" => {
                    for pattern in args {
                        for file in expand_ssh_include(self.home, &pattern) {
                            self.read_file(&file);
                        }
                    }
                    continue;
                }
                _ => {}
            }
            let field = match key.as_str() {
                "hostname" => "hostname",
                "user" => "user",
                "port" => "port",
                "identityfile" => "identityFile",
                _ => continue,
            };
            let value = args.join(" ");
            for &i in &self.current {
                // First value wins, like ssh itself.
                if self.hosts[i].get(field).is_none() {
                    self.hosts[i][field] = json!(value);
                }
            }
        }
        self.current = enclosing;
        self.stack.pop();
    }

    /// Repeated Host blocks for one alias merge into a single entry.
    fn host_index(&mut self, name: &str) -> usize {
        if let Some(i) = self
            .hosts
            .iter()
            .position(|h| h.get("host").and_then(Value::as_str) == Some(name))
        {
            return i;
        }
        self.hosts.push(json!({ "host": name }));
        self.hosts.len() - 1
    }
}

/// OpenSSH's argument quoting, escapes and word-start comments.
fn ssh_config_args(value: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut arg = String::new();
    let mut quote = None;
    let mut started = false;
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' if quote.is_none() => {
                if started {
                    args.push(std::mem::take(&mut arg));
                    started = false;
                }
            }
            '#' if !started => break,
            '\\' if chars.peek().is_some_and(|&next| {
                matches!(next, '\\' | '\'' | '"') || (quote.is_none() && next == ' ')
            }) =>
            {
                arg.push(chars.next().unwrap());
                started = true;
            }
            '\'' | '"' if quote.is_none() => {
                quote = Some(c);
                started = true;
            }
            c if quote == Some(c) => quote = None,
            c => {
                arg.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Vec::new();
    }
    if started {
        args.push(arg);
    }
    args
}

/// Relative Include paths are under `~/.ssh`, even in nested files.
fn expand_ssh_include(home: &std::path::Path, pattern: &str) -> Vec<std::path::PathBuf> {
    use std::path::Path;
    let pattern = if let Some(rest) = pattern.strip_prefix('~') {
        let (user, rest) = rest.split_once(['/', '\\']).unwrap_or((rest, ""));
        let home = if user.is_empty() {
            home.to_path_buf()
        } else if let Some(home) = ssh_user_home(user) {
            home
        } else {
            return Vec::new();
        };
        let home = glob::Pattern::escape(&home.to_string_lossy());
        if rest.is_empty() {
            home
        } else {
            format!("{home}/{rest}")
        }
    } else if Path::new(pattern).is_absolute() || pattern.starts_with(['/', '\\']) {
        pattern.to_string()
    } else {
        format!(
            "{}/{pattern}",
            glob::Pattern::escape(&home.join(".ssh").to_string_lossy())
        )
    };
    let pattern = ssh_include_glob(&pattern);
    let Ok(matcher) = glob::Pattern::new(&pattern) else {
        return Vec::new();
    };
    let options = glob::MatchOptions {
        case_sensitive: !cfg!(windows),
        require_literal_separator: true,
        require_literal_leading_dot: true,
    };
    // glob's directory filter drops even explicit dot patterns and panics on
    // non-UTF-8 names. Apply the literal-dot rule to safe full-path matches.
    let scan_options = glob::MatchOptions {
        require_literal_leading_dot: false,
        ..options
    };
    glob::glob_with(&pattern, scan_options)
        .map(|paths| {
            paths
                .flatten()
                .filter(|path| matcher.matches_path_with(path, options))
                .collect()
        })
        .unwrap_or_default()
}

fn ssh_include_glob(pattern: &str) -> String {
    let mut result = String::new();
    let mut in_class = false;
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && !cfg!(windows) && !in_class {
            let Some(literal) = chars.next() else {
                result.push(c);
                break;
            };
            result.push_str(&glob::Pattern::escape(&literal.to_string()));
            continue;
        }
        result.push(c);
        match c {
            '[' => in_class = true,
            ']' => in_class = false,
            // OpenSSH glob has no recursive ** operator.
            '*' if !in_class => {
                while chars.peek() == Some(&'*') {
                    chars.next();
                }
            }
            _ => {}
        }
    }
    result
}

#[cfg(unix)]
fn ssh_user_home(user: &str) -> Option<std::path::PathBuf> {
    use std::ffi::{CStr, CString, OsStr};
    use std::os::unix::ffi::OsStrExt;

    let user = CString::new(user).ok()?;
    let mut buffer = vec![0u8; 16384];
    loop {
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        // SAFETY: the input and writable buffers remain valid throughout the call.
        let error = unsafe {
            libc::getpwnam_r(
                user.as_ptr(),
                entry.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if error == libc::ERANGE && buffer.len() < 1024 * 1024 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if error != 0 || result.is_null() {
            return None;
        }
        // SAFETY: success initializes the entry and its string lives in buffer.
        let directory = unsafe { CStr::from_ptr(entry.assume_init().pw_dir) };
        return Some(OsStr::from_bytes(directory.to_bytes()).into());
    }
}

#[cfg(windows)]
fn ssh_user_home(user: &str) -> Option<std::path::PathBuf> {
    use std::os::windows::process::CommandExt;

    // OpenSSH resolves the account SID's ProfileImagePath, not a guessed folder.
    let script = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$sid = [Security.Principal.NTAccount]::new($env:ORX_SSH_CONFIG_USER).Translate([Security.Principal.SecurityIdentifier]).Value
$path = [Microsoft.Win32.Registry]::GetValue('HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\' + $sid, 'ProfileImagePath', $null)
if (!$path) { $path = $env:SystemRoot }
[Environment]::ExpandEnvironmentVariables($path)
"#;
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("ORX_SSH_CONFIG_USER", user)
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = std::str::from_utf8(&output.stdout).ok()?.trim();
    (!path.is_empty()).then(|| path.into())
}

pub(super) async fn ssh_settings() -> ApiResult {
    tokio::task::spawn_blocking(|| {
        let mut hosts = list_ssh_hosts();
        let settings = crate::config::ssh_settings()?;
        for host in &mut hosts {
            let options = host
                .get("host")
                .and_then(Value::as_str)
                .and_then(|host| settings.hosts.get(host))
                .cloned()
                .unwrap_or_default();
            host["container"] = json!(options.container);
        }
        // Best-effort, like the preflight write: a store hiccup shouldn't take
        // out the host listing — hosts just render as never tested.
        let tests: HashMap<String, SshHostTest> = Store::open()
            .and_then(|s| s.list_ssh_host_tests())
            .unwrap_or_else(|e| {
                eprintln!("orx up: could not load ssh test history: {e}");
                Vec::new()
            })
            .into_iter()
            .map(|t| (t.host.clone(), t))
            .collect();
        for h in &mut hosts {
            let Some(t) = h
                .get("host")
                .and_then(Value::as_str)
                .and_then(|a| tests.get(a))
            else {
                continue;
            };
            h["lastTest"] = json!(t);
        }
        Ok(Json(
            json!({ "hosts": hosts, "defaultHost": settings.default_host }),
        ))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("ssh task failed: {e}")))?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SaveSshSettingsReq {
    host: String,
    container: Option<String>,
}

pub(super) async fn save_ssh_settings(Json(req): Json<SaveSshSettingsReq>) -> ApiResult {
    let host = req.host.trim().to_string();
    require_configured_ssh_host(&host).await?;
    let options = crate::config::SshHostSettings {
        container: req.container,
    };
    crate::jobs::ssh::validate_host_options(&options).map_err(bad_request)?;
    blocking_api(move || {
        crate::config::set_ssh_host(host, options)?;
        Ok(Json(json!({"ok": true})))
    })
    .await
}

#[derive(Deserialize)]
pub(super) struct SaveSshDefaultReq {
    host: Option<String>,
}

pub(super) async fn save_ssh_default(Json(req): Json<SaveSshDefaultReq>) -> ApiResult {
    let host = req.host.map(|host| host.trim().to_string());
    if let Some(host) = &host {
        require_configured_ssh_host(host).await?;
    }
    blocking_api(move || {
        crate::config::set_ssh_default(host)?;
        Ok(Json(json!({"ok": true})))
    })
    .await
}

pub(super) fn ssh_config_path() -> Result<std::path::PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow!("no home directory"))?
        .join(".ssh")
        .join("config"))
}

pub(super) async fn ssh_config() -> ApiResult {
    blocking_api(move || {
        let path = ssh_config_path()?;
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(ApiError::from(anyhow!(
                    "could not read SSH config: {error}"
                )))
            }
        };
        Ok(Json(json!({ "path": "~/.ssh/config", "content": content })))
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SaveSshConfigReq {
    content: String,
    previous_content: String,
}

pub(super) async fn save_ssh_config(Json(req): Json<SaveSshConfigReq>) -> ApiResult {
    blocking_api(move || {
        if req.content.len() as u64 > FILE_WRITE_LIMIT {
            return Err(bad_request("SSH config is too large to save"));
        }
        let path = ssh_config_path()?;
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("SSH config has no parent"))?;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(parent).map_err(anyhow::Error::from)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(parent.join(".orx-config.lock"))
            .map_err(anyhow::Error::from)?;
        let mut lock = fd_lock::RwLock::new(file);
        let _guard = lock.write().map_err(anyhow::Error::from)?;
        let current = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(ApiError::from(anyhow!(
                    "could not read SSH config: {error}"
                )))
            }
        };
        if current != req.previous_content {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "SSH config changed on disk. Reload it before saving.".into(),
            ));
        }

        crate::local::git::atomic_write_with_mode(&path, req.content.as_bytes(), Some(0o600))
            .map_err(|error| ApiError::from(anyhow!("could not save SSH config: {error}")))?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

#[derive(Deserialize)]
pub(super) struct SshPreflightReq {
    pub(super) host: String,
    pub(super) container: Option<String>,
}

pub(super) async fn ssh_master_status(Query(req): Query<SshPreflightReq>) -> ApiResult {
    let host = req.host.trim();
    if host.is_empty() {
        return Err(bad_request("host is required"));
    }
    // A missing master is meaningful only on platforms that support multiplexing.
    // Windows opens a new SSH connection for each command; reporting false here
    // makes a successful preflight immediately look disconnected in the dashboard.
    let running = if cfg!(unix) {
        Some(crate::jobs::ssh::master_is_running(&crate::jobs::ssh::SshTarget::alias(host)).await?)
    } else {
        None
    };
    Ok(Json(json!({ "running": running })))
}

/// Live check for one host: can we reach it and run bash/tar snapshots?
pub(super) async fn ssh_preflight(Json(req): Json<SshPreflightReq>) -> ApiResult {
    let host = req.host.trim().to_string();
    if host.is_empty() {
        return Err(bad_request("host is required"));
    }
    require_configured_ssh_host(&host).await?;
    if let Some(reference) = &req.container {
        crate::jobs::ssh::validate_container_reference(reference).map_err(bad_request)?;
    }
    let test = run_ssh_host_preflight(host.clone()).await;
    let container = if let Some(reference) = req.container {
        let result = crate::jobs::ssh::resolve_container(
            &crate::jobs::ssh::SshTarget::alias(&host),
            &reference,
        )
        .await;
        Some(
            json!({"reference": reference, "ready": result.is_ok(), "error": result.err().map(|error| error.to_string())}),
        )
    } else {
        None
    };
    let mut result =
        serde_json::to_value(test).map_err(|error| ApiError::from(anyhow!("{error}")))?;
    result["container"] = json!(container);
    Ok(Json(result))
}

pub(super) async fn require_configured_ssh_host(host: &str) -> Result<(), ApiError> {
    let hosts = tokio::task::spawn_blocking(list_ssh_hosts)
        .await
        .map_err(|e| ApiError::from(anyhow!("ssh task failed: {e}")))?;
    if hosts
        .iter()
        .any(|candidate| candidate.get("host").and_then(Value::as_str) == Some(host))
    {
        Ok(())
    } else {
        Err(bad_request("Choose a host from ~/.ssh/config."))
    }
}

pub(super) async fn run_ssh_host_preflight(host: String) -> SshHostTest {
    let test = probe_ssh_host_preflight(host).await;
    record_ssh_host_test(&test).await;
    test
}

pub(super) async fn probe_ssh_host_preflight(host: String) -> SshHostTest {
    let p = crate::jobs::ssh::preflight(&crate::jobs::ssh::SshTarget::alias(&host)).await;
    SshHostTest {
        host,
        reachable: p.reachable,
        tools_found: p.tools_found,
        missing_tools: p.missing_tools,
        error: p.error,
        tested_at: now_ms(),
    }
}

pub(super) async fn record_ssh_host_test(test: &SshHostTest) {
    // Best-effort persistence — the UI shows "last tested" across restarts,
    // but a store hiccup shouldn't hide a test result that already ran.
    let record = test.clone();
    if let Err(e) =
        tokio::task::spawn_blocking(move || Store::open()?.upsert_ssh_host_test(&record))
            .await
            .map_err(|e| anyhow!("ssh task failed: {e}"))
            .and_then(|r| r)
    {
        eprintln!("orx up: could not record ssh test for {}: {e}", test.host);
    }
}

// --- slurm --------------------------------------------------------------------

use crate::jobs::slurm;

/// One payload powers the whole settings card: stored cluster defaults plus
/// the ssh hosts to pick a login node from (same `~/.ssh/config` source as
/// the ssh backend — a Slurm login node is just an ssh host).
pub(super) fn slurm_settings_json() -> Value {
    let settings = slurm::load_settings().ok().flatten().unwrap_or_default();
    json!({
        "host": settings.host,
        "partition": settings.partition,
        "account": settings.account,
        "timeLimit": settings.time_limit,
        "cpusPerTask": settings.cpus_per_task,
        "mem": settings.mem,
        "hosts": list_ssh_hosts(),
    })
}

pub(super) async fn slurm_settings() -> ApiResult {
    tokio::task::spawn_blocking(|| Ok(Json(slurm_settings_json())))
        .await
        .map_err(|e| ApiError::from(anyhow!("slurm task failed: {e}")))?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetSlurmSettingsReq {
    /// `None` leaves the field alone; `Some("")` clears it (cluster default).
    host: Option<String>,
    partition: Option<String>,
    account: Option<String>,
    time_limit: Option<String>,
    /// Cores per job. `Some(0)` clears it (partition default).
    cpus_per_task: Option<u32>,
    mem: Option<String>,
}

pub(super) async fn set_slurm_settings(Json(req): Json<SetSlurmSettingsReq>) -> ApiResult {
    // One spawn_blocking around the whole load→mutate→save→respond body
    // (settings + ~/.ssh/config are sync fs I/O), like the git handlers.
    tokio::task::spawn_blocking(move || {
        let mut settings = slurm::load_settings()?.unwrap_or_default();
        let norm = |v: String| Some(v.trim().to_string()).filter(|s| !s.is_empty());
        if let Some(h) = req.host {
            settings.host = norm(h);
        }
        if let Some(p) = req.partition {
            settings.partition = norm(p);
        }
        if let Some(a) = req.account {
            settings.account = norm(a);
        }
        if let Some(t) = req.time_limit {
            // Reject a default that would fail every later launch.
            let t = norm(t);
            if let Some(t) = &t {
                crate::jobs::huggingface::parse_timeout(t).map_err(bad_request)?;
            }
            settings.time_limit = t;
        }
        if let Some(c) = req.cpus_per_task {
            settings.cpus_per_task = Some(c).filter(|c| *c > 0);
        }
        if let Some(m) = req.mem {
            // Reject a default that would fail every later launch.
            let m = norm(m);
            settings.mem = match m {
                Some(m) => Some(slurm::parse_mem(&m).map_err(bad_request)?),
                None => None,
            };
        }
        slurm::save_settings(&settings)?;
        Ok(Json(slurm_settings_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("slurm task failed: {e}")))?
}

#[derive(Deserialize)]
pub(super) struct SlurmPreflightReq {
    host: String,
}

/// Live check for one login node: reachable, Slurm CLI + snapshot tools, and
/// which partitions exist (feeds the partition picker).
pub(super) async fn slurm_preflight(Json(req): Json<SlurmPreflightReq>) -> ApiResult {
    let host = req.host.trim().to_string();
    if host.is_empty() {
        return Err(bad_request("host is required"));
    }
    let p = slurm::preflight(&host).await;
    Ok(Json(slurm_preflight_value(&p)))
}

pub(super) fn slurm_preflight_value(p: &slurm::SlurmPreflight) -> Value {
    json!({
        "reachable": p.reachable,
        "slurmFound": p.slurm_found,
        "toolsFound": p.tools_found,
        "partitions": p.partitions,
        "error": p.error,
    })
}

// --- ray --------------------------------------------------------------------

use crate::jobs::ray;

pub(super) fn ray_settings_json() -> Value {
    let settings = ray::load_settings().ok().flatten().unwrap_or_default();
    let (resolved, source) = ray::resolve_address_with_source();
    let source_label = match source {
        ray::AddressSource::Settings => "settings",
        ray::AddressSource::AstroaiEnv => "ASTROAI_RAY_JOBS_ADDRESS",
        ray::AddressSource::RayEnv => "RAY_DASHBOARD_URL",
        ray::AddressSource::Default => "default",
    };
    json!({
        "address": settings.address,
        "resolvedAddress": resolved,
        "source": source_label,
    })
}

pub(super) async fn ray_settings() -> ApiResult {
    tokio::task::spawn_blocking(|| Ok(Json(ray_settings_json())))
        .await
        .map_err(|e| ApiError::from(anyhow!("ray task failed: {e}")))?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetRaySettingsReq {
    /// `None` leaves alone; `Some("")` clears (fall back to env / default).
    address: Option<String>,
}

pub(super) async fn set_ray_settings(Json(req): Json<SetRaySettingsReq>) -> ApiResult {
    tokio::task::spawn_blocking(move || {
        let mut settings = ray::load_settings()?.unwrap_or_default();
        if let Some(a) = req.address {
            let a = Some(a.trim().to_string()).filter(|s| !s.is_empty());
            if let Some(a) = &a {
                // Reject a default that would fail every later launch.
                let url = reqwest::Url::parse(a)
                    .map_err(|e| bad_request(anyhow!("Invalid Jobs URL {a:?}: {e}")))?;
                if !matches!(url.scheme(), "http" | "https") {
                    return Err(bad_request(anyhow!(
                        "The Jobs URL must be http(s), e.g. http://127.0.0.1:8265 (got {a:?})."
                    )));
                }
            }
            settings.address = a;
        }
        ray::save_settings(&settings)?;
        Ok(Json(ray_settings_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("ray task failed: {e}")))?
}

#[derive(Deserialize)]
pub(super) struct RayPreflightReq {
    address: Option<String>,
}

/// Live check for a Ray Jobs / Dashboard endpoint.
pub(super) async fn ray_preflight(Json(req): Json<RayPreflightReq>) -> ApiResult {
    let address = ray::resolve_address(req.address.as_deref());
    match ray::preflight(&address).await {
        Ok(ray_version) => Ok(Json(json!({
            "reachable": true,
            "address": address,
            "rayVersion": ray_version,
            "error": null,
        }))),
        Err(e) => Ok(Json(json!({
            "reachable": false,
            "address": address,
            "rayVersion": null,
            "error": e.to_string(),
        }))),
    }
}

// --- compute targets (unified settings list + default) --------------------------

/// The whole payload for the Compute tab's collapsed list, in one round trip.
/// CHEAP probes only — env vars and file reads, never a network call, kubectl,
/// or the modal python import. `configured` means "worth trying", not
/// "healthy"; deep health stays in each backend's own settings endpoint,
/// fetched when a row is expanded.
/// Whether a box would accept this machine. Three-valued on purpose: the check
/// needs the api, so "we couldn't ask" is a different answer from "no" and the
/// badge shouldn't have to pick one of the two. Each arm carries what the row
/// needs to say — guessing a key path would send the user to a file that may
/// not exist.
#[derive(Clone, PartialEq)]
pub(super) enum SshReadiness {
    Ready,
    /// The `.pub` on this machine worth registering, if there is one.
    NoUsableKey {
        pub_path: Option<String>,
    },
    Unverified {
        reason: String,
    },
}

pub(super) async fn openresearch_ssh_readiness() -> SshReadiness {
    use crate::local::ssh_identity::{preferred_local, tilde, KeyStatus};
    let Ok(Some(creds)) = crate::config::load_credentials().await else {
        // Signed-out is reported by the row's own `or_logged_in`.
        return SshReadiness::NoUsableKey { pub_path: None };
    };
    let named = |local: &[crate::local::ssh_identity::LocalKey]| SshReadiness::NoUsableKey {
        pub_path: preferred_local(local)
            .and_then(|k| k.path.as_deref())
            .map(tilde),
    };
    match crate::local::ssh_identity::check(&creds).await {
        KeyStatus::Matched => SshReadiness::Ready,
        KeyStatus::NoLocalMatch { local, .. } | KeyStatus::NoneRegistered { local } => {
            named(&local)
        }
        KeyStatus::Unknown { reason } => SshReadiness::Unverified { reason },
    }
}

/// The openresearch row's one-line status. Every branch that tells the user to
/// run something names a path we actually found — never a guessed one.
pub(super) fn openresearch_summary(logged_in: bool, ssh: &SshReadiness) -> String {
    if !logged_in {
        return "Not signed in — run orx login".to_string();
    }
    match ssh {
        SshReadiness::Ready => "Signed in — ephemeral boxes billed to your org".to_string(),
        SshReadiness::NoUsableKey {
            pub_path: Some(path),
        } => format!("No usable SSH key — run orx ssh-key add {path}"),
        SshReadiness::NoUsableKey { pub_path: None } => {
            "No SSH key on this computer — run ssh-keygen -t ed25519, then orx ssh-key add"
                .to_string()
        }
        SshReadiness::Unverified { reason } => {
            format!("Signed in — couldn't check your SSH key ({reason})")
        }
    }
}

pub(super) fn compute_settings_json(ssh: SshReadiness) -> Value {
    let default = crate::config::compute_default();
    let (default_backend, default_flavor) = match &default {
        Some((b, f)) => (Some(b.as_str()), f.as_deref()),
        None => (None, None),
    };

    let hf = crate::jobs::huggingface::resolve_token_with_source().ok();
    let tinker = crate::jobs::tinker::resolve_api_key_with_source().ok();
    let modal_source = crate::jobs::modal::token_source();
    let k8s_settings = k8s::load_settings().ok().flatten();
    let ssh_hosts = list_ssh_hosts().len();
    let slurm_settings = crate::jobs::slurm::load_settings().ok().flatten();
    let slurm_host = slurm_settings.as_ref().and_then(|s| s.host.clone());
    let (ray_resolved, ray_source) = crate::jobs::ray::resolve_address_with_source();
    let ray_configured = !matches!(ray_source, crate::jobs::ray::AddressSource::Default);
    let ray_source_label = match ray_source {
        crate::jobs::ray::AddressSource::Settings => "Saved address",
        crate::jobs::ray::AddressSource::AstroaiEnv => "ASTROAI_RAY_JOBS_ADDRESS",
        crate::jobs::ray::AddressSource::RayEnv => "RAY_DASHBOARD_URL",
        crate::jobs::ray::AddressSource::Default => "Default localhost:8265",
    };
    // Presence of the credentials file only — whether the token still works is
    // the expanded row's (network) question.
    let or_logged_in = crate::config::credentials_present();

    // Same spellings as the expanded rows' SOURCE_LABELS/MODAL_TOKEN_LABELS
    // in the UI — the collapsed head stays visible above the open row, so the
    // same fact must not read two different ways.
    let source_label = |s: &crate::jobs::huggingface::TokenSource| match s {
        crate::jobs::huggingface::TokenSource::Env => "HF_TOKEN env var",
        crate::jobs::huggingface::TokenSource::OpenresearchEnv => "Token from Environment tab",
        crate::jobs::huggingface::TokenSource::HfCache => "Token from ~/.cache/huggingface/token",
    };
    let mut targets = json!([
        {
            "id": "local",
            "configured": true,
            "summary": "Runs as a detached process on this machine",
        },
        {
            "id": "ssh",
            "configured": ssh_hosts > 0,
            "summary": match ssh_hosts {
                0 => "No hosts in ~/.ssh/config".to_string(),
                1 => "1 host in ~/.ssh/config".to_string(),
                n => format!("{n} hosts in ~/.ssh/config"),
            },
        },
        {
            "id": "tinker",
            "configured": tinker.is_some(),
            "fromEnvironmentTab": matches!(tinker.as_ref().map(|(_, source)| source), Some(crate::jobs::tinker::ApiKeySource::OpenresearchEnv)),
            "summary": match tinker.map(|(_, source)| source) {
                Some(crate::jobs::tinker::ApiKeySource::Env) => "TINKER_API_KEY env var",
                Some(crate::jobs::tinker::ApiKeySource::OpenresearchEnv) => "Key from Environment tab",
                None => "No API key",
            },
        },
        {
            "id": "hf",
            "configured": hf.is_some(),
            "fromEnvironmentTab": matches!(hf.as_ref().map(|(_, source)| source), Some(crate::jobs::huggingface::TokenSource::OpenresearchEnv)),
            "summary": hf.as_ref().map_or_else(
                || "No token".to_string(),
                |(_, s)| source_label(s).to_string(),
            ),
        },
        {
            "id": "modal",
            "configured": modal_source.is_some(),
            "fromEnvironmentTab": modal_source == Some("syncedEnv"),
            "summary": match modal_source {
                Some("env") => "MODAL_TOKEN_ID + MODAL_TOKEN_SECRET env vars",
                Some("syncedEnv") => "Token from Environment tab",
                Some("modalToml") => "Token from ~/.modal.toml",
                _ => "No token",
            },
        },
        {
            "id": "k8s",
            "configured": k8s_settings.is_some(),
            "summary": k8s_settings.as_ref().map_or_else(
                || "No context selected".to_string(),
                |s| format!(
                    "Context {} / namespace {}",
                    s.context.as_deref().unwrap_or("(kubectl default)"),
                    s.namespace,
                ),
            ),
        },
        {
            "id": "slurm",
            "configured": slurm_host.is_some(),
            "summary": slurm_host.as_ref().map_or_else(
                || "No login node configured".to_string(),
                |h| match slurm_settings.as_ref().and_then(|s| s.partition.as_deref()) {
                    Some(partition) => format!("Login node {h} / partition {partition}"),
                    None => format!("Login node {h}"),
                },
            ),
        },
        {
            "id": "ray",
            "configured": ray_configured,
            "summary": if ray_configured {
                format!("{ray_source_label} ({ray_resolved})")
            } else {
                ray_source_label.to_string()
            },
        },
        {
            "id": "openresearch",
            // Signed in alone would be a green light on a backend that can't
            // connect — the box authorizes your *registered* keys, so one of
            // them has to be on this machine too.
            "configured": or_logged_in && ssh == SshReadiness::Ready,
            "unverified": or_logged_in && matches!(ssh, SshReadiness::Unverified { .. }),
            "summary": openresearch_summary(or_logged_in, &ssh),
        },
    ]);
    if let Some(targets) = targets.as_array_mut() {
        for target in targets {
            if let Some(target) = target.as_object_mut() {
                target.insert("enabled".to_string(), Value::Bool(true));
                target.insert("disabledReason".to_string(), Value::Null);
            }
        }
    }
    json!({
        "defaultBackend": default_backend.unwrap_or("local"),
        "defaultFlavor": default_flavor,
        "configuredDefaultBackend": default_backend,
        "configuredDefaultFlavor": default_flavor,
        "targets": targets,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ComputeSettingsQuery {
    project_id: Option<String>,
}

pub(super) async fn compute_settings(Query(query): Query<ComputeSettingsQuery>) -> ApiResult {
    let ssh = openresearch_ssh_readiness().await;
    let _project_id = query.project_id;
    // fs/env probes only, but keep them off the async runtime anyway.
    let payload =
        tokio::task::spawn_blocking(move || -> Result<Value> { Ok(compute_settings_json(ssh)) })
            .await
            .map_err(|e| ApiError::from(anyhow!("compute settings task failed: {e}")))??;
    Ok(Json(payload))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetComputeDefaultReq {
    /// `None`/absent clears the default (and its flavor with it).
    backend: Option<String>,
    flavor: Option<String>,
    project_id: Option<String>,
}

/// Persist the default compute target. An *unconfigured* backend is allowed
/// (config state fluctuates outside orx; the UI warns instead) — only unknown
/// backends and meaningless flavors are rejected.
pub(super) async fn set_compute_default(Json(req): Json<SetComputeDefaultReq>) -> ApiResult {
    let _project_id = req.project_id;
    let backend = req
        .backend
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty());
    let flavor = req
        .flavor
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty());
    if let Some(b) = &backend {
        local::validate_compute_default(b, flavor.as_deref()).map_err(bad_request)?;
    }
    // Picking openresearch as the default is the moment to answer "will this
    // actually work?", so the row that comes back is honest about the SSH key.
    let ssh = openresearch_ssh_readiness().await;
    // Validation already ran above, so a failure in here is a server-side
    // fault (io error, corrupt settings.json refusal) — surface it as 500 via
    // the plain ApiError conversion, not as a 400 blaming the request.
    let payload = tokio::task::spawn_blocking(move || -> Result<Value> {
        crate::config::set_compute_default(backend, flavor)?;
        Ok(compute_settings_json(ssh))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("compute default task failed: {e}")))??;
    Ok(Json(payload))
}

/// The "This machine" row's expanded detail: detected hardware. Subprocess
/// probes (hostname, sysctl, nvidia-smi) — blocking, so spawned.
pub(super) async fn local_machine_settings() -> ApiResult {
    let hw = tokio::task::spawn_blocking(crate::jobs::localbox::hardware_info)
        .await
        .map_err(|e| ApiError::from(anyhow!("hardware probe task failed: {e}")))?;
    Ok(Json(json!(hw)))
}

/// The OpenResearch row's expanded detail. Network calls are fine here (the
/// row is open) but each is individually best-effort — an offline machine
/// still renders "signed in, status unknown" instead of an error page.
pub(super) async fn openresearch_settings() -> ApiResult {
    let Some(creds) = crate::config::load_credentials().await? else {
        return Ok(Json(json!({
            "loggedIn": false,
            "apiUrl": null,
            "orgs": [],
            "sshKeyStatus": "unknown",
            "error": null,
        })));
    };
    let mut error: Option<String> = None;
    let orgs = match crate::client::list_orgs(&creds).await {
        Ok(o) => o.orgs.into_iter().map(|o| o.name).collect::<Vec<_>>(),
        Err(e) => {
            error = Some(e.to_string());
            Vec::new()
        }
    };
    // "Registered" alone is a misleading green: a key registered from another
    // laptop leaves this machine unable to reach any box. Report whether the
    // private half is actually here.
    use crate::local::ssh_identity::{preferred_local, tilde, KeyStatus};
    // Hand back the .pub we actually found so the note can name a real file
    // rather than guessing at ~/.ssh/id_ed25519.pub.
    let mut ssh_key_path: Option<String> = None;
    let mut note_key = |local: &[crate::local::ssh_identity::LocalKey]| {
        ssh_key_path = preferred_local(local)
            .and_then(|k| k.path.as_deref())
            .map(tilde);
    };
    let ssh_key_status = match crate::local::ssh_identity::check(&creds).await {
        KeyStatus::Matched => "matched",
        KeyStatus::NoLocalMatch { local, .. } => {
            note_key(&local);
            "no_local_match"
        }
        KeyStatus::NoneRegistered { local } => {
            note_key(&local);
            "none_registered"
        }
        KeyStatus::Unknown { reason } => {
            error.get_or_insert(reason);
            "unknown"
        }
    };
    Ok(Json(json!({
        "loggedIn": true,
        "apiUrl": creds.api_url,
        "orgs": orgs,
        "sshKeyStatus": ssh_key_status,
        "sshKeyPath": ssh_key_path,
        "error": error,
    })))
}

fn cli_result(result: ApiResult) -> Result<Value> {
    result
        .map(|Json(value)| value)
        .map_err(|ApiError(_, message)| anyhow!(message))
}

pub(crate) async fn status() -> Result<Value> {
    cli_result(compute_settings(Query(ComputeSettingsQuery { project_id: None })).await)
}

pub(crate) async fn show(backend: &str) -> Result<Value> {
    let mut value = cli_result(match backend {
        "local" => local_machine_settings().await,
        "ssh" => ssh_settings().await,
        "slurm" => slurm_settings().await,
        "ray" => ray_settings().await,
        "hf" => Ok(hf_settings().await),
        "modal" => modal_settings().await,
        "tinker" => tinker_settings().await,
        "k8s" => k8s_settings().await,
        "openresearch" => openresearch_settings().await,
        _ => return Err(anyhow!("Unknown compute backend: {backend}")),
    })?;
    if backend == "ssh" {
        if let Some(hosts) = value["hosts"].as_array_mut() {
            for host in hosts {
                if let Some(alias) = host["host"].as_str() {
                    host["masterRunning"] = ssh_master_status(Query(SshPreflightReq {
                        host: alias.to_owned(),
                        container: None,
                    }))
                    .await
                    .ok()
                    .map(|Json(value)| value["running"].clone())
                    .unwrap_or(Value::Null);
                }
            }
        }
    }
    Ok(value)
}

pub(crate) async fn default(backend: Option<String>, flavor: Option<String>) -> Result<Value> {
    cli_result(
        set_compute_default(Json(SetComputeDefaultReq {
            backend,
            flavor,
            project_id: None,
        }))
        .await,
    )
}

pub(crate) async fn configure(backend: &str, body: Value) -> Result<Value> {
    if body.get("clearCredentials") == Some(&Value::Bool(true)) {
        let keys: &[&str] = match backend {
            "hf" => &["HF_TOKEN"],
            "tinker" => &["TINKER_API_KEY"],
            "modal" => &["MODAL_TOKEN_ID", "MODAL_TOKEN_SECRET"],
            _ => return Err(anyhow!("No saved credentials for {backend}.")),
        };
        for key in keys {
            crate::config::remove_synced_env_var(key)?;
        }
        let mut value = show(backend)
            .await
            .unwrap_or_else(|error| json!({"error": error.to_string()}));
        value["clearedSavedCredentials"] = json!(true);
        return Ok(value);
    }
    cli_result(match backend {
        "ssh" => {
            // Validate both changes before persisting either.
            if let Some(host) = body.get("defaultHost").and_then(Value::as_str) {
                require_configured_ssh_host(host)
                    .await
                    .map_err(|ApiError(_, message)| anyhow!(message))?;
            }
            if body.get("container").is_some() {
                let req: SaveSshSettingsReq = serde_json::from_value(
                    json!({"host": body["host"], "container": body["container"]}),
                )?;
                cli_result(save_ssh_settings(Json(req)).await)?;
            }
            if let Some(host) = body.get("defaultHost") {
                cli_result(
                    save_ssh_default(Json(SaveSshDefaultReq {
                        host: serde_json::from_value(host.clone())?,
                    }))
                    .await,
                )?;
            }
            ssh_settings().await
        }
        "slurm" => set_slurm_settings(Json(serde_json::from_value(body)?)).await,
        "ray" => set_ray_settings(Json(serde_json::from_value(body)?)).await,
        "k8s" => set_k8s_settings(Json(serde_json::from_value(body)?)).await,
        "hf" => set_hf_token(Json(serde_json::from_value(body)?)).await,
        "modal" => set_modal_token(Json(serde_json::from_value(body)?)).await,
        "tinker" => set_tinker_key(Json(serde_json::from_value(body)?)).await,
        _ => {
            return Err(anyhow!(
                "{backend} has no saved configuration; use compute connect or default set."
            ))
        }
    })
}

pub(crate) async fn check(
    backend: &str,
    host: Option<String>,
    container: Option<String>,
    no_container: bool,
    address: Option<String>,
) -> Result<Value> {
    let mut value = match backend {
        "ssh" => {
            let settings = crate::config::ssh_settings()?;
            let host = host
                .or(settings.default_host)
                .ok_or_else(|| anyhow!("Pass --host or configure a default SSH host."))?;
            let container = if no_container {
                None
            } else {
                container.or_else(|| settings.hosts.get(&host).and_then(|s| s.container.clone()))
            };
            cli_result(ssh_preflight(Json(SshPreflightReq { host, container })).await)?
        }
        "slurm" => {
            let host = host
                .or(slurm::load_settings()?.unwrap_or_default().host)
                .ok_or_else(|| anyhow!("Pass --host or configure a Slurm host."))?;
            cli_result(slurm_preflight(Json(SlurmPreflightReq { host })).await)?
        }
        "ray" => cli_result(ray_preflight(Json(RayPreflightReq { address })).await)?,
        "modal" => match modal::preflight().await {
            Ok(()) => json!({"ready": true}),
            Err(error) => json!({"ready": false, "error": error.to_string()}),
        },
        _ => show(backend)
            .await
            .unwrap_or_else(|error| json!({"error": error.to_string()})),
    };
    let ready = match backend {
        "local" => true,
        "ssh" => {
            value["reachable"] == true
                && value["toolsFound"] == true
                && (value["container"].is_null() || value["container"]["ready"] == true)
        }
        "slurm" => {
            value["reachable"] == true && value["toolsFound"] == true && value["slurmFound"] == true
        }
        "ray" => value["reachable"] == true,
        "k8s" => {
            value["preflight"]["reachable"] == true && value["preflight"]["canCreateJobs"] == true
        }
        "hf" => value["validationStatus"] == "valid" && value["jobsWrite"] != false,
        "tinker" => value["validationStatus"] == "valid",
        "openresearch" => {
            value["loggedIn"] == true
                && value["sshKeyStatus"] == "matched"
                && value["error"].is_null()
        }
        "modal" => value["ready"] == true,
        _ => false,
    };
    value["ready"] = json!(ready);
    Ok(value)
}

pub(crate) async fn read_ssh_config() -> Result<Value> {
    cli_result(ssh_config().await)
}
pub(crate) async fn write_ssh_config(content: String, previous_content: String) -> Result<Value> {
    cli_result(
        save_ssh_config(Json(SaveSshConfigReq {
            content,
            previous_content,
        }))
        .await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_home(files: &[(&str, &str)]) -> crate::local::git::TemporaryDirectory {
        let home = crate::local::git::TemporaryDirectory::new("orx-ssh-include").unwrap();
        for (path, contents) in files {
            let path = path
                .split('/')
                .fold(home.path().to_path_buf(), |p, part| p.join(part));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        home
    }

    fn names(hosts: &[Value]) -> Vec<&str> {
        hosts.iter().map(|h| h["host"].as_str().unwrap()).collect()
    }

    #[test]
    fn ssh_hosts_lists_direct_hosts_without_patterns_or_match() {
        let home = fake_home(&[(
            ".ssh/config",
            "Host lab gpu-*\n  HostName lab.invalid\n  User alice\nHost *\n  User nobody\nMatch host lab\n  Port 2222\n",
        )]);
        let hosts = ssh_hosts_in(home.path());
        assert_eq!(names(&hosts), ["lab"]);
        assert_eq!(hosts[0]["hostname"], "lab.invalid");
        assert_eq!(hosts[0]["user"], "alice");
        assert!(hosts[0].get("port").is_none());
    }

    #[test]
    fn ssh_hosts_follow_relative_glob_absolute_and_tilde_includes() {
        let home = fake_home(&[
            ("absolute.conf", "Host absolute\n"),
            ("tilde.conf", "Host tilde\n"),
            (".ssh/extra.conf", "Host relative\n  User bob\n"),
            (".ssh/config.d/b.conf", "Host glob-b\n"),
            (".ssh/config.d/a.conf", "Host glob-a\n"),
            (".ssh/config.d/.hidden", "Host hidden\n"),
        ]);
        let absolute = home.path().join("absolute.conf");
        std::fs::write(
            home.path().join(".ssh").join("config"),
            format!(
                "Include extra.conf config.d/*\nInclude \"{}\"\nInclude ~/tilde.conf\nHost direct\n",
                absolute.display()
            ),
        )
        .unwrap();
        let hosts = ssh_hosts_in(home.path());
        assert_eq!(
            names(&hosts),
            ["relative", "glob-a", "glob-b", "absolute", "tilde", "direct"]
        );
        assert_eq!(hosts[0]["user"], "bob");
    }

    #[test]
    fn ssh_include_inside_host_block_resumes_enclosing_block() {
        let home = fake_home(&[
            (
                ".ssh/config",
                "Host outer\n  Include inner.conf\n  User carol\n",
            ),
            (
                ".ssh/inner.conf",
                "HostName outer.invalid\nHost inner\n  User dave\n",
            ),
        ]);
        let hosts = ssh_hosts_in(home.path());
        assert_eq!(names(&hosts), ["outer", "inner"]);
        assert_eq!(hosts[0]["hostname"], "outer.invalid");
        assert_eq!(hosts[0]["user"], "carol");
        assert_eq!(hosts[1]["user"], "dave");
    }

    #[test]
    fn ssh_each_included_file_starts_in_the_enclosing_block() {
        let home = fake_home(&[
            (
                ".ssh/config",
                "Host outer
  Include a.conf b.conf
",
            ),
            (
                ".ssh/a.conf",
                "Host a
  User alice
",
            ),
            (
                ".ssh/b.conf",
                "Port 2222
Host b
",
            ),
        ]);
        let hosts = ssh_hosts_in(home.path());
        assert_eq!(names(&hosts), ["outer", "a", "b"]);
        assert_eq!(hosts[0]["port"], "2222");
        assert!(hosts[1].get("port").is_none());
    }

    #[test]
    fn ssh_comments_start_only_at_an_unquoted_word() {
        let home = fake_home(&[
            (
                ".ssh/config",
                "Include \"conf.d/lab#1.conf\" # the first lab
# Host commented-out
",
            ),
            (
                ".ssh/conf.d/lab#1.conf",
                "Host lab-one # note
  User alice#1
",
            ),
        ]);
        let hosts = ssh_hosts_in(home.path());
        assert_eq!(names(&hosts), ["lab-one"]);
        assert_eq!(hosts[0]["user"], "alice#1");
    }

    #[test]
    fn ssh_include_cycles_terminate_and_merge_repeated_hosts() {
        let home = fake_home(&[
            (".ssh/config", "Include a.conf\nHost top a\n  User erin\n"),
            (".ssh/a.conf", "Host a\nInclude b.conf\n"),
            (".ssh/b.conf", "Host b\nInclude a.conf config\n"),
        ]);
        let hosts = ssh_hosts_in(home.path());
        assert_eq!(names(&hosts), ["a", "b", "top"]);
        assert_eq!(hosts[0]["user"], "erin");
    }

    #[test]
    fn ssh_include_missing_files_are_skipped() {
        let home = fake_home(&[(
            ".ssh/config",
            "Host before\nInclude missing.conf nowhere.d/* ~/absent/*.conf\nHost after\n",
        )]);
        assert_eq!(names(&ssh_hosts_in(home.path())), ["before", "after"]);
    }

    #[test]
    fn ssh_includes_support_single_quotes_and_escaped_spaces() {
        let home = fake_home(&[
            (
                ".ssh/config",
                r#"Include 'conf.d/lab #1.conf' conf.d/lab\ space.conf conf.d/lab\'s.conf # ignored
"#,
            ),
            (".ssh/conf.d/lab #1.conf", "Host quoted\n"),
            (".ssh/conf.d/lab space.conf", "Host escaped\n"),
            (".ssh/conf.d/lab's.conf", "Host apostrophe\n"),
        ]);
        assert_eq!(
            names(&ssh_hosts_in(home.path())),
            ["quoted", "escaped", "apostrophe"]
        );
    }

    #[test]
    fn ssh_includes_expand_named_user_homes() {
        #[cfg(windows)]
        let user = std::env::var("USERNAME").unwrap();
        #[cfg(unix)]
        let user = String::from_utf8(
            std::process::Command::new("id")
                .arg("-un")
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        let actual_home = ssh_user_home(&user).unwrap();
        assert!(actual_home.is_dir());
        let fake_home = fake_home(&[(".ssh/config", "")]);
        assert_eq!(
            expand_ssh_include(fake_home.path(), &format!("~{user}")),
            [actual_home]
        );
        assert!(expand_ssh_include(
            fake_home.path(),
            &format!("~orx-missing-{}/config", uuid::Uuid::new_v4()),
        )
        .is_empty());
    }

    #[test]
    fn ssh_args_preserve_windows_separators_and_quoted_hashes() {
        assert_eq!(
            ssh_config_args(r#"C:\Users\alice\lab.conf 'lab #1.conf' lab\ #2.conf "#),
            [r"C:\Users\alice\lab.conf", "lab #1.conf", "lab #2.conf"]
        );
        assert_eq!(
            ssh_config_args(r#""lab\"quote" 'lab\'quote'"#),
            ["lab\"quote", "lab'quote"]
        );
        assert!(ssh_config_args("'unclosed path").is_empty());
    }

    #[test]
    fn ssh_include_globs_match_explicit_dotfiles_only() {
        let home = fake_home(&[
            (".ssh/config", "Include conf.d/.lab*.conf conf.d/*.conf\n"),
            (".ssh/conf.d/.lab1.conf", "Host hidden\n"),
            (".ssh/conf.d/.private.conf", "Host private\n"),
            (".ssh/conf.d/lab.conf", "Host public\n"),
        ]);
        assert_eq!(names(&ssh_hosts_in(home.path())), ["hidden", "public"]);
    }

    #[test]
    fn ssh_include_double_stars_are_nonrecursive() {
        let home = fake_home(&[
            (".ssh/config", "Include conf.d/**.conf conf.d/**/lab.conf\n"),
            (".ssh/conf.d/a.conf", "Host immediate\n"),
            (".ssh/conf.d/one/lab.conf", "Host one\n"),
            (".ssh/conf.d/one/two/lab.conf", "Host two\n"),
        ]);
        assert_eq!(names(&ssh_hosts_in(home.path())), ["immediate", "one"]);
    }

    #[cfg(unix)]
    #[test]
    fn ssh_include_globs_preserve_escaped_literal_characters() {
        let home = fake_home(&[
            (
                ".ssh/config",
                r"Include conf.d/foo\#bar.conf conf.d/\*.conf",
            ),
            (".ssh/conf.d/foo#bar.conf", "Host hash\n"),
            (".ssh/conf.d/*.conf", "Host literal\n"),
            (".ssh/conf.d/other.conf", "Host other\n"),
        ]);
        assert_eq!(names(&ssh_hosts_in(home.path())), ["hash", "literal"]);
    }

    #[cfg(unix)]
    #[test]
    fn ssh_include_globs_skip_non_utf8_filenames_without_panicking() {
        use std::os::unix::ffi::OsStringExt;
        let home = fake_home(&[
            (".ssh/config", "Host direct\nInclude conf.d/*.conf\n"),
            (".ssh/conf.d/lab.conf", "Host included\n"),
        ]);
        let name = std::ffi::OsString::from_vec(b"bad\xff.conf".to_vec());
        std::fs::write(home.path().join(".ssh/conf.d").join(name), "Host bad\n").unwrap();
        assert_eq!(names(&ssh_hosts_in(home.path())), ["direct", "included"]);
    }

    #[test]
    fn ssh_include_globs_match_components_and_character_classes() {
        let home = fake_home(&[
            (".ssh/config", "Include = config.?/[ab].conf\n"),
            (".ssh/config.d/a.conf", "Host a\n"),
            (".ssh/config.d/b.conf", "Host b\n"),
            (".ssh/config.d/c.conf", "Host excluded\n"),
            (".ssh/config.d/nested/a.conf", "Host nested\n"),
        ]);
        assert_eq!(names(&ssh_hosts_in(home.path())), ["a", "b"]);
    }

    #[test]
    fn ssh_nested_includes_resolve_under_ssh_and_stop_at_depth_limit() {
        let home = fake_home(&[
            (".ssh/config", "Include conf.d/start.conf\n"),
            (".ssh/conf.d/start.conf", "Include 0.conf\n"),
        ]);
        for i in 0..=SSH_INCLUDE_MAX_DEPTH {
            std::fs::write(
                home.path().join(".ssh").join(format!("{i}.conf")),
                format!("Host depth-{i}\nInclude {}.conf\n", i + 1),
            )
            .unwrap();
        }
        let hosts = ssh_hosts_in(home.path());
        assert_eq!(hosts.len(), SSH_INCLUDE_MAX_DEPTH - 1);
        assert_eq!(hosts[0]["host"], "depth-0");
        assert_eq!(hosts.last().unwrap()["host"], "depth-14");
    }
}
