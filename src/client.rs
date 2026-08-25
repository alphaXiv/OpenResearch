//! HTTP client for the OpenResearch API.
//!
//! JSON field names use `serde(rename_all = "camelCase")` so the wire
//! format matches the API exactly. The `request` helper surfaces errors as:
//!   - network failure  -> `Could not reach the API at {url}: ...`
//!   - HTTP 401         -> `Unauthorized — your token is invalid or revoked. Run `orx login` again.`
//!   - other non-2xx    -> `Request to {path} failed ({status} {reason}): {body}`
//!
//! All endpoint fns are `async` and take `&Credentials` as the first argument,
//! matching how commands call them.

use std::collections::HashMap;
use std::sync::OnceLock;

use reqwest::{Client, Method};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Credentials;
use crate::error::{anyhow, Result};

// ---------------------------------------------------------------------------
// Response DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Org {
    pub id: String,
    pub name: String,
    pub created_by: String,
}

/// Disk pricing for an offer. Mirrors the backend `zDisk` discriminated union,
/// keyed on the `sizable` bool: when `true`, `per_gb_hour` is set and the disk
/// bills per GB/hour; when `false`, `included_gb` is set and the offer bundles a
/// fixed capacity. Modeled as a flat struct with optional payloads rather than an
/// enum because serde's tagged enums can't key on a bool discriminator, and an
/// untagged enum would not apply the container's `camelCase` rename to variant
/// fields. The unused payload is simply `None`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Disk {
    pub sizable: bool,
    pub per_gb_hour: Option<f64>,
    pub included_gb: Option<f64>,
}

/// A single GPU offer from the compute catalog (`GET /compute/catalog`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuOffer {
    pub provider: String,
    pub offer_id: String,
    pub gpu: String,
    pub gpu_count: i64,
    /// Effective vCPUs allocated to the instance.
    pub vcpus: f64,
    /// System RAM in GB.
    pub ram_gb: f64,
    pub price_per_hour: f64,
    pub disk: Disk,
    pub region: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListCatalog {
    pub offers: Vec<GpuOffer>,
}

/// A single CPU-only offer from the CPU catalog (`GET /compute/catalog/cpu`).
/// Sibling to [`GpuOffer`]; CPU instances live in their own RunPod-only catalog.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuOffer {
    pub provider: String,
    pub offer_id: String,
    /// Flavor id: cpu5c (compute), cpu5g (general), or cpu5m (memory-optimized).
    pub cpu_flavor: String,
    /// Virtual CPUs allocated to the instance.
    pub vcpus: f64,
    /// System RAM in GB.
    pub ram_gb: f64,
    pub price_per_hour: f64,
    pub disk: Disk,
    pub region: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListCpuCatalog {
    pub offers: Vec<CpuOffer>,
}

// Thin envelope DTOs for the list endpoints.

#[derive(Debug, Clone, Deserialize)]
pub struct ListOrgs {
    pub orgs: Vec<Org>,
}

// ---------------------------------------------------------------------------
// Request bodies (mirroring the inline TS body shapes)
// ---------------------------------------------------------------------------

/// The `target` of a standalone instance (`POST /sandboxes`). Mirrors
/// the provider catalog's GPU and CPU variants.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SandboxTarget {
    /// Provision a fresh GPU instance.
    New {
        gpu: String,
        #[serde(rename = "gpuCount")]
        gpu_count: i64,
        #[serde(rename = "diskGb")]
        disk_gb: i64,
        /// Single lowercase word — same under camelCase, so no rename needed.
        /// Omitted from the payload when `None`.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
    },
    /// Provision a fresh CPU-only instance.
    #[serde(rename = "new-cpu")]
    NewCpu {
        #[serde(rename = "cpuFlavor")]
        cpu_flavor: String,
        #[serde(rename = "vcpuCount")]
        vcpu_count: i64,
    },
}

/// Body of `POST /sandboxes`. `projectId` is intentionally omitted — the server
/// rejects it for `new`/`new-cpu` (those are org-level standalone only).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSandboxBody {
    pub organization_id: String,
    pub target: SandboxTarget,
}

/// A sandbox as returned by `POST /sandboxes`. Mirrors the API's `zSandbox`;
/// fields are nullable while a box is still provisioning.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sandbox {
    pub id: String,
    pub organization_id: String,
    pub project_id: Option<String>,
    pub ssh_hostname: Option<String>,
    pub ssh_port: Option<i64>,
    pub ssh_username: Option<String>,
    pub status: String,
    pub machine_type: String,
    pub created_by: Option<String>,
    pub updated_at: String,
    pub provision_warnings: Option<String>,
    #[serde(default)]
    pub failure_code: Option<String>,
    #[serde(default)]
    pub failure_message: Option<String>,
    #[serde(default)]
    pub failed_at: Option<String>,
    pub provider_name: Option<String>,
    pub provider_instance_id: Option<String>,
    pub price_per_hour: Option<f64>,
    pub gpu: Option<String>,
    pub gpu_count: Option<i64>,
    pub vcpu_count: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SandboxEnvelope {
    pub sandbox: Sandbox,
}

/// `GET /sandboxes` — each row is a `Sandbox` (the extra `connections` the
/// dashboard renders is ignored on deserialize).
#[derive(Debug, Clone, Deserialize)]
pub struct ListSandboxes {
    pub sandboxes: Vec<Sandbox>,
}

/// A registered SSH public key (`zSshKey`, secrets-free). `public_key` is the
/// raw OpenSSH line, so the CLI can tell whether this machine holds the
/// matching private half.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshKey {
    pub id: String,
    pub name: String,
    pub public_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSshKeys {
    pub ssh_keys: Vec<SshKey>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshKeyEnvelope {
    pub ssh_key: SshKey,
}

// ---------------------------------------------------------------------------
// Core request helper — preserves TS error semantics exactly.
// ---------------------------------------------------------------------------

fn http() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(Client::new)
}

/// Sends a request and returns the response after applying the shared error
/// semantics (network failure, 401, other non-2xx). Body decoding is left to
/// the caller so both JSON-decoding and no-content endpoints can share this.
///
/// `body` is `None` for GET requests (no `content-type` header sent), or
/// `Some(json)` for a JSON request body, matching the TS `init` shape.
async fn send_request(
    creds: &Credentials,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<reqwest::Response> {
    let url = format!("{}{}", creds.api_url, path);
    let mut req = http().request(method, &url).bearer_auth(&creds.token);
    if let Some(ref b) = body {
        req = req.header("content-type", "application/json").json(b);
    }

    let res = match req.send().await {
        Ok(res) => res,
        Err(err) => {
            return Err(anyhow!(
                "Could not reach the API at {}: {}",
                creds.api_url,
                err
            ));
        }
    };

    let status = res.status();
    if status.as_u16() == 401 {
        return Err(anyhow!(
            "Unauthorized — your token is invalid or revoked. Run `orx login` again."
        ));
    }
    if !status.is_success() {
        let reason = status.canonical_reason().unwrap_or("");
        let detail = res.text().await.unwrap_or_default();
        let suffix = if detail.is_empty() {
            String::new()
        } else {
            format!(": {}", detail)
        };
        return Err(anyhow!(
            "Request to {} failed ({} {}){}",
            path,
            status.as_u16(),
            reason,
            suffix
        ));
    }

    Ok(res)
}

/// Issues a request and decodes the JSON body into `T`.
async fn request<T: DeserializeOwned>(
    creds: &Credentials,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<T> {
    let res = send_request(creds, method, path, body).await?;
    let parsed = res.json::<T>().await?;
    Ok(parsed)
}

/// Issues a request that returns no body (e.g. `204 No Content`).
async fn request_no_content(
    creds: &Credentials,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<()> {
    send_request(creds, method, path, body).await?;
    Ok(())
}

async fn api_get<T: DeserializeOwned>(creds: &Credentials, path: &str) -> Result<T> {
    request(creds, Method::GET, path, None).await
}

async fn api_post<T: DeserializeOwned>(creds: &Credentials, path: &str, body: Value) -> Result<T> {
    request(creds, Method::POST, path, Some(body)).await
}

// ---------------------------------------------------------------------------
// Endpoint fns (one per TS export, same path/method/shape)
// ---------------------------------------------------------------------------

pub async fn list_orgs(creds: &Credentials) -> Result<ListOrgs> {
    api_get(creds, "/orgs").await
}

pub async fn list_catalog(creds: &Credentials) -> Result<ListCatalog> {
    api_get(creds, "/compute/catalog").await
}

pub async fn list_cpu_catalog(creds: &Credentials) -> Result<ListCpuCatalog> {
    api_get(creds, "/compute/catalog/cpu").await
}

/// Spin up a standalone instance in an org (no experiment) — `POST /sandboxes`.
pub async fn create_sandbox(
    creds: &Credentials,
    body: &CreateSandboxBody,
) -> Result<SandboxEnvelope> {
    let body = serde_json::to_value(body)?;
    api_post(creds, "/sandboxes", body).await
}

/// One box's provisioning state / SSH target — `GET /sandboxes/{id}`. The
/// openresearch backend polls this while its box goes provisioning → online.
pub async fn get_sandbox(creds: &Credentials, sandbox_id: &str) -> Result<SandboxEnvelope> {
    api_get(creds, &format!("/sandboxes/{}", sandbox_id)).await
}

/// Tear a box down (destroys the provider instance) — `DELETE /sandboxes/{id}`.
pub async fn delete_sandbox(creds: &Credentials, sandbox_id: &str) -> Result<()> {
    request_no_content(
        creds,
        Method::DELETE,
        &format!("/sandboxes/{}", sandbox_id),
        None,
    )
    .await
}

/// Every sandbox in an org (project-scoped + standalone) — `GET /sandboxes`.
pub async fn list_sandboxes(creds: &Credentials, org_id: &str) -> Result<ListSandboxes> {
    api_get(creds, &format!("/sandboxes?organizationId={}", org_id)).await
}

/// The user's registered SSH public keys — `GET /ssh-keys`. Boxes authorize
/// org members' registered keys, so an empty list means an unreachable box.
pub async fn list_ssh_keys(creds: &Credentials) -> Result<ListSshKeys> {
    api_get(creds, "/ssh-keys").await
}

/// Register a public key on the account — `POST /ssh-keys`. The api pushes it to
/// every live box in the user's orgs, so an already-running box becomes
/// reachable without a restart.
pub async fn create_ssh_key(
    creds: &Credentials,
    name: &str,
    public_key: &str,
) -> Result<SshKeyEnvelope> {
    api_post(
        creds,
        "/ssh-keys",
        serde_json::json!({ "name": name, "publicKey": public_key }),
    )
    .await
}

// ---------------------------------------------------------------------------
// alphaXiv literature endpoints (public — no auth, different hosts).
//
// These do NOT go through `send_request`/`Credentials`: they hit alphaXiv's
// public API/web hosts and require no token, so discovery and paper reading work
// even without `orx login`. They keep their own (simpler) error semantics and
// translate a 404 into `Ok(None)` where "not generated yet" is a normal answer.
// ---------------------------------------------------------------------------

/// Sent on external requests — some CDNs reject the default (empty) UA.
const ALPHAXIV_UA: &str = concat!("openresearch-cli/", env!("CARGO_PKG_VERSION"));

/// One alphaXiv full-text or discovery search hit. Serialize is derived so the
/// CLI can emit endpoint results verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperHit {
    pub paper_id: String,
    pub title: String,
    #[serde(rename = "abstract", default)]
    pub abstract_: String,
    #[serde(default)]
    pub publication_date: Option<String>,
    #[serde(default)]
    pub votes: i64,
    #[serde(default)]
    pub snippets: Vec<PaperSnippet>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperSnippet {
    #[serde(default)]
    pub page_number: i64,
    pub snippet: String,
}

#[derive(Clone, Copy)]
pub struct PaperDiscoveryOptions<'a> {
    pub published_after: Option<&'a str>,
    pub published_before: Option<&'a str>,
    pub prioritize: &'a str,
}

#[derive(Clone, Copy)]
pub struct OpenAlexDiscoveryOptions<'a> {
    pub limit: u32,
    pub published_after: Option<&'a str>,
    pub published_before: Option<&'a str>,
    pub prioritize: &'a str,
    pub source_filter: Option<&'a str>,
}

fn paper_discovery_url(
    base: &str,
    strategy: &str,
    query: &str,
    options: PaperDiscoveryOptions<'_>,
) -> Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(&format!("{base}/search/v2/paper/discover/{strategy}"))?;
    {
        let mut params = url.query_pairs_mut();
        params.append_pair("q", query);
        params.append_pair("prioritize", options.prioritize);
        if let Some(date) = options.published_after {
            params.append_pair("publishedAfter", date);
        }
        if let Some(date) = options.published_before {
            params.append_pair("publishedBefore", date);
        }
    }
    Ok(url)
}

async fn discover_papers(
    strategy: &str,
    query: &str,
    options: PaperDiscoveryOptions<'_>,
) -> Result<Vec<PaperHit>> {
    let base = crate::config::alphaxiv_api_url();
    let url = paper_discovery_url(&base, strategy, query, options)?;
    let res = http()
        .get(url)
        .header("user-agent", ALPHAXIV_UA)
        .send()
        .await
        .map_err(|e| anyhow!("Could not reach alphaXiv at {}: {}", base, e))?;
    let status = res.status();
    if !status.is_success() {
        let reason = status.canonical_reason().unwrap_or("");
        return Err(anyhow!(
            "alphaXiv {} retrieval failed ({} {})",
            strategy,
            status.as_u16(),
            reason
        ));
    }
    Ok(res.json::<Vec<PaperHit>>().await?)
}

pub async fn discover_papers_by_keyword(
    query: &str,
    options: PaperDiscoveryOptions<'_>,
) -> Result<Vec<PaperHit>> {
    discover_papers("keyword", query, options).await
}

pub async fn discover_papers_by_embedding(
    query: &str,
    options: PaperDiscoveryOptions<'_>,
) -> Result<Vec<PaperHit>> {
    discover_papers("embedding", query, options).await
}

/// `2401.12345v2` → `2401.12345`; alphaXiv lookups want the versionless id.
pub(crate) fn versionless_id(paper_id: &str) -> &str {
    paper_id
        .rfind('v')
        .filter(|&i| i > 0 && !paper_id[i + 1..].is_empty())
        .filter(|&i| paper_id[i + 1..].chars().all(|c| c.is_ascii_digit()))
        .map_or(paper_id, |i| &paper_id[..i])
}

/// One hit from the fast (Google-backed) paper search — the endpoint built for
/// title lookups, versus the BM25 discovery primitive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FastPaperHit {
    pub paper_id: String,
    pub title: String,
    #[serde(default)]
    pub snippet: Option<String>,
}

/// Title/keyword paper search (`GET /search/v2/paper/fast`).
pub async fn search_papers_fast(query: &str) -> Result<Vec<FastPaperHit>> {
    let base = crate::config::alphaxiv_api_url();
    let url = format!(
        "{}/search/v2/paper/fast?q={}&includePrivate=false",
        base,
        urlencoding::encode(query)
    );
    let res = http()
        .get(&url)
        .header("user-agent", ALPHAXIV_UA)
        .send()
        .await
        .map_err(|e| anyhow!("Could not reach alphaXiv at {}: {}", base, e))?;
    let status = res.status();
    if !status.is_success() {
        let reason = status.canonical_reason().unwrap_or("");
        return Err(anyhow!(
            "alphaXiv search failed ({} {})",
            status.as_u16(),
            reason
        ));
    }
    Ok(res.json::<Vec<FastPaperHit>>().await?)
}

/// A paper resolved for the "start from a paper" project flow.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPaper {
    /// Canonical versionless id (`2401.12345`).
    pub paper_id: String,
    pub title: Option<String>,
    /// Linked GitHub repo — author repos first, then most stars.
    pub repo_url: Option<String>,
    pub repo_stars: Option<i64>,
}

/// Resolve an arXiv id to title + linked GitHub repo. `/papers/v3/{id}` scrapes
/// arXiv on a miss, so brand-new papers resolve too (their repo links may lag).
/// The implementations lookup is best-effort — a failure there just means no repo.
pub async fn resolve_paper(paper_id: &str) -> Result<ResolvedPaper> {
    let id = versionless_id(paper_id);
    let base = crate::config::alphaxiv_api_url();
    let url = format!("{}/papers/v3/{}", base, urlencoding::encode(id));
    let res = http()
        .get(&url)
        .header("user-agent", ALPHAXIV_UA)
        .send()
        .await
        .map_err(|e| anyhow!("Could not reach alphaXiv at {}: {}", base, e))?;
    let status = res.status();
    if !status.is_success() {
        return Err(anyhow!(
            "Paper {} not found on alphaXiv/arXiv ({})",
            id,
            status.as_u16()
        ));
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Paper {
        group_id: Option<String>,
        universal_id: Option<String>,
        title: Option<String>,
    }
    let paper = res.json::<Paper>().await?;
    let mut resolved = ResolvedPaper {
        paper_id: paper.universal_id.unwrap_or_else(|| id.to_string()),
        title: paper.title,
        repo_url: None,
        repo_stars: None,
    };
    let Some(group_id) = paper.group_id.filter(|g| !g.is_empty()) else {
        return Ok(resolved);
    };

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Implementations {
        #[serde(default)]
        paper_resources: Vec<Resource>,
        #[serde(default)]
        alpha_xiv_implementations: Vec<Resource>,
    }
    #[derive(Deserialize)]
    struct Resource {
        #[serde(rename = "type")]
        kind: Option<String>,
        url: Option<String>,
        #[serde(default)]
        stars: Option<i64>,
        #[serde(default)]
        source: Option<String>,
    }
    let url = format!(
        "{}/papers/v3/{}/implementations",
        base,
        urlencoding::encode(&group_id)
    );
    let impls = match http()
        .get(&url)
        .header("user-agent", ALPHAXIV_UA)
        .send()
        .await
    {
        Ok(res) if res.status().is_success() => match res.json::<Implementations>().await {
            Ok(body) => body,
            Err(_) => return Ok(resolved),
        },
        _ => return Ok(resolved),
    };

    let is_github = |r: &&Resource| {
        r.kind.as_deref() == Some("github") && r.url.as_deref().is_some_and(|u| !u.is_empty())
    };
    let best = impls
        .paper_resources
        .iter()
        .filter(is_github)
        // Author repos beat community ones; stars break ties.
        .max_by_key(|r| (r.source.as_deref() == Some("author"), r.stars.unwrap_or(0)))
        .or_else(|| impls.alpha_xiv_implementations.iter().find(is_github));
    if let Some(repo) = best {
        resolved.repo_url = repo.url.clone();
        resolved.repo_stars = repo.stars;
    }
    Ok(resolved)
}

/// A declared length past this is not a paper; arXiv's own submission limit is
/// far below it.
const MAX_PAPER_PDF_BYTES: u64 = 64 * 1024 * 1024;

/// `export.arxiv.org` is the host arXiv asks automated clients to use. Old-style
/// ids (`hep-th/9901001`) carry a slash, so each segment is encoded separately.
fn paper_pdf_url(paper_id: &str) -> String {
    let path = versionless_id(paper_id)
        .split('/')
        .map(|segment| urlencoding::encode(segment).into_owned())
        .collect::<Vec<_>>()
        .join("/");
    format!("https://export.arxiv.org/pdf/{path}")
}

/// Download a paper's PDF, for paper projects that start blank because the
/// paper has no linked public code repository.
pub async fn fetch_paper_pdf(paper_id: &str) -> Result<Vec<u8>> {
    // The id reaches here straight from the request body; `..` would walk to
    // another paper's PDF on the same host.
    if paper_id
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return Err(anyhow!("{} is not an arXiv id", paper_id));
    }
    let url = paper_pdf_url(paper_id);
    let res = http()
        .get(&url)
        .header("user-agent", ALPHAXIV_UA)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| anyhow!("Could not reach arXiv at {}: {}", url, e))?;
    let status = res.status();
    if !status.is_success() {
        let reason = status.canonical_reason().unwrap_or("");
        return Err(anyhow!(
            "arXiv PDF download failed ({} {})",
            status.as_u16(),
            reason
        ));
    }
    // arXiv answers some unknown ids with an HTML page rather than a 404.
    let is_pdf = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/pdf"));
    if !is_pdf {
        return Err(anyhow!("{} did not return a PDF", url));
    }
    if res
        .content_length()
        .is_some_and(|len| len > MAX_PAPER_PDF_BYTES)
    {
        return Err(anyhow!("{} is too large to download", url));
    }
    Ok(res.bytes().await?.to_vec())
}

/// Look up a paper's linked GitHub repository (the most-starred repo associated
/// with it on alphaXiv). Returns `Ok(None)` when the paper has no linked repo or
/// isn't known to alphaXiv. Best-effort metadata — callers shouldn't fail on it.
pub async fn fetch_paper_github(paper_id: &str) -> Result<Option<String>> {
    // The feed lookup wants a versionless universal id (`2401.12345`, not `2401.12345v2`).
    let versionless = versionless_id(paper_id);
    let base = crate::config::alphaxiv_api_url();
    let url = format!(
        "{}/papers/v3/feed?universalId={}&pageNum=0&pageSize=1&sort=Hot&interval=All%20time",
        base,
        urlencoding::encode(versionless)
    );
    let res = http()
        .get(&url)
        .header("user-agent", ALPHAXIV_UA)
        .send()
        .await
        .map_err(|e| anyhow!("Could not reach alphaXiv at {}: {}", base, e))?;
    let status = res.status();
    if !status.is_success() {
        let reason = status.canonical_reason().unwrap_or("");
        return Err(anyhow!(
            "alphaXiv paper lookup failed ({} {})",
            status.as_u16(),
            reason
        ));
    }

    #[derive(Deserialize)]
    struct FeedResponse {
        papers: Vec<FeedPaper>,
    }
    #[derive(Deserialize)]
    struct FeedPaper {
        github_url: Option<String>,
    }

    let body = res.json::<FeedResponse>().await?;
    Ok(body.papers.into_iter().next().and_then(|p| p.github_url))
}

/// Fetch one of a paper's markdown documents from the alphaXiv web app.
/// `kind` is `"overview"` (the machine-readable report) or `"abs"` (full text).
/// Returns `Ok(None)` on 404 — i.e. that document hasn't been generated yet.
pub async fn fetch_paper_markdown(kind: &str, paper_id: &str) -> Result<Option<String>> {
    let base = crate::config::alphaxiv_web_url();
    let url = format!("{}/{}/{}.md", base, kind, paper_id);
    let res = http()
        .get(&url)
        .header("user-agent", ALPHAXIV_UA)
        .send()
        .await
        .map_err(|e| anyhow!("Could not reach alphaXiv at {}: {}", base, e))?;
    let status = res.status();
    if status.as_u16() == 404 {
        return Ok(None);
    }
    if !status.is_success() {
        let reason = status.canonical_reason().unwrap_or("");
        return Err(anyhow!(
            "alphaXiv request for {} failed ({} {})",
            url,
            status.as_u16(),
            reason
        ));
    }
    Ok(Some(res.text().await?))
}

// ---------------------------------------------------------------------------
// Unified literature hit + OpenAlex / bioRxiv sources.
//
// Discovery returns a uniform list and `orx paper` fetches one paper. Like the
// alphaXiv block above, these hit public hosts with
// no token and keep their own light error semantics. bioRxiv has no search API,
// so `--source biorxiv` searches OpenAlex filtered to bioRxiv's source and
// bioRxiv's own API is used only to fetch a preprint by DOI.
// ---------------------------------------------------------------------------

/// OpenAlex source id for the bioRxiv repository — `--source biorxiv` filters to it.
pub const BIORXIV_SOURCE_ID: &str = "S4306402567";

/// A single discovery hit, uniform across sources. Per-source-only fields
/// (`votes`, `citations`, `snippets`) are omitted when empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LitHit {
    /// `"alphaxiv" | "openalex" | "biorxiv"` — set by the search fn.
    pub source: String,
    /// Self-routing id for `orx paper`: an arXiv id, a DOI, or an OpenAlex `W…` id.
    pub id: String,
    pub title: String,
    #[serde(rename = "abstract", default)]
    pub abstract_: String,
    #[serde(default)]
    pub publication_date: Option<String>,
    /// alphaXiv community votes; `None` for other sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub votes: Option<i64>,
    /// Citation count (OpenAlex); `None` for alphaXiv.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citations: Option<i64>,
    /// Matched full-text snippets; alphaXiv only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub snippets: Vec<PaperSnippet>,
}

impl From<PaperHit> for LitHit {
    fn from(h: PaperHit) -> Self {
        LitHit {
            source: "alphaxiv".to_string(),
            id: h.paper_id,
            title: h.title,
            abstract_: h.abstract_,
            publication_date: h.publication_date,
            votes: Some(h.votes),
            citations: None,
            snippets: h.snippets,
        }
    }
}

/// `https://openalex.org/W123` (or any `.../W123`) → `W123`.
fn strip_openalex_prefix(id: &str) -> &str {
    id.rsplit('/').next().unwrap_or(id)
}

/// `https://doi.org/10.1101/x` / `doi:10.1101/x` → `10.1101/x`.
fn strip_doi_prefix(doi: &str) -> &str {
    doi.strip_prefix("https://doi.org/")
        .or_else(|| doi.strip_prefix("http://doi.org/"))
        .or_else(|| doi.strip_prefix("doi:"))
        .unwrap_or(doi)
}

/// Rebuild abstract text from OpenAlex's `abstract_inverted_index` (token →
/// positions). Returns `""` when the index is absent (OpenAlex omits abstracts
/// for some works).
fn reconstruct_abstract(index: &Option<HashMap<String, Vec<i64>>>) -> String {
    let Some(index) = index else {
        return String::new();
    };
    let mut positioned: Vec<(i64, &str)> = Vec::new();
    for (token, positions) in index {
        for &p in positions {
            positioned.push((p, token.as_str()));
        }
    }
    positioned.sort_by_key(|(p, _)| *p);
    positioned
        .into_iter()
        .map(|(_, t)| t)
        .collect::<Vec<_>>()
        .join(" ")
}

// OpenAlex serves snake_case JSON, so the Rust field names match the wire
// directly — no `rename_all` here (unlike the OpenResearch API DTOs above).
#[derive(Debug, Clone, Deserialize)]
struct OaAuthor {
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct OaAuthorship {
    #[serde(default)]
    author: Option<OaAuthor>,
}

#[derive(Debug, Clone, Deserialize)]
struct OaLocation {
    #[serde(default)]
    pdf_url: Option<String>,
    #[serde(default)]
    landing_page_url: Option<String>,
}

/// One OpenAlex work — used both for `/works` search rows (populated per the
/// `select` list) and for a single-work fetch (all fields present). Unselected
/// fields simply default.
#[derive(Debug, Clone, Deserialize)]
pub struct OpenAlexWork {
    // The three fields `paper.rs` reads straight off the struct stay `pub`; the
    // rest are reached through the accessor methods below.
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    doi: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub publication_date: Option<String>,
    #[serde(default)]
    pub cited_by_count: Option<i64>,
    #[serde(default)]
    abstract_inverted_index: Option<HashMap<String, Vec<i64>>>,
    #[serde(default)]
    authorships: Vec<OaAuthorship>,
    #[serde(default)]
    best_oa_location: Option<OaLocation>,
}

impl OpenAlexWork {
    /// Abstract text reconstructed from the inverted index (`""` when absent).
    pub fn abstract_text(&self) -> String {
        reconstruct_abstract(&self.abstract_inverted_index)
    }

    /// Author display names, in order.
    pub fn author_names(&self) -> Vec<String> {
        self.authorships
            .iter()
            .filter_map(|a| a.author.as_ref()?.display_name.clone())
            .collect()
    }

    /// Bare DOI (`10.…`) when the work has one.
    pub fn doi_bare(&self) -> Option<String> {
        self.doi.as_deref().map(|d| strip_doi_prefix(d).to_string())
    }

    /// Bare OpenAlex work id (`W…`) when present.
    pub fn work_id(&self) -> Option<String> {
        self.id
            .as_deref()
            .map(|i| strip_openalex_prefix(i).to_string())
    }

    /// A readable open-access URL: PDF if OpenAlex has one, else the landing page.
    pub fn oa_url(&self) -> Option<String> {
        self.best_oa_location
            .as_ref()
            .and_then(|l| l.pdf_url.clone().or_else(|| l.landing_page_url.clone()))
    }

    /// Best self-routing id for `orx paper`. For bioRxiv-sourced hits always the
    /// DOI (routes back to the richer bioRxiv fetch); otherwise the DOI when
    /// present, else the bare OpenAlex work id.
    fn routing_id(&self, prefer_doi: bool) -> String {
        let doi = self.doi_bare();
        if prefer_doi {
            if let Some(d) = doi {
                return d;
            }
        }
        doi.or_else(|| self.work_id()).unwrap_or_default()
    }

    fn into_lit_hit(self, biorxiv: bool) -> LitHit {
        let id = self.routing_id(biorxiv);
        let abstract_ = self.abstract_text();
        LitHit {
            source: if biorxiv { "biorxiv" } else { "openalex" }.to_string(),
            id,
            title: self.title.unwrap_or_default(),
            abstract_,
            publication_date: self.publication_date,
            votes: None,
            citations: self.cited_by_count,
            snippets: Vec::new(),
        }
    }
}

/// Fields to request from OpenAlex `/works` search — keeps the payload small.
const OPENALEX_SELECT: &str =
    "id,doi,title,publication_date,cited_by_count,abstract_inverted_index";

fn openalex_discovery_url(
    base: &str,
    query: &str,
    mailto: &str,
    options: OpenAlexDiscoveryOptions<'_>,
) -> Result<reqwest::Url> {
    // Preserve relevance by reranking a broader page instead of sorting the whole corpus by date.
    let fetch_limit = if options.prioritize == "default" {
        options.limit
    } else {
        options.limit.saturating_mul(4).max(50)
    };
    let mut url = reqwest::Url::parse(&format!("{base}/works"))?;
    {
        let mut params = url.query_pairs_mut();
        params.append_pair("search", query);
        params.append_pair("per_page", &fetch_limit.clamp(1, 200).to_string());
        params.append_pair("mailto", mailto);
        params.append_pair("select", OPENALEX_SELECT);

        let mut filters = Vec::new();
        if let Some(source) = options.source_filter {
            filters.push(format!("primary_location.source.id:{source}"));
        }
        if let Some(date) = options.published_after {
            filters.push(format!("from_publication_date:{date}"));
        }
        if let Some(date) = options.published_before {
            filters.push(format!("to_publication_date:{date}"));
        }
        if !filters.is_empty() {
            params.append_pair("filter", &filters.join(","));
        }
    }
    Ok(url)
}

fn rerank_openalex_works(works: &mut [OpenAlexWork], prioritize: &str) {
    match prioritize {
        "recency" => works.sort_by(|a, b| match (&a.publication_date, &b.publication_date) {
            (Some(a), Some(b)) => b.cmp(a),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }),
        "historical" => works.sort_by(|a, b| match (&a.publication_date, &b.publication_date) {
            (Some(a), Some(b)) => a.cmp(b),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }),
        "popular" => works.sort_by(|a, b| {
            b.cited_by_count
                .unwrap_or_default()
                .cmp(&a.cited_by_count.unwrap_or_default())
        }),
        _ => {}
    }
}

/// Search OpenAlex works, optionally restricted to a source such as bioRxiv.
pub async fn discover_openalex(
    query: &str,
    options: OpenAlexDiscoveryOptions<'_>,
) -> Result<Vec<LitHit>> {
    let base = crate::config::openalex_api_url();
    let url = openalex_discovery_url(&base, query, &crate::config::openalex_mailto(), options)?;
    let res = http()
        .get(url)
        .header("user-agent", ALPHAXIV_UA)
        .send()
        .await
        .map_err(|e| anyhow!("Could not reach OpenAlex at {}: {}", base, e))?;
    let status = res.status();
    if !status.is_success() {
        let reason = status.canonical_reason().unwrap_or("");
        return Err(anyhow!(
            "OpenAlex search failed ({} {})",
            status.as_u16(),
            reason
        ));
    }

    #[derive(Deserialize)]
    struct WorksResponse {
        #[serde(default)]
        results: Vec<OpenAlexWork>,
    }
    let biorxiv = options.source_filter == Some(BIORXIV_SOURCE_ID);
    let mut body = res.json::<WorksResponse>().await?;
    rerank_openalex_works(&mut body.results, options.prioritize);
    Ok(body
        .results
        .into_iter()
        .take(options.limit as usize)
        .map(|w| w.into_lit_hit(biorxiv))
        .collect())
}

/// The `/works/{id}` selector for a work fetched by id or DOI. A DOI (bare,
/// `doi:`-prefixed, or a `doi.org` URL) becomes OpenAlex's `doi:<doi>` form
/// (slashes kept literal); anything else is treated as a bare `W…` work id.
fn openalex_selector(input: &str) -> String {
    let bare = strip_doi_prefix(input.trim());
    if bare.starts_with("10.") {
        return format!("doi:{}", bare);
    }
    strip_openalex_prefix(bare).to_string()
}

/// Fetch a single OpenAlex work by its `W…` id or a DOI. Returns `Ok(None)` on
/// 404 (unknown id) — a normal "not found" answer.
pub async fn fetch_openalex_work(id_or_doi: &str) -> Result<Option<OpenAlexWork>> {
    let base = crate::config::openalex_api_url();
    let url = format!(
        "{}/works/{}?mailto={}",
        base,
        openalex_selector(id_or_doi),
        urlencoding::encode(&crate::config::openalex_mailto()),
    );
    let res = http()
        .get(&url)
        .header("user-agent", ALPHAXIV_UA)
        .send()
        .await
        .map_err(|e| anyhow!("Could not reach OpenAlex at {}: {}", base, e))?;
    let status = res.status();
    if status.as_u16() == 404 {
        return Ok(None);
    }
    if !status.is_success() {
        let reason = status.canonical_reason().unwrap_or("");
        return Err(anyhow!(
            "OpenAlex lookup failed ({} {})",
            status.as_u16(),
            reason
        ));
    }
    Ok(Some(res.json::<OpenAlexWork>().await?))
}

/// One version row from the bioRxiv details API. `authors` is a single
/// semicolon-delimited string; `published` is the peer-reviewed DOI or the
/// literal string `"NA"`.
#[derive(Debug, Clone, Deserialize)]
pub struct BiorxivDetail {
    #[serde(default)]
    pub doi: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub authors: String,
    #[serde(rename = "abstract", default)]
    pub abstract_: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub published: String,
}

/// Fetch a bioRxiv preprint's metadata by DOI (`10.1101/…`). The details
/// endpoint lists every version oldest→newest; this returns the latest, or
/// `Ok(None)` when bioRxiv knows no such preprint (200 with an empty collection).
pub async fn fetch_biorxiv(doi: &str) -> Result<Option<BiorxivDetail>> {
    let base = crate::config::biorxiv_api_url();
    let url = format!("{}/details/biorxiv/{}/na/json", base, doi.trim());
    let res = http()
        .get(&url)
        .header("user-agent", ALPHAXIV_UA)
        .send()
        .await
        .map_err(|e| anyhow!("Could not reach bioRxiv at {}: {}", base, e))?;
    let status = res.status();
    if status.as_u16() == 404 {
        return Ok(None);
    }
    if !status.is_success() {
        let reason = status.canonical_reason().unwrap_or("");
        return Err(anyhow!(
            "bioRxiv lookup failed ({} {})",
            status.as_u16(),
            reason
        ));
    }

    #[derive(Deserialize)]
    struct DetailsResponse {
        #[serde(default)]
        collection: Vec<BiorxivDetail>,
    }
    let body = res.json::<DetailsResponse>().await?;
    Ok(body.collection.into_iter().last())
}

#[cfg(test)]
mod tests {
    use super::{
        openalex_discovery_url, openalex_selector, paper_discovery_url, reconstruct_abstract,
        rerank_openalex_works, CreateSandboxBody, ListCatalog, ListCpuCatalog, LitHit,
        OpenAlexDiscoveryOptions, OpenAlexWork, PaperDiscoveryOptions, PaperHit, SandboxEnvelope,
        SandboxTarget, BIORXIV_SOURCE_ID,
    };
    use serde_json::json;

    #[test]
    fn paper_pdf_url_drops_the_version_and_keeps_legacy_id_slashes() {
        assert_eq!(
            super::paper_pdf_url("2401.12345v2"),
            "https://export.arxiv.org/pdf/2401.12345"
        );
        assert_eq!(
            super::paper_pdf_url("hep-th/9901001"),
            "https://export.arxiv.org/pdf/hep-th/9901001"
        );
    }

    #[test]
    fn paper_discovery_url_encodes_strategy_and_controls() {
        let url = paper_discovery_url(
            "https://api.alphaxiv.org",
            "keyword",
            "attention & memory",
            PaperDiscoveryOptions {
                published_after: Some("2024-01-01"),
                published_before: Some("2025-12-31"),
                prioritize: "historical",
            },
        )
        .expect("valid discovery URL");
        let params = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();

        assert_eq!(url.path(), "/search/v2/paper/discover/keyword");
        assert_eq!(
            params.get("q").map(|value| value.as_ref()),
            Some("attention & memory")
        );
        assert_eq!(
            params.get("publishedAfter").map(|value| value.as_ref()),
            Some("2024-01-01")
        );
        assert_eq!(
            params.get("publishedBefore").map(|value| value.as_ref()),
            Some("2025-12-31")
        );
        assert_eq!(
            params.get("prioritize").map(|value| value.as_ref()),
            Some("historical")
        );
    }

    #[test]
    fn openalex_discovery_url_encodes_source_dates_and_priority() {
        let url = openalex_discovery_url(
            "https://api.openalex.org",
            "protein folding & agents",
            "dev@example.org",
            OpenAlexDiscoveryOptions {
                limit: 15,
                published_after: Some("2024-01-01"),
                published_before: Some("2026-01-31"),
                prioritize: "popular",
                source_filter: Some(BIORXIV_SOURCE_ID),
            },
        )
        .expect("valid OpenAlex URL");
        let params = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();

        assert_eq!(
            params.get("search").map(|value| value.as_ref()),
            Some("protein folding & agents")
        );
        assert_eq!(
            params.get("per_page").map(|value| value.as_ref()),
            Some("60")
        );
        assert_eq!(params.get("sort"), None);
        assert_eq!(
            params.get("filter").map(|value| value.as_ref()),
            Some("primary_location.source.id:S4306402567,from_publication_date:2024-01-01,to_publication_date:2026-01-31")
        );
    }

    #[test]
    fn reranks_only_the_relevant_openalex_result_pool() {
        let json = r#"[
            {"title":"middle","publication_date":"2020-01-01","cited_by_count":5},
            {"title":"new","publication_date":"2025-01-01","cited_by_count":1},
            {"title":"old","publication_date":"2010-01-01","cited_by_count":20},
            {"title":"unknown","publication_date":null,"cited_by_count":2}
        ]"#;
        let works = || serde_json::from_str::<Vec<OpenAlexWork>>(json).expect("valid works");

        let mut recency = works();
        rerank_openalex_works(&mut recency, "recency");
        assert_eq!(recency[0].title.as_deref(), Some("new"));

        let mut historical = works();
        rerank_openalex_works(&mut historical, "historical");
        assert_eq!(historical[0].title.as_deref(), Some("old"));
        assert_eq!(historical[3].title.as_deref(), Some("unknown"));

        let mut popular = works();
        rerank_openalex_works(&mut popular, "popular");
        assert_eq!(popular[0].title.as_deref(), Some("old"));
    }

    #[test]
    fn openresearch_client_contains_only_account_and_compute_paths() {
        let source = include_str!("client.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("client source has a production section");
        for forbidden in ["\"/projects", "\"/experiments", "\"/runs", "\"/skills"] {
            assert!(
                !production.contains(forbidden),
                "research-state endpoint remains in client.rs: {forbidden}"
            );
        }
        for retained in [
            "\"/orgs\"",
            "\"/compute/catalog",
            "\"/sandboxes",
            "\"/ssh-keys",
        ] {
            assert!(
                production.contains(retained),
                "missing infrastructure endpoint: {retained}"
            );
        }
    }

    /// The GPU catalog wire format carries `disk` as a discriminated union and an
    /// optional `region`, plus `bandwidth*` fields the CLI ignores. Pin that we
    /// decode both disk shapes, treat a missing region as `None`, and tolerate the
    /// extra fields — this is exactly the drift that previously broke `orx compute`.
    #[test]
    fn deserializes_gpu_catalog_with_disk_union_and_optional_region() {
        let json = r#"{
            "offers": [
                {
                    "provider": "runpod",
                    "offerId": "a",
                    "gpu": "H100_SXM",
                    "gpuCount": 1,
                    "vcpus": 16,
                    "ramGb": 188,
                    "pricePerHour": 2.5,
                    "disk": { "sizable": true, "perGbHour": 0.0001 },
                    "bandwidthInPerGb": 0,
                    "bandwidthOutPerGb": 0,
                    "region": "US_CA"
                },
                {
                    "provider": "lambda",
                    "offerId": "b",
                    "gpu": "A100_SXM_80GB",
                    "gpuCount": 8,
                    "vcpus": 124,
                    "ramGb": 1800,
                    "pricePerHour": 14.0,
                    "disk": { "sizable": false, "includedGb": 1024 },
                    "bandwidthInPerGb": 0,
                    "bandwidthOutPerGb": 0
                }
            ]
        }"#;

        let parsed: ListCatalog = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(parsed.offers.len(), 2);

        let sizable = &parsed.offers[0];
        assert_eq!(sizable.region.as_deref(), Some("US_CA"));
        assert!(sizable.disk.sizable);
        assert_eq!(sizable.disk.per_gb_hour, Some(0.0001));
        assert_eq!(sizable.disk.included_gb, None);

        let fixed = &parsed.offers[1];
        // `region` absent on the wire must decode to `None`.
        assert_eq!(fixed.region, None);
        assert!(!fixed.disk.sizable);
        assert_eq!(fixed.disk.included_gb, Some(1024.0));
        assert_eq!(fixed.disk.per_gb_hour, None);
    }

    /// CPU offers share the same `disk` union; pin that the CPU catalog decodes too.
    #[test]
    fn deserializes_cpu_catalog_with_disk_union() {
        let json = r#"{
            "offers": [
                {
                    "provider": "runpod",
                    "offerId": "c",
                    "cpuFlavor": "cpu5c",
                    "vcpus": 4,
                    "ramGb": 16,
                    "pricePerHour": 0.1,
                    "disk": { "sizable": true, "perGbHour": 0.0001 }
                }
            ]
        }"#;

        let parsed: ListCpuCatalog = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(parsed.offers.len(), 1);
        assert!(parsed.offers[0].disk.sizable);
        assert_eq!(parsed.offers[0].disk.per_gb_hour, Some(0.0001));
    }

    /// The standalone GPU sandbox target mirrors the run target's wire shape.
    #[test]
    fn serializes_sandbox_target_new() {
        let target = SandboxTarget::New {
            gpu: "H100_SXM".into(),
            gpu_count: 2,
            disk_gb: 100,
            provider: Some("vast".into()),
        };
        assert_eq!(
            serde_json::to_value(&target).unwrap(),
            json!({"type": "new", "gpu": "H100_SXM", "gpuCount": 2, "diskGb": 100, "provider": "vast"}),
        );
    }

    /// Omitting the provider must drop the key entirely — that's what lets the
    /// server pick the cheapest offer across providers for `instance create`.
    #[test]
    fn serializes_sandbox_target_new_without_provider() {
        let target = SandboxTarget::New {
            gpu: "H100_SXM".into(),
            gpu_count: 1,
            disk_gb: 100,
            provider: None,
        };
        let value = serde_json::to_value(&target).unwrap();
        assert_eq!(
            value,
            json!({"type": "new", "gpu": "H100_SXM", "gpuCount": 1, "diskGb": 100}),
        );
        assert!(value.get("provider").is_none());
    }

    /// The CPU sandbox target uses the `new-cpu` discriminant and camelCase keys.
    #[test]
    fn serializes_sandbox_target_new_cpu() {
        let target = SandboxTarget::NewCpu {
            cpu_flavor: "cpu5g".into(),
            vcpu_count: 8,
        };
        assert_eq!(
            serde_json::to_value(&target).unwrap(),
            json!({"type": "new-cpu", "cpuFlavor": "cpu5g", "vcpuCount": 8}),
        );
    }

    /// The create-sandbox body sends `organizationId` and never a `projectId`
    /// (the server rejects a project-scoped `new`/`new-cpu`).
    #[test]
    fn serializes_create_sandbox_body_without_project() {
        let body = CreateSandboxBody {
            organization_id: "org_123".into(),
            target: SandboxTarget::NewCpu {
                cpu_flavor: "cpu5c".into(),
                vcpu_count: 2,
            },
        };
        let value = serde_json::to_value(&body).unwrap();
        assert_eq!(value.get("organizationId"), Some(&json!("org_123")));
        assert!(value.get("projectId").is_none());
    }

    /// `POST /sandboxes` returns a freshly-provisioning box: ssh fields are still
    /// `null` while the GPU/provider/price fields are already populated from the
    /// offer. Pin that we decode that shape (camelCase keys, nulls → `None`).
    #[test]
    fn deserializes_sandbox_envelope_while_provisioning() {
        let json = r#"{
            "sandbox": {
                "id": "sb_1",
                "organizationId": "org_1",
                "projectId": null,
                "sshHostname": null,
                "sshPort": null,
                "sshUsername": null,
                "status": "provisioning",
                "machineType": "persistent",
                "createdBy": "user_1",
                "updatedAt": "2026-06-18T00:00:00Z",
                "provisionWarnings": null,
                "providerName": "runpod",
                "providerInstanceId": null,
                "pricePerHour": 2.5,
                "gpu": "H100_SXM",
                "gpuCount": 1,
                "vcpuCount": null
            }
        }"#;

        let parsed: SandboxEnvelope = serde_json::from_str(json).expect("should deserialize");
        let sb = parsed.sandbox;
        assert_eq!(sb.id, "sb_1");
        assert_eq!(sb.status, "provisioning");
        assert_eq!(sb.project_id, None);
        assert_eq!(sb.ssh_hostname, None);
        assert_eq!(sb.provider_name.as_deref(), Some("runpod"));
        assert_eq!(sb.gpu.as_deref(), Some("H100_SXM"));
        assert_eq!(sb.gpu_count, Some(1));
        assert_eq!(sb.vcpu_count, None);
        assert_eq!(sb.price_per_hour, Some(2.5));
    }

    /// `GET /sandboxes/{id}` on an online box: ssh fields populated — this is
    /// the shape the openresearch backend's provisioning wait consumes. Extra
    /// keys (the list endpoint adds `connections`) must be tolerated.
    #[test]
    fn deserializes_sandbox_envelope_when_online() {
        let json = r#"{
            "sandbox": {
                "id": "sb_1",
                "organizationId": "org_1",
                "projectId": null,
                "sshHostname": "203.0.113.7",
                "sshPort": 22022,
                "sshUsername": "root",
                "status": "online",
                "machineType": "persistent",
                "createdBy": "user_1",
                "updatedAt": "2026-06-18T00:00:00Z",
                "provisionWarnings": null,
                "providerName": "runpod",
                "providerInstanceId": "pod-abc",
                "pricePerHour": 2.5,
                "gpu": "H100_SXM",
                "gpuCount": 1,
                "vcpuCount": null,
                "connections": []
            }
        }"#;

        let parsed: SandboxEnvelope = serde_json::from_str(json).expect("should deserialize");
        let sb = parsed.sandbox;
        assert_eq!(sb.status, "online");
        assert_eq!(sb.ssh_hostname.as_deref(), Some("203.0.113.7"));
        assert_eq!(sb.ssh_port, Some(22022));
        assert_eq!(sb.ssh_username.as_deref(), Some("root"));
    }

    #[test]
    fn deserializes_retained_sandbox_failure() {
        let json = r#"{
            "sandbox": {
                "id": "sb_1",
                "organizationId": "org_1",
                "projectId": null,
                "sshHostname": null,
                "sshPort": null,
                "sshUsername": null,
                "status": "failed",
                "machineType": "persistent",
                "createdBy": "user_1",
                "updatedAt": "2026-06-18T00:05:00Z",
                "provisionWarnings": null,
                "failureCode": "capacity_unavailable",
                "failureMessage": "No capacity is available.",
                "failedAt": "2026-06-18T00:05:00Z",
                "providerName": "runpod",
                "providerInstanceId": null,
                "pricePerHour": 2.5,
                "gpu": "RTX_4090",
                "gpuCount": 2,
                "vcpuCount": null
            }
        }"#;

        let sandbox = serde_json::from_str::<SandboxEnvelope>(json)
            .expect("failed sandbox should deserialize")
            .sandbox;
        assert_eq!(sandbox.status, "failed");
        assert_eq!(
            sandbox.failure_code.as_deref(),
            Some("capacity_unavailable")
        );
        assert_eq!(
            sandbox.failure_message.as_deref(),
            Some("No capacity is available.")
        );
        assert_eq!(sandbox.failed_at.as_deref(), Some("2026-06-18T00:05:00Z"));
    }

    /// The inverted index maps token → positions; reconstruction must restore the
    /// original word order, and a missing index yields an empty string.
    #[test]
    fn reconstructs_openalex_abstract() {
        let work: OpenAlexWork = serde_json::from_str(
            r#"{ "abstract_inverted_index": { "Deep": [0], "learning": [1], "works": [2] } }"#,
        )
        .expect("should deserialize");
        assert_eq!(work.abstract_text(), "Deep learning works");
        assert_eq!(reconstruct_abstract(&None), "");
    }

    /// A DOI (bare, `doi:`-prefixed, or a `doi.org` URL) becomes the `doi:` form
    /// with slashes kept literal; anything else resolves to a bare `W…` work id.
    #[test]
    fn openalex_selector_routes_doi_vs_work_id() {
        assert_eq!(
            openalex_selector("10.1038/nature14539"),
            "doi:10.1038/nature14539"
        );
        assert_eq!(openalex_selector("doi:10.1038/x"), "doi:10.1038/x");
        assert_eq!(
            openalex_selector("https://doi.org/10.1101/2020.09.09.20191205"),
            "doi:10.1101/2020.09.09.20191205"
        );
        assert_eq!(openalex_selector("W2919115771"), "W2919115771");
        assert_eq!(
            openalex_selector("https://openalex.org/W2919115771"),
            "W2919115771"
        );
    }

    /// An OpenAlex `/works` row (id + doi as URLs, citations, inverted-index
    /// abstract, plus unselected extra fields) maps to a `LitHit`: DOI preferred
    /// as the routing id, `citations` set, `votes`/`snippets` empty.
    #[test]
    fn maps_openalex_work_to_lit_hit() {
        let work: OpenAlexWork = serde_json::from_str(
            r#"{
                "id": "https://openalex.org/W2919115771",
                "doi": "https://doi.org/10.1038/nature14539",
                "title": "Deep learning",
                "publication_date": "2015-05-26",
                "cited_by_count": 82932,
                "abstract_inverted_index": { "A": [0], "review.": [1] },
                "authorships": [{ "author": { "display_name": "Yann LeCun" } }],
                "some_unknown_field": true
            }"#,
        )
        .expect("should deserialize");

        assert_eq!(work.author_names(), vec!["Yann LeCun".to_string()]);
        assert_eq!(work.work_id().as_deref(), Some("W2919115771"));

        let hit = work.into_lit_hit(false);
        assert_eq!(hit.source, "openalex");
        assert_eq!(hit.id, "10.1038/nature14539");
        assert_eq!(hit.title, "Deep learning");
        assert_eq!(hit.abstract_, "A review.");
        assert_eq!(hit.citations, Some(82932));
        assert_eq!(hit.votes, None);
        assert!(hit.snippets.is_empty());
    }

    /// A bioRxiv-filtered OpenAlex hit routes through its `10.1101/…` DOI (so
    /// `orx paper` hits the richer bioRxiv fetch) and is labeled `biorxiv`.
    #[test]
    fn maps_biorxiv_filtered_hit_by_doi() {
        assert_eq!(BIORXIV_SOURCE_ID, "S4306402567");
        let work: OpenAlexWork = serde_json::from_str(
            r#"{
                "id": "https://openalex.org/W123",
                "doi": "https://doi.org/10.1101/2020.09.09.20191205",
                "title": "A preprint",
                "cited_by_count": 3
            }"#,
        )
        .expect("should deserialize");
        let hit = work.into_lit_hit(true);
        assert_eq!(hit.source, "biorxiv");
        assert_eq!(hit.id, "10.1101/2020.09.09.20191205");
        assert_eq!(hit.citations, Some(3));
    }

    /// An alphaXiv `PaperHit` maps to `LitHit` keeping `votes` and `snippets`;
    /// the JSON stays uniform, omitting the `None`/empty per-source fields.
    #[test]
    fn maps_paper_hit_and_serializes_uniform_json() {
        let ph: PaperHit = serde_json::from_str(
            r#"{
                "paperId": "2401.12345",
                "title": "A paper",
                "abstract": "Body.",
                "publicationDate": "2024-01-01T00:00:00Z",
                "votes": 7,
                "snippets": [{ "pageNumber": 2, "snippet": "hit" }]
            }"#,
        )
        .expect("should deserialize");

        let hit = LitHit::from(ph);
        assert_eq!(hit.source, "alphaxiv");
        assert_eq!(hit.id, "2401.12345");
        assert_eq!(hit.votes, Some(7));
        assert_eq!(hit.snippets.len(), 1);

        let value = serde_json::to_value(&hit).unwrap();
        assert_eq!(value.get("source"), Some(&json!("alphaxiv")));
        assert_eq!(value.get("votes"), Some(&json!(7)));
        // Per-source-only fields are omitted when empty.
        assert!(value.get("citations").is_none());

        let openalex = LitHit {
            source: "openalex".to_string(),
            id: "10.1/x".to_string(),
            title: "T".to_string(),
            abstract_: String::new(),
            publication_date: None,
            votes: None,
            citations: Some(5),
            snippets: Vec::new(),
        };
        let value = serde_json::to_value(&openalex).unwrap();
        assert_eq!(value.get("citations"), Some(&json!(5)));
        assert!(value.get("votes").is_none());
        assert!(value.get("snippets").is_none());
    }

    /// The bioRxiv details API wraps versions in a `collection`; we take the last
    /// (latest) and tolerate the extra top-level `messages` block.
    #[test]
    fn parses_biorxiv_latest_version() {
        #[derive(serde::Deserialize)]
        struct DetailsResponse {
            #[serde(default)]
            collection: Vec<super::BiorxivDetail>,
        }
        let body: DetailsResponse = serde_json::from_str(
            r#"{
                "messages": [{ "status": "ok" }],
                "collection": [
                    { "doi": "10.1101/2020.09.09.20191205", "title": "T", "version": "1",
                      "authors": "A; B", "abstract": "old", "date": "2020-09-09",
                      "category": "cell_biology", "published": "NA" },
                    { "doi": "10.1101/2020.09.09.20191205", "title": "T", "version": "2",
                      "authors": "A; B", "abstract": "new", "date": "2020-09-15",
                      "category": "cell_biology", "published": "10.1000/j.x" }
                ]
            }"#,
        )
        .expect("should deserialize");
        let latest = body.collection.into_iter().last().expect("has a version");
        assert_eq!(latest.version, "2");
        assert_eq!(latest.abstract_, "new");
        assert_eq!(latest.published, "10.1000/j.x");
    }
}
