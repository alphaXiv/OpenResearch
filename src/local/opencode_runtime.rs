use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use base64::Engine;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::error::{anyhow, Result};
use crate::local::native_store::opencode_database::DatabaseLease;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Protocol {
    V1,
    V2,
}

impl Protocol {
    pub fn major(self) -> u64 {
        match self {
            Self::V1 => 1,
            Self::V2 => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedBinary {
    pub path: PathBuf,
    pub version: String,
    pub protocol: Protocol,
    modified: std::time::SystemTime,
    size: u64,
}

impl ResolvedBinary {
    pub fn check_unchanged(&self) -> Result<()> {
        let metadata = std::fs::metadata(&self.path)?;
        if metadata.modified()? != self.modified || metadata.len() != self.size {
            return Err(anyhow!(
                "OpenCode changed during startup. Retry with the newly installed version."
            ));
        }
        Ok(())
    }
}

pub(crate) struct ProbeEnvironment(crate::local::git::TemporaryDirectory);

impl ProbeEnvironment {
    pub fn new() -> Result<Self> {
        Ok(Self(crate::local::git::TemporaryDirectory::new(
            "orx-opencode-probe",
        )?))
    }

    pub fn configure(&self, cmd: &mut Command) {
        for (key, dir) in [
            ("HOME", "home"),
            ("USERPROFILE", "home"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_CACHE_HOME", "cache"),
            ("OPENCODE_CONFIG_DIR", "config/opencode"),
        ] {
            cmd.env(key, self.0.path().join(dir));
        }
        cmd.current_dir(self.0.path())
            .env("OPENCODE_DB", ":memory:")
            .env_remove("OPENCODE_CONFIG")
            .env_remove("OPENCODE_CONFIG_CONTENT")
            .env("OPENCODE_DISABLE_PROJECT_CONFIG", "1")
            .env("OPENCODE_CONFIG_PROJECT_DISABLE", "1")
            .env("OPENCODE_DISABLE_AUTOUPDATE", "1")
            .env("OPENCODE_DISABLE_MODELS_FETCH", "1")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .kill_on_drop(true);
    }
}

/// First opencode that resolves, in discovery order — picking the highest
/// version instead would move a pinned 1.x install onto the 2.x protocol.
/// The `--version` probes run in parallel (a cold node start can take
/// seconds); only the pick stays discovery-ordered.
pub(crate) async fn resolve_binary() -> Result<ResolvedBinary> {
    let candidates = super::opencode_candidates();
    let results = futures::future::join_all(candidates.into_iter().map(resolve_binary_at)).await;
    let mut first_error = None;
    for result in results {
        match result {
            Ok(binary) => return Ok(binary),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    Err(first_error.unwrap_or_else(super::not_found))
}

pub(crate) async fn resolve_binary_at(path: PathBuf) -> Result<ResolvedBinary> {
    let path = crate::paths::canonicalize(path)?;
    // The installer's layout pins the matching plugin release in
    // `<install>/package.json` next to `bin/` — when that pin is at least as
    // fresh as the binary it stands in for the multi-second `--version`
    // child (one costs ~4s on Windows).
    if let Some(binary) = resolve_binary_from_pin(&path) {
        return Ok(binary);
    }
    let metadata = std::fs::metadata(&path)?;
    let probe = ProbeEnvironment::new()?;
    let mut cmd = Command::new(&path);
    crate::local::chat::prepare_env(&mut cmd);
    probe.configure(&mut cmd);
    cmd.arg("--version");
    let output = crate::local::harness::detect_spawn_output_timed(cmd, Duration::from_secs(15))
        .await
        .ok_or_else(|| anyhow!("OpenCode version check timed out"))??;
    if !output.status.success() {
        return Err(anyhow!(
            "OpenCode version check failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let protocol = protocol_for_version(&version)?;
    let binary = ResolvedBinary {
        path,
        version,
        protocol,
        modified: metadata.modified()?,
        size: metadata.len(),
    };
    binary.check_unchanged()?;
    Ok(binary)
}

/// The opencode version pinned by the installer's own layout:
/// `<install>/{package.json,bin/opencode.exe}` carries
/// `dependencies."@opencode-ai/plugin"` written at install/upgrade time.
/// A binary whose mtime sits outside the manifest's install burst was
/// replaced without the pin following — distrust it and let the caller spawn
/// `--version` instead. Any other layout returns `None` the same way.
fn resolve_binary_from_pin(path: &Path) -> Option<ResolvedBinary> {
    let install = path.parent()?.parent()?;
    if !install
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.to_ascii_lowercase().contains("opencode"))
    {
        return None;
    }
    let metadata = std::fs::metadata(path).ok()?;
    let pin_path = install.join("package.json");
    let pin_modified = std::fs::metadata(&pin_path).ok()?.modified().ok()?;
    // The installer writes the manifest then extracts the binary, so a fresh
    // install legitimately has the binary a few seconds newer; a *replaced*
    // binary differs by days, and a manifest newer than the binary describes
    // a different install. Five minutes separates the two, either direction.
    let gap = match metadata.modified().ok()?.duration_since(pin_modified) {
        Ok(gap) => gap,
        Err(earlier) => earlier.duration(),
    };
    if gap > Duration::from_secs(300) {
        return None;
    }
    let pkg: serde_json::Value = serde_json::from_slice(&std::fs::read(pin_path).ok()?).ok()?;
    let deps = &pkg["dependencies"];
    let version = deps["@opencode-ai/plugin"]
        .as_str()
        .or_else(|| deps["opencode-ai"].as_str())?
        .trim_start_matches(['^', '~', 'v'])
        .to_string();
    let protocol = protocol_for_version(&version).ok()?;
    Some(ResolvedBinary {
        path: path.to_path_buf(),
        version,
        protocol,
        modified: metadata.modified().ok()?,
        size: metadata.len(),
    })
}

fn protocol_for_version(version: &str) -> Result<Protocol> {
    let semver = version
        .split_whitespace()
        .find_map(|part| semver::Version::parse(part.trim_start_matches('v')).ok())
        .ok_or_else(|| anyhow!("Unrecognized OpenCode version: {version}"))?;
    match semver.major {
        1 => Ok(Protocol::V1),
        2 => Ok(Protocol::V2),
        _ => Err(anyhow!(
            "OpenCode {version} is unsupported. Install a V1 or V2 release."
        )),
    }
}

#[derive(Clone)]
pub(crate) struct AgentEndpoint {
    pub base_url: String,
    pub client: reqwest::Client,
    pub protocol: Protocol,
    pub legacy_v2_api: bool,
}

impl AgentEndpoint {
    async fn check_health(&mut self) -> Result<bool> {
        let path = match self.protocol {
            Protocol::V1 => "/global/health",
            Protocol::V2 => "/api/status",
        };
        let response = self
            .client
            .get(format!("{}{path}", self.base_url))
            .timeout(Duration::from_secs(2))
            .send()
            .await?;
        if self.protocol == Protocol::V2 && response.status() == reqwest::StatusCode::NOT_FOUND {
            let response = self
                .client
                .get(format!("{}/api/health", self.base_url))
                .timeout(Duration::from_secs(2))
                .send()
                .await?;
            self.legacy_v2_api = response.status().is_success();
            return Ok(self.legacy_v2_api);
        }
        Ok(response.status().is_success())
    }

    pub fn v2_export_path(&self, session: &str) -> String {
        let prefix = if self.legacy_v2_api {
            "/api"
        } else {
            "/api/experimental"
        };
        format!("{prefix}/session/{session}/export")
    }

    pub fn v2_generate_path(&self) -> &'static str {
        if self.legacy_v2_api {
            "/api/generate"
        } else {
            "/api/experimental/generate"
        }
    }

    pub fn port(&self) -> Result<u16> {
        reqwest::Url::parse(&self.base_url)?
            .port()
            .ok_or_else(|| anyhow!("OpenCode readiness URL has no port"))
    }
}

pub(crate) async fn start_server(
    binary: &ResolvedBinary,
    mut cmd: Command,
) -> Result<(Child, AgentEndpoint)> {
    if let Some(parent) = super::agent_log_path().parent() {
        std::fs::create_dir_all(parent)?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(super::agent_log_path())?;
    let mut headers = reqwest::header::HeaderMap::new();
    let v1_port = if binary.protocol == Protocol::V1 {
        Some(super::free_port()?)
    } else {
        None
    };
    cmd.env("OPENCODE_DISABLE_AUTOUPDATE", "1")
        .arg("serve")
        .kill_on_drop(true)
        .stderr(Stdio::from(log.try_clone()?));
    if let Some(port) = v1_port {
        cmd.args([
            "--port",
            &port.to_string(),
            "--hostname",
            "127.0.0.1",
            "--print-logs",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(log));
    } else {
        let password = uuid::Uuid::new_v4().to_string();
        let auth = base64::engine::general_purpose::STANDARD.encode(format!("opencode:{password}"));
        let mut value = reqwest::header::HeaderValue::from_str(&format!("Basic {auth}"))?;
        value.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, value);
        cmd.args(["--stdio", "--port", "0"])
            .env("OPENCODE_PASSWORD", password)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
    }
    #[cfg(unix)]
    cmd.process_group(0);
    binary.check_unchanged()?;
    let mut child = crate::local::harness::spawn_retrying_busy(|| cmd.spawn())?;
    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()?;
    let ready = async {
        let base_url = if let Some(port) = v1_port {
            format!("http://127.0.0.1:{port}")
        } else {
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| anyhow!("OpenCode readiness pipe missing"))?;
            let mut lines = BufReader::new(stdout).lines();
            let first = tokio::time::timeout(super::HEALTH_TIMEOUT, lines.next_line())
                .await??
                .ok_or_else(|| anyhow!("OpenCode exited before reporting readiness"))?;
            let ready: serde_json::Value = serde_json::from_str(&first)?;
            let url = ready["url"]
                .as_str()
                .ok_or_else(|| anyhow!("Invalid OpenCode readiness response"))?
                .to_string();
            let parsed = reqwest::Url::parse(&url)?;
            if parsed.scheme() != "http"
                || !matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
            {
                return Err(anyhow!("OpenCode returned a non-loopback server URL"));
            }
            tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
            url.trim_end_matches('/').to_string()
        };
        let mut endpoint = AgentEndpoint {
            base_url,
            client,
            protocol: binary.protocol,
            legacy_v2_api: false,
        };
        let deadline = tokio::time::Instant::now() + super::HEALTH_TIMEOUT;
        loop {
            if let Some(status) = child.try_wait()? {
                return Err(anyhow!("OpenCode exited during startup ({status})"));
            }
            if endpoint.check_health().await.unwrap_or(false) {
                return Ok(endpoint);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(anyhow!(
                    "OpenCode health check timed out; see {}",
                    super::agent_log_path().display()
                ));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    .await;
    match ready {
        Ok(endpoint) => Ok((child, endpoint)),
        Err(error) => {
            let _ = child.kill().await;
            Err(error)
        }
    }
}

pub(crate) async fn prepare_database(binary: &ResolvedBinary, db: &Path) -> Result<DatabaseLease> {
    prepare_database_waiting(binary, db, Duration::from_secs(3), |_| {}).await
}

pub(super) async fn prepare_database_with_progress(
    binary: &ResolvedBinary,
    db: &Path,
    progress: impl FnMut(&str) + Send,
) -> Result<DatabaseLease> {
    prepare_database_waiting(binary, db, Duration::from_secs(600), progress).await
}

async fn prepare_database_waiting(
    binary: &ResolvedBinary,
    db: &Path,
    busy_timeout: Duration,
    mut progress: impl FnMut(&str) + Send,
) -> Result<DatabaseLease> {
    let major = binary.protocol.major();
    let deadline = tokio::time::Instant::now() + busy_timeout;
    let lease = loop {
        let path = db.to_path_buf();
        match tokio::task::spawn_blocking(move || DatabaseLease::acquire(&path, major)).await? {
            Ok(lease) => break lease,
            Err(error)
                if error
                    .downcast_ref::<crate::local::native_store::opencode_database::DatabaseBusy>()
                    .is_some()
                    && tokio::time::Instant::now() < deadline =>
            {
                progress("Waiting for other OpenCode work or database migration to finish");
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(error) => return Err(error),
        }
    };
    if !lease.requires_migration() {
        return Ok(lease);
    }
    progress("Backing up and preparing the OpenCode database");
    let (mut lease, backup) = tokio::task::spawn_blocking(move || {
        let backup = lease.prepare_migration()?;
        Ok::<_, anyhow::Error>((lease, backup))
    })
    .await??;
    if let Some(backup) = &backup {
        progress(&format!(
            "Backup saved to {}. Upgrading OpenCode history",
            backup.display()
        ));
    }
    let mut cmd = Command::new(&binary.path);
    crate::local::local_models::prepare_env(&mut cmd, None)?;
    cmd.current_dir(
        lease
            .path()
            .parent()
            .ok_or_else(|| anyhow!("OpenCode database has no parent"))?,
    )
    .env("OPENCODE_DB", lease.path())
    .env("OPENCODE_CONFIG_PROJECT_DISABLE", "1");
    let result = async {
        let (mut child, endpoint) = start_server(binary, cmd).await?;
        let migrated = wait_migration(&mut child, &endpoint, &mut progress).await;
        let stopped = child.kill().await;
        migrated?;
        stopped?;
        progress("Validating OpenCode conversation history");
        lease.complete_migration()
    }
    .await;
    if let Err(error) = result {
        let _ = lease.fail_migration(&error.to_string());
        if let Some(backup) = backup {
            return Err(anyhow!(
                "{error}. Database backup retained at {}",
                backup.display()
            ));
        }
        return Err(error);
    }
    Ok(lease)
}

async fn wait_migration(
    child: &mut Child,
    endpoint: &AgentEndpoint,
    progress: &mut (impl FnMut(&str) + Send),
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(anyhow!("OpenCode exited during migration ({status})"));
        }
        let status: serde_json::Value = endpoint
            .client
            .get(format!(
                "{}/api/experimental/migration/v1",
                endpoint.base_url
            ))
            .timeout(Duration::from_secs(5))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        match status["status"].as_str() {
            Some("completed") => return Ok(()),
            Some("error") => return Err(anyhow!("OpenCode migration failed: {}", status["error"])),
            Some("required" | "running") => {
                let label = status
                    .pointer("/progress/label")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Upgrading OpenCode history");
                if let (Some(done), Some(total)) = (
                    status
                        .pointer("/progress/numerator")
                        .and_then(serde_json::Value::as_u64),
                    status
                        .pointer("/progress/denominator")
                        .and_then(serde_json::Value::as_u64),
                ) {
                    progress(&format!("{label} ({done}/{total})"));
                } else {
                    progress(label);
                }
            }
            _ => {
                return Err(anyhow!(
                    "OpenCode returned an unknown migration status: {status}"
                ))
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(anyhow!(
                "OpenCode migration is taking longer than ten minutes. Retry to resume."
            ));
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn health_negotiates_v2_routes_without_masking_server_errors() {
        use axum::{http::StatusCode, routing::get, Router};

        for (protocol, status, health, ready, legacy) in [
            (Protocol::V1, 404, 200, true, false),
            (Protocol::V2, 200, 404, true, false),
            (Protocol::V2, 404, 200, true, true),
            (Protocol::V2, 401, 200, false, false),
            (Protocol::V2, 503, 200, false, false),
            (Protocol::V2, 404, 404, false, false),
        ] {
            let app = Router::new()
                .route(
                    "/api/status",
                    get(move || async move { StatusCode::from_u16(status).unwrap() }),
                )
                .route(
                    "/api/health",
                    get(move || async move { StatusCode::from_u16(health).unwrap() }),
                )
                .route("/global/health", get(|| async { StatusCode::OK }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let mut endpoint = AgentEndpoint {
                base_url: format!("http://{}", listener.local_addr().unwrap()),
                client: reqwest::Client::new(),
                protocol,
                legacy_v2_api: false,
            };
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            assert_eq!(endpoint.check_health().await.unwrap(), ready);
            assert_eq!(endpoint.legacy_v2_api, legacy);
            let prefix = if legacy { "/api" } else { "/api/experimental" };
            assert_eq!(
                endpoint.v2_export_path("ses_test"),
                format!("{prefix}/session/ses_test/export")
            );
            assert_eq!(endpoint.v2_generate_path(), format!("{prefix}/generate"));
            server.abort();
        }
    }

    #[tokio::test]
    async fn background_database_check_does_not_wait_for_a_turn() {
        let fixture = ProbeEnvironment::new().unwrap();
        let db = fixture.0.path().join("opencode.db");
        let _lease = DatabaseLease::acquire(&db, 2).unwrap();
        let binary = ResolvedBinary {
            path: fixture.0.path().join("must-not-launch"),
            version: "2.0.1".into(),
            protocol: Protocol::V2,
            modified: std::time::SystemTime::UNIX_EPOCH,
            size: 0,
        };
        let result = tokio::time::timeout(Duration::from_secs(5), prepare_database(&binary, &db))
            .await
            .expect("busy background checks must return promptly");
        assert!(result
            .err()
            .unwrap()
            .downcast_ref::<crate::local::native_store::opencode_database::DatabaseBusy>()
            .is_some());
    }

    #[test]
    fn supported_versions_and_isolated_probes() {
        assert_eq!(protocol_for_version("1.18.31").unwrap(), Protocol::V1);
        assert_eq!(
            protocol_for_version("opencode v2.0.1").unwrap(),
            Protocol::V2
        );
        assert_eq!(protocol_for_version("2.0.0-beta.1").unwrap(), Protocol::V2);
        assert!(protocol_for_version("0.0.0-next-unknown").is_err());
        assert!(protocol_for_version("3.0.0").is_err());
        let probe = ProbeEnvironment::new().unwrap();
        let mut cmd = Command::new("opencode");
        cmd.env("OPENCODE_DB", "/do-not-open/user.db");
        probe.configure(&mut cmd);
        let env: std::collections::HashMap<_, _> = cmd.as_std().get_envs().collect();
        assert_eq!(
            env[std::ffi::OsStr::new("OPENCODE_DB")],
            Some(std::ffi::OsStr::new(":memory:"))
        );
        for key in [
            "HOME",
            "XDG_DATA_HOME",
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
            "XDG_STATE_HOME",
        ] {
            assert!(Path::new(env[std::ffi::OsStr::new(key)].unwrap()).starts_with(probe.0.path()));
        }
    }
}
