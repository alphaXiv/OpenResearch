//! GitHub CLI calls for optional project publication.

use std::time::Duration;

use serde_json::Value;
use tokio::process::Command;

use crate::error::{anyhow, Result};

const UA: &str = concat!("orx/", env!("CARGO_PKG_VERSION"));
pub const SHALLOW_CLONE_THRESHOLD_KB: u64 = 250 * 1024;
const MAX_REPO_TOPICS: usize = 20;
const MAX_TOPIC_LEN: usize = 50;

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

pub struct RepoMeta {
    pub can_push: bool,
    pub archived: bool,
}

pub fn sanitize_topics(topics: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for topic in topics {
        if let Some(topic) = normalize_topic(topic) {
            if out.iter().any(|existing| existing == &topic) {
                continue;
            }
            out.push(topic);
            if out.len() >= MAX_REPO_TOPICS {
                break;
            }
        }
    }
    out
}

pub fn default_topics_for_project(paper_id: Option<&str>) -> Vec<String> {
    let mut topics = vec!["openresearch".to_string()];
    if let Some(id) = paper_id.and_then(normalize_arxiv_topic) {
        topics.push(id);
        topics.push("paper-repro".to_string());
    }
    topics
}

pub fn effective_project_topics(
    paper_id: Option<&str>,
    custom_topics: &[String],
    auto_topics_enabled: bool,
) -> Vec<String> {
    let mut topics = Vec::new();
    if auto_topics_enabled {
        topics.extend(default_topics_for_project(paper_id));
    }
    topics.extend(custom_topics.iter().cloned());
    sanitize_topics(&topics)
}

/// Publishes `topics` onto the repository, unioned with the topics it already
/// carries. Never clears the repository's topics, and skips the write when the
/// union changes nothing.
pub async fn set_repo_topics(owner: &str, repo: &str, topics: &[String]) -> Result<()> {
    // Read failures must abort rather than degrade to an empty set: PUT replaces
    // the whole list, so treating "could not read" as "has none" would delete the
    // topics this merge exists to protect.
    let existing = repo_topics(owner, repo).await?;
    let merged = merge_topics(&sanitize_topics(topics), &existing);
    if merged.is_empty() || topics_match(&existing, &merged) {
        // Never PUT an empty set (it clears the repo) and skip a no-op write.
        return Ok(());
    }
    let mut args = vec![
        "api".to_string(),
        "-X".to_string(),
        "PUT".to_string(),
        repository_endpoint(owner, repo),
    ];
    args[3].push_str("/topics");
    for topic in &merged {
        args.push("-f".to_string());
        args.push(format!("names[]={topic}"));
    }
    let borrowed = args.iter().map(String::as_str).collect::<Vec<_>>();
    gh(&borrowed, Duration::from_secs(20)).await?;
    Ok(())
}

/// Topics the repository currently carries. Errors propagate to the caller: a
/// merge computed against a silently-empty read would delete the real topics.
async fn repo_topics(owner: &str, repo: &str) -> Result<Vec<String>> {
    let endpoint = format!("{}/topics", repository_endpoint(owner, repo));
    let body = gh(
        &[
            "api",
            &endpoint,
            "-H",
            "Accept: application/vnd.github+json",
        ],
        Duration::from_secs(15),
    )
    .await?;
    let parsed: Value = serde_json::from_str(&body)?;
    Ok(parsed
        .get("names")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default())
}

/// Whether the repository already carries exactly this set of topics. Order is
/// deliberately ignored: GitHub need not echo our PUT order back, and an
/// order-sensitive check would re-PUT on every push.
fn topics_match(current: &[String], next: &[String]) -> bool {
    let mut current = sanitize_topics(current);
    let mut next = sanitize_topics(next);
    current.sort_unstable();
    next.sort_unstable();
    current == next
}

/// GitHub's topic PUT replaces the whole set, so merge instead of overwrite: a
/// repository shared by several projects — or one whose owner added topics by
/// hand — keeps everything it already had. Desired topics win the cap because
/// they are the ones this project is asking for.
fn merge_topics(desired: &[String], existing: &[String]) -> Vec<String> {
    let mut merged = sanitize_topics(desired);
    for topic in sanitize_topics(existing) {
        if merged.len() >= MAX_REPO_TOPICS {
            break;
        }
        if merged.contains(&topic) {
            continue;
        }
        merged.push(topic);
    }
    merged
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

fn normalize_topic(input: &str) -> Option<String> {
    let mut topic = input
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    while topic.contains("--") {
        topic = topic.replace("--", "-");
    }
    let topic = topic.trim_matches('-').to_string();
    if topic.is_empty() {
        return None;
    }
    let topic = if topic.len() > MAX_TOPIC_LEN {
        topic[..MAX_TOPIC_LEN].trim_matches('-').to_string()
    } else {
        topic
    };
    (!topic.is_empty()).then_some(topic)
}

fn normalize_arxiv_topic(paper_id: &str) -> Option<String> {
    let trimmed = paper_id.trim();
    // Pasted ids arrive as `arXiv:2401.12345` as often as the bare id; match the
    // prefix case-insensitively or the scheme word rides into the topic.
    let stripped = trimmed
        .get(..6)
        .filter(|prefix| prefix.eq_ignore_ascii_case("arxiv:"))
        .map_or(trimmed, |_| trimmed[6..].trim());
    let id = stripped.split('/').next_back().unwrap_or(stripped);
    let id = id.strip_suffix(".pdf").unwrap_or(id);
    let id = strip_arxiv_version(id);
    if id.is_empty() {
        // A blank or scheme-only id has nothing to derive a topic from.
        return None;
    }
    let id = id.replace('.', "-");
    normalize_topic(&format!("arxiv-{id}"))
}

/// Drops a trailing `vN` so every version of one paper maps to one topic —
/// otherwise `2401.12345v1` and `v2` land in separate search buckets.
///
/// Only a `v` followed by digits counts, and only when digits precede it, so an
/// old-style id like `hep-th/9901001` keeps its trailing `1`.
fn strip_arxiv_version(id: &str) -> &str {
    if let Some((head, tail)) = id.rsplit_once('v') {
        if !head.is_empty()
            && head.ends_with(|ch: char| ch.is_ascii_digit())
            && !tail.is_empty()
            && tail.chars().all(|ch| ch.is_ascii_digit())
        {
            return head;
        }
    }
    id
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
    fn github_api_errors_preserve_missing_and_collision_signals() {
        assert!(github_api_not_found("gh: Not Found (HTTP 404)"));
        assert!(!github_api_not_found(
            "gh: API rate limit exceeded (HTTP 403)"
        ));
        assert!(repository_name_exists(
            "GraphQL: Name already exists on this account"
        ));
    }

    #[test]
    fn topic_normalization_and_dedup_work() {
        let topics = sanitize_topics(&[
            " OpenResearch ".to_string(),
            "paper_repro".to_string(),
            "paper--repro".to_string(),
            "".to_string(),
        ]);
        assert_eq!(topics, vec!["openresearch", "paper-repro"]);
    }

    #[test]
    fn effective_topics_include_auto_and_custom() {
        let topics = effective_project_topics(
            Some("2401.12345v2"),
            &["Topic-A".to_string(), "paper-repro".to_string()],
            true,
        );
        // The version suffix is dropped, so v1/v2 share one searchable topic.
        assert_eq!(
            topics,
            vec!["openresearch", "arxiv-2401-12345", "paper-repro", "topic-a"]
        );
    }

    #[test]
    fn auto_topics_can_be_disabled_without_losing_extras() {
        let topics = effective_project_topics(Some("2401.12345"), &["llm".to_string()], false);
        assert_eq!(topics, vec!["llm"]);
    }

    #[test]
    fn arxiv_ids_normalize_across_input_shapes() {
        let topic = |id: &str| default_topics_for_project(Some(id));
        let expected = vec![
            "openresearch".to_string(),
            "arxiv-2401-12345".to_string(),
            "paper-repro".to_string(),
        ];
        for id in [
            "2401.12345",
            "2401.12345v2",
            "arXiv:2401.12345v12",
            "https://arxiv.org/abs/2401.12345",
            "2401.12345.pdf",
        ] {
            assert_eq!(topic(id), expected, "id {id} should normalize");
        }
    }

    #[test]
    fn old_style_and_malformed_ids_keep_their_signals() {
        // Old-style ids end in digits but carry no `vN`, so nothing is stripped.
        assert_eq!(strip_arxiv_version("hep-th/9901001"), "hep-th/9901001");
        assert_eq!(strip_arxiv_version("cs.CL/0701001"), "cs.CL/0701001");
        // A trailing `v` with no digits is part of the id, not a version.
        assert_eq!(strip_arxiv_version("2401.12345v"), "2401.12345v");
        // No arxiv id at all leaves the topic off entirely.
        assert_eq!(default_topics_for_project(None), vec!["openresearch"]);
        assert_eq!(
            default_topics_for_project(Some("   ")),
            vec!["openresearch"]
        );
    }

    #[test]
    fn topic_sets_compare_ignoring_order_and_spacing() {
        let stored = vec!["openresearch".to_string(), "paper-repro".to_string()];
        // Same set, different order: GitHub need not echo our PUT order back.
        let reordered = vec!["paper-repro".to_string(), "openresearch".to_string()];
        assert!(topics_match(&stored, &reordered));

        // Normalization-only differences also count as unchanged.
        assert!(topics_match(
            &stored,
            &[" Paper_Repro ".to_string(), "OpenResearch".to_string()]
        ));

        // A genuinely different set still writes.
        assert!(!topics_match(&stored, &["openresearch".to_string()]));
        assert!(!topics_match(
            &stored,
            &[
                "openresearch".to_string(),
                "paper-repro".to_string(),
                "llm".to_string()
            ]
        ));
    }

    #[test]
    fn merging_topics_preserves_what_the_repository_already_had() {
        let desired = vec!["openresearch".to_string(), "paper-repro".to_string()];
        let existing = vec!["hand-added".to_string(), "paper-repro".to_string()];
        assert_eq!(
            merge_topics(&desired, &existing),
            vec!["openresearch", "paper-repro", "hand-added"]
        );
    }

    #[test]
    fn merging_topics_preserves_other_projects_and_respects_the_cap() {
        let desired = vec!["a".to_string()];
        let existing = (0..30).map(|i| format!("topic-{i}")).collect::<Vec<_>>();
        let merged = merge_topics(&desired, &existing);
        assert_eq!(merged.len(), MAX_REPO_TOPICS);
        assert_eq!(merged[0], "a");
    }
}
