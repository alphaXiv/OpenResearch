//! GitHub CLI calls for optional project publication.

use std::time::Duration;

use serde_json::Value;
use tokio::process::Command;

use crate::error::{anyhow, Result};

const UA: &str = concat!("orx/", env!("CARGO_PKG_VERSION"));
pub const SHALLOW_CLONE_THRESHOLD_KB: u64 = 250 * 1024;

#[derive(Clone, Copy)]
pub struct Status {
    pub installed: bool,
    pub authenticated: bool,
}

pub fn should_shallow_clone(size_kb: Option<u64>) -> bool {
    size_kb.is_some_and(|size| size >= SHALLOW_CLONE_THRESHOLD_KB)
}

pub async fn status() -> Status {
    let installed = gh(&["--version"], Duration::from_secs(5)).await.is_ok();
    let authenticated = installed
        && gh(
            &["auth", "status", "--active", "--hostname", "github.com"],
            Duration::from_secs(10),
        )
        .await
        .is_ok();
    Status {
        installed,
        authenticated,
    }
}

async fn gh(args: &[&str], timeout: Duration) -> Result<String> {
    let mut command = match super::shell_env::find_on_path("gh") {
        Some(path) => Command::new(path),
        None => Command::new("gh"),
    };
    command
        .args(args)
        .env("GH_HOST", "github.com")
        .env("GH_PROMPT_DISABLED", "1")
        .kill_on_drop(true);
    if let Some(paths) = super::shell_env::search_path() {
        command.env("PATH", paths);
    }
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| {
            anyhow!(
                "gh {} timed out",
                args.first().copied().unwrap_or("command")
            )
        })?
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                anyhow!("GitHub CLI (`gh`) is required — install it from https://cli.github.com.")
            } else {
                anyhow!("Could not run GitHub CLI: {error}")
            }
        })?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(anyhow!(
            "gh {} failed: {}",
            args.first().copied().unwrap_or("command"),
            if detail.is_empty() {
                "unknown error"
            } else {
                &detail
            }
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn repository_candidate(repo: &str, suffix: usize) -> String {
    if suffix == 1 {
        repo.to_string()
    } else {
        format!("{repo}-{suffix}")
    }
}

fn repository_endpoint(owner: &str, repo: &str) -> String {
    format!(
        "repos/{}/{}",
        urlencoding::encode(owner),
        urlencoding::encode(repo)
    )
}

pub async fn create_project_repo(repo: &str) -> Result<(String, String)> {
    let owner = viewer_login().await?;
    for suffix in 1..=100 {
        let candidate = repository_candidate(repo, suffix);
        let name_with_owner = format!("{owner}/{candidate}");
        match gh(
            &["repo", "create", &name_with_owner, "--private"],
            Duration::from_secs(30),
        )
        .await
        {
            Ok(_) => return Ok((owner, candidate)),
            Err(error) if repository_name_exists(&error.to_string()) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(anyhow!(
        "Could not find an available GitHub repository name for '{repo}'."
    ))
}

pub async fn available_project_repo_name(repo: &str) -> Result<String> {
    let owner = viewer_login().await?;
    for suffix in 1..=100 {
        let candidate = repository_candidate(repo, suffix);
        if repo_meta(&owner, &candidate).await?.is_none() {
            return Ok(candidate);
        }
    }
    Err(anyhow!(
        "Could not find an available GitHub repository name for '{repo}'."
    ))
}

pub async fn public_repo_size_kb(url: &str) -> Option<u64> {
    let (owner, repo) = super::git::github_repository(url)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;
    let response = client
        .get(format!(
            "https://api.github.com/repos/{}/{}",
            urlencoding::encode(&owner),
            urlencoding::encode(&repo)
        ))
        .header("user-agent", UA)
        .header("accept", "application/vnd.github+json")
        .header("x-github-api-version", "2022-11-28")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body: Value = response.json().await.ok()?;
    body.get("size").and_then(Value::as_u64)
}

/// Fork a public GitHub repository under the currently authenticated account
/// without cloning it locally or adding a remote. Returns the fork's
/// `(owner, repo)`.
pub async fn fork_public_repo(url: &str) -> Result<(String, String)> {
    let (owner, repo) = super::git::github_repository(url)
        .ok_or_else(|| anyhow!("Expected a github.com URL like https://github.com/owner/repo."))?;
    // `gh repo fork` is idempotent: an existing fork is reused rather than
    // duplicated, so re-running against the same repo is safe.
    gh(
        &["repo", "fork", &format!("{owner}/{repo}")],
        Duration::from_secs(120),
    )
    .await?;
    let login = viewer_login().await?;
    Ok((login, repo))
}

/// Canonical `https://github.com/{owner}/{repo}` URL for an input repo URL.
pub fn canonical_repo_url(url: &str) -> Option<String> {
    let (owner, repo) = super::git::github_repository(url)?;
    Some(format!("https://github.com/{owner}/{repo}"))
}

pub struct RepoMeta {
    pub can_push: bool,
    pub archived: bool,
}

pub async fn viewer_login() -> Result<String> {
    gh(&["api", "user", "--jq", ".login"], Duration::from_secs(10))
        .await
        .and_then(|login| {
            if login.is_empty() {
                Err(anyhow!("GitHub CLI returned an empty account login."))
            } else {
                Ok(login)
            }
        })
}

pub async fn repo_meta(owner: &str, repo: &str) -> Result<Option<RepoMeta>> {
    let body = match gh(
        &["api", &repository_endpoint(owner, repo)],
        Duration::from_secs(10),
    )
    .await
    {
        Ok(body) => body,
        Err(error) if github_api_not_found(&error.to_string()) => return Ok(None),
        Err(error) => return Err(error),
    };
    parse_repo_meta(&body).map(Some)
}

fn parse_repo_meta(body: &str) -> Result<RepoMeta> {
    let body: Value = serde_json::from_str(body)
        .map_err(|error| anyhow!("Could not parse GitHub repository metadata: {error}"))?;
    Ok(RepoMeta {
        can_push: body
            .pointer("/permissions/push")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        archived: body
            .get("archived")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn github_api_not_found(error: &str) -> bool {
    error.contains("(HTTP 404)")
}

fn repository_name_exists(error: &str) -> bool {
    error
        .to_ascii_lowercase()
        .contains("name already exists on this account")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shallow_clone_is_reserved_for_large_repositories() {
        assert!(!should_shallow_clone(None));
        assert!(!should_shallow_clone(Some(SHALLOW_CLONE_THRESHOLD_KB - 1)));
        assert!(should_shallow_clone(Some(SHALLOW_CLONE_THRESHOLD_KB)));
    }

    #[test]
    fn repository_names_and_endpoints_are_safe() {
        assert_eq!(repository_candidate("project", 1), "project");
        assert_eq!(repository_candidate("project", 2), "project-2");
        assert_eq!(
            repository_endpoint("owner/name", "repo name"),
            "repos/owner%2Fname/repo%20name"
        );
    }

    #[test]
    fn repository_metadata_defaults_to_no_access() {
        let meta =
            parse_repo_meta(r#"{"permissions":{"push":true},"archived":false}"#).expect("metadata");
        assert!(meta.can_push);
        assert!(!meta.archived);

        let meta = parse_repo_meta("{}").expect("metadata");
        assert!(!meta.can_push);
        assert!(!meta.archived);
    }

    #[test]
    fn canonical_repo_url_normalizes_github_urls() {
        assert_eq!(
            canonical_repo_url("https://github.com/owner/repo.git"),
            Some("https://github.com/owner/repo".to_string())
        );
        assert_eq!(
            canonical_repo_url("git@github.com:owner/repo.git"),
            Some("https://github.com/owner/repo".to_string())
        );
        assert_eq!(
            canonical_repo_url("https://github.com/owner/repo/"),
            Some("https://github.com/owner/repo".to_string())
        );
        assert_eq!(canonical_repo_url("not a github url"), None);
    }

    #[test]
    fn github_api_errors_preserve_missing_and_collision_signals() {
        assert!(github_api_not_found("gh: Not Found (HTTP 404)"));
        assert!(!github_api_not_found(
            "gh: API rate limit exceeded (HTTP 403)"
        ));
        assert!(repository_name_exists(
            "GraphQL: Name already exists on this account"
        ));
    }
}
