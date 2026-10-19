//! arXiv and Semantic Scholar discovery, owned by the tuandang304 fork.
//!
//! `discover.rs` dispatches `orx discover arxiv|semanticscholar` here and the
//! Settings toggles live in `up/fork_settings.rs`; see docs/fork-maintenance.md.
//! Both hosts are public and keyless (Semantic Scholar takes an optional key).
//! Hits carry ids that `orx paper` already routes: an arXiv id goes to
//! alphaXiv, a DOI to OpenAlex/bioRxiv, a PMID to PubMed.

use crate::client::{compare_dates, http, public_get, LitHit};
use crate::error::{anyhow, Result};
use crate::DiscoverySearchArgs;
use serde::Deserialize;

pub const ARXIV: &str = "arxiv";
pub const SEMANTIC_SCHOLAR: &str = "semanticscholar";

/// The fork's sources as `(settings name, display name)`.
pub const SOURCES: [(&str, &str); 2] = [(ARXIV, "arXiv"), (SEMANTIC_SCHOLAR, "Semantic Scholar")];

/// Base URL for the arXiv query API. Override with `ARXIV_API_URL`.
fn arxiv_api_url() -> String {
    std::env::var("ARXIV_API_URL").unwrap_or_else(|_| "https://export.arxiv.org/api".to_string())
}

/// Base URL for the Semantic Scholar Graph API. Override with `SEMANTIC_SCHOLAR_API_URL`.
fn semantic_scholar_api_url() -> String {
    std::env::var("SEMANTIC_SCHOLAR_API_URL")
        .unwrap_or_else(|_| "https://api.semanticscholar.org/graph/v1".to_string())
}

/// Optional Semantic Scholar key: moves requests out of the shared, often
/// throttled keyless pool into a dedicated allowance.
fn semantic_scholar_api_key() -> Option<String> {
    std::env::var("SEMANTIC_SCHOLAR_API_KEY")
        .ok()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

pub fn ensure_enabled(source: &str, disabled: &[String]) -> Result<()> {
    if disabled.iter().any(|d| d == source) {
        let name = SOURCES
            .iter()
            .find(|(key, _)| *key == source)
            .map_or(source, |(_, name)| name);
        return Err(anyhow!(
            "{name} is disabled by your OpenResearch literature-source configuration. Re-enable it to discover {name} papers."
        ));
    }
    Ok(())
}

/// How many results to request: a broader relevance page when a priority will rerank it.
fn fetch_limit(args: &DiscoverySearchArgs, reranks: bool, max: u32) -> u32 {
    let limit = if reranks {
        args.limit.saturating_mul(4).max(50)
    } else {
        args.limit
    };
    limit.clamp(1, max)
}

fn rerank(hits: &mut [LitHit], prioritize: &str) {
    match prioritize {
        "recency" => {
            hits.sort_by(|a, b| compare_dates(&a.publication_date, &b.publication_date, true))
        }
        "historical" => {
            hits.sort_by(|a, b| compare_dates(&a.publication_date, &b.publication_date, false))
        }
        "popular" => hits.sort_by(|a, b| {
            b.citations
                .unwrap_or_default()
                .cmp(&a.citations.unwrap_or_default())
        }),
        _ => {}
    }
}

fn hit(source: &str, id: String, title: String, abstract_: String) -> LitHit {
    LitHit {
        source: source.to_string(),
        id,
        title,
        abstract_,
        publication_date: None,
        votes: None,
        citations: None,
        snippets: Vec::new(),
    }
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// arXiv: the Atom query API. arXiv asks for at most one request every three
// seconds; each `orx discover arxiv` makes exactly one.
// ---------------------------------------------------------------------------

/// arXiv field prefixes; a query that already uses one is passed through as written.
const ARXIV_FIELDS: [&str; 9] = [
    "ti:", "au:", "abs:", "co:", "jr:", "cat:", "rn:", "id:", "all:",
];

/// Plain words become `all:` terms joined with AND (quoted phrases stay
/// together); a query written in arXiv's own syntax is kept as is.
fn arxiv_search_query(query: &str) -> String {
    let query = query.trim();
    let lower = query.to_ascii_lowercase();
    let uses_fields = lower
        .split(|c: char| c.is_whitespace() || c == '(')
        .any(|word| ARXIV_FIELDS.iter().any(|field| word.starts_with(field)));
    if uses_fields {
        return query.to_string();
    }
    let mut terms = Vec::new();
    let mut rest = query;
    while !rest.is_empty() {
        rest = rest.trim_start();
        if let Some(quoted) = rest.strip_prefix('"') {
            let end = quoted.find('"').unwrap_or(quoted.len());
            let phrase = collapse_whitespace(&quoted[..end]);
            if !phrase.is_empty() {
                terms.push(format!("all:\"{phrase}\""));
            }
            rest = quoted.get(end + 1..).unwrap_or("");
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let word = rest[..end].trim_matches(|c: char| c == '(' || c == ')' || c == ':');
            if !word.is_empty() {
                terms.push(format!("all:{word}"));
            }
            rest = &rest[end..];
        }
    }
    terms.join(" AND ")
}

/// `YYYY-MM-DD` → arXiv's `YYYYMMDDHHMM`, or the open-ended fallback.
fn arxiv_date(date: Option<&str>, fallback: &str, time: &str) -> String {
    date.map_or_else(
        || fallback.to_string(),
        |d| format!("{}{time}", d.replace('-', "")),
    )
}

fn arxiv_discovery_url(base: &str, args: &DiscoverySearchArgs) -> Result<reqwest::Url> {
    let mut search = arxiv_search_query(&args.query);
    if args.published_after.is_some() || args.published_before.is_some() {
        let from = arxiv_date(args.published_after.as_deref(), "199101010000", "0000");
        let to = arxiv_date(args.published_before.as_deref(), "209912312359", "2359");
        search = format!("({search}) AND submittedDate:[{from} TO {to}]");
    }
    let reranks = matches!(args.prioritize.as_str(), "recency" | "historical");
    let mut url = reqwest::Url::parse(&format!("{base}/query"))?;
    url.query_pairs_mut()
        .append_pair("search_query", &search)
        .append_pair("start", "0")
        .append_pair("max_results", &fetch_limit(args, reranks, 200).to_string())
        .append_pair("sortBy", "relevance")
        .append_pair("sortOrder", "descending");
    Ok(url)
}

/// `http://arxiv.org/abs/2401.12345v2` → `2401.12345`; old-style ids keep their archive.
fn arxiv_id(entry_id: &str) -> String {
    let id = entry_id
        .split_once("/abs/")
        .map_or(entry_id, |(_, id)| id)
        .trim();
    match id.rfind('v') {
        Some(at)
            if at > 0 && id[at + 1..].bytes().all(|b| b.is_ascii_digit()) && at + 1 < id.len() =>
        {
            id[..at].to_string()
        }
        _ => id.to_string(),
    }
}

fn parse_arxiv_feed(xml: &str) -> Result<Vec<LitHit>> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| anyhow!("arXiv returned a response that is not valid XML: {e}"))?;
    let text = |node: roxmltree::Node<'_, '_>, name: &str| {
        node.children()
            .find(|c| c.has_tag_name(name))
            .and_then(|c| c.text())
            .map(collapse_whitespace)
            .unwrap_or_default()
    };
    let mut hits = Vec::new();
    for entry in doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name("entry"))
    {
        let id = text(entry, "id");
        if id.contains("/api/errors") {
            return Err(anyhow!("arXiv search failed: {}", text(entry, "summary")));
        }
        let mut item = hit(
            ARXIV,
            arxiv_id(&id),
            text(entry, "title"),
            text(entry, "summary"),
        );
        let published = text(entry, "published");
        item.publication_date = published.get(..10).map(str::to_string);
        hits.push(item);
    }
    Ok(hits)
}

pub async fn discover_arxiv(args: &DiscoverySearchArgs) -> Result<Vec<LitHit>> {
    let base = arxiv_api_url();
    let url = arxiv_discovery_url(&base, args)?;
    let res = public_get(
        || http().get(url.clone()),
        "arXiv",
        |e| anyhow!("Could not reach arXiv at {}: {}", base, e),
    )
    .await?;
    let status = res.status();
    if !status.is_success() {
        let reason = status.canonical_reason().unwrap_or("");
        return Err(anyhow!(
            "arXiv search failed ({} {}). arXiv allows one request every three seconds; wait and retry.",
            status.as_u16(),
            reason
        ));
    }
    let mut hits = parse_arxiv_feed(&res.text().await?)?;
    rerank(&mut hits, args.prioritize.as_str());
    hits.truncate(args.limit as usize);
    Ok(hits)
}

// ---------------------------------------------------------------------------
// Semantic Scholar: Graph API relevance search across all disciplines.
// ---------------------------------------------------------------------------

const S2_FIELDS: &str = "title,abstract,year,publicationDate,citationCount,externalIds";

fn semantic_scholar_discovery_url(base: &str, args: &DiscoverySearchArgs) -> Result<reqwest::Url> {
    let reranks = args.prioritize.as_str() != "default";
    let mut url = reqwest::Url::parse(&format!("{base}/paper/search"))?;
    {
        let mut params = url.query_pairs_mut();
        params.append_pair("query", args.query.trim());
        params.append_pair("limit", &fetch_limit(args, reranks, 100).to_string());
        params.append_pair("fields", S2_FIELDS);
        if args.published_after.is_some() || args.published_before.is_some() {
            let range = format!(
                "{}:{}",
                args.published_after.as_deref().unwrap_or(""),
                args.published_before.as_deref().unwrap_or("")
            );
            params.append_pair("publicationDateOrYear", &range);
        }
    }
    Ok(url)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct S2Paper {
    paper_id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(rename = "abstract", default)]
    abstract_: Option<String>,
    #[serde(default)]
    year: Option<i64>,
    #[serde(default)]
    publication_date: Option<String>,
    #[serde(default)]
    citation_count: Option<i64>,
    #[serde(default)]
    external_ids: Option<S2ExternalIds>,
}

#[derive(Debug, Default, Deserialize)]
struct S2ExternalIds {
    #[serde(rename = "ArXiv", default)]
    arxiv: Option<String>,
    #[serde(rename = "DOI", default)]
    doi: Option<String>,
    #[serde(rename = "PubMed", default)]
    pubmed: Option<String>,
}

impl S2Paper {
    /// The id `orx paper` can read: arXiv, then DOI, then PMID; else the Semantic Scholar page.
    fn routable_id(&self) -> String {
        let ids = self.external_ids.as_ref();
        ids.and_then(|i| i.arxiv.clone())
            .or_else(|| ids.and_then(|i| i.doi.clone()))
            .or_else(|| ids.and_then(|i| i.pubmed.as_ref().map(|pmid| format!("pmid:{pmid}"))))
            .unwrap_or_else(|| format!("https://www.semanticscholar.org/paper/{}", self.paper_id))
    }

    fn into_lit_hit(self) -> LitHit {
        let id = self.routable_id();
        let mut item = hit(
            SEMANTIC_SCHOLAR,
            id,
            self.title.unwrap_or_default(),
            self.abstract_.unwrap_or_default(),
        );
        item.publication_date = self
            .publication_date
            .or_else(|| self.year.map(|year| year.to_string()));
        item.citations = self.citation_count;
        item
    }
}

pub async fn discover_semantic_scholar(args: &DiscoverySearchArgs) -> Result<Vec<LitHit>> {
    let base = semantic_scholar_api_url();
    let url = semantic_scholar_discovery_url(&base, args)?;
    let key = semantic_scholar_api_key();
    let res = public_get(
        || {
            let request = http().get(url.clone());
            match &key {
                Some(key) => request.header("x-api-key", key),
                None => request,
            }
        },
        "Semantic Scholar",
        |e| anyhow!("Could not reach Semantic Scholar at {}: {}", base, e),
    )
    .await
    .map_err(|e| match key {
        Some(_) => e,
        None => anyhow!(
            "{e}. Keyless requests share one throttled pool; set SEMANTIC_SCHOLAR_API_KEY (free at semanticscholar.org/product/api) for a dedicated limit."
        ),
    })?;
    let status = res.status();
    if !status.is_success() {
        let reason = status.canonical_reason().unwrap_or("");
        return Err(anyhow!(
            "Semantic Scholar search failed ({} {})",
            status.as_u16(),
            reason
        ));
    }

    #[derive(Deserialize)]
    struct SearchResponse {
        #[serde(default)]
        data: Vec<S2Paper>,
    }
    let body = res.json::<SearchResponse>().await?;
    let mut hits: Vec<LitHit> = body.data.into_iter().map(S2Paper::into_lit_hit).collect();
    rerank(&mut hits, args.prioritize.as_str());
    hits.truncate(args.limit as usize);
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DiscoveryPriority;

    fn args(query: &str, prioritize: DiscoveryPriority) -> DiscoverySearchArgs {
        DiscoverySearchArgs {
            query: query.to_string(),
            published_after: None,
            published_before: None,
            prioritize,
            limit: 15,
        }
    }

    #[test]
    fn arxiv_query_ands_words_and_keeps_phrases_and_field_syntax() {
        assert_eq!(
            arxiv_search_query(r#"masked "discrete diffusion" sampling"#),
            r#"all:masked AND all:"discrete diffusion" AND all:sampling"#
        );
        assert_eq!(
            arxiv_search_query("au:hinton AND cat:cs.LG"),
            "au:hinton AND cat:cs.LG"
        );
        assert_eq!(arxiv_search_query("  (unbalanced  "), "all:unbalanced");
    }

    #[test]
    fn arxiv_url_adds_date_window_and_widens_date_priorities() {
        let mut a = args("diffusion", DiscoveryPriority::Recency);
        a.published_after = Some("2024-01-01".to_string());
        let url = arxiv_discovery_url("https://export.arxiv.org/api", &a).unwrap();
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(
            pairs["search_query"],
            "(all:diffusion) AND submittedDate:[202401010000 TO 209912312359]"
        );
        assert_eq!(pairs["max_results"], "60");
        let url = arxiv_discovery_url(
            "https://export.arxiv.org/api",
            &args("diffusion", DiscoveryPriority::Popular),
        )
        .unwrap();
        assert!(url.as_str().contains("max_results=15"));
    }

    #[test]
    fn arxiv_feed_parses_entries_and_strips_versions() {
        let xml = r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <entry>
    <id>http://arxiv.org/abs/2406.04329v4</id>
    <title>Simple and Effective
      Masked Diffusion</title>
    <summary> We study masked diffusion. </summary>
    <published>2024-06-06T17:59:10Z</published>
  </entry>
  <entry>
    <id>http://arxiv.org/abs/hep-th/9901001v1</id>
    <title>Old</title>
    <summary>Old style.</summary>
    <published>1999-01-01T00:00:00Z</published>
  </entry>
</feed>"#;
        let hits = parse_arxiv_feed(xml).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "2406.04329");
        assert_eq!(hits[0].title, "Simple and Effective Masked Diffusion");
        assert_eq!(hits[0].abstract_, "We study masked diffusion.");
        assert_eq!(hits[0].publication_date.as_deref(), Some("2024-06-06"));
        assert_eq!(hits[0].source, "arxiv");
        assert_eq!(hits[1].id, "hep-th/9901001");
    }

    #[test]
    fn arxiv_error_entry_becomes_an_error() {
        let xml = r#"<feed xmlns="http://www.w3.org/2005/Atom"><entry>
<id>https://arxiv.org/api/errors</id><title>Error</title>
<summary>Invalid query string</summary></entry></feed>"#;
        let error = parse_arxiv_feed(xml).unwrap_err();
        assert!(error.to_string().contains("Invalid query string"));
    }

    #[test]
    fn semantic_scholar_url_sets_date_range_and_pool() {
        let mut a = args("protein folding", DiscoveryPriority::Popular);
        a.published_before = Some("2020-12-31".to_string());
        let url =
            semantic_scholar_discovery_url("https://api.semanticscholar.org/graph/v1", &a).unwrap();
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(pairs["query"], "protein folding");
        assert_eq!(pairs["limit"], "60");
        assert_eq!(pairs["publicationDateOrYear"], ":2020-12-31");
        assert_eq!(pairs["fields"], S2_FIELDS);
    }

    #[test]
    fn semantic_scholar_hits_prefer_ids_orx_paper_can_read() {
        let body = r#"{"data":[
          {"paperId":"a1","title":"A","abstract":null,"year":2017,"publicationDate":null,
           "citationCount":5,"externalIds":{"ArXiv":"1706.03762","DOI":"10.48550/arXiv.1706.03762"}},
          {"paperId":"b2","title":"B","year":2020,"publicationDate":"2020-05-01",
           "citationCount":90,"externalIds":{"DOI":"10.1038/x","PubMed":"123"}},
          {"paperId":"c3","title":"C","externalIds":{"PubMed":"456"}},
          {"paperId":"d4","title":"D","externalIds":{}}
        ]}"#;
        #[derive(Deserialize)]
        struct Body {
            data: Vec<S2Paper>,
        }
        let mut hits: Vec<LitHit> = serde_json::from_str::<Body>(body)
            .unwrap()
            .data
            .into_iter()
            .map(S2Paper::into_lit_hit)
            .collect();
        let ids: Vec<_> = hits.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "1706.03762",
                "10.1038/x",
                "pmid:456",
                "https://www.semanticscholar.org/paper/d4"
            ]
        );
        assert_eq!(hits[0].publication_date.as_deref(), Some("2017"));
        assert_eq!(hits[0].abstract_, "");
        rerank(&mut hits, "popular");
        assert_eq!(hits[0].title, "B");
    }

    #[test]
    fn cli_parses_fork_discovery_sources() {
        use clap::Parser as _;
        for source in ["arxiv", "semanticscholar"] {
            let cli = crate::Cli::try_parse_from(["orx", "discover", source, "masked diffusion"])
                .expect("fork discovery source should parse");
            let Some(crate::Command::Discover(crate::DiscoverArgs { command })) = cli.command
            else {
                panic!("expected discover command");
            };
            let parsed = match command {
                crate::DiscoverCommand::Arxiv(args) => (ARXIV, args.query),
                crate::DiscoverCommand::Semanticscholar(args) => (SEMANTIC_SCHOLAR, args.query),
                _ => panic!("expected a fork discovery source"),
            };
            assert_eq!(parsed, (source, "masked diffusion".to_string()));
        }
    }

    #[test]
    fn disabled_fork_sources_refuse_discovery() {
        let error = ensure_enabled(ARXIV, &["arxiv".to_string()]).unwrap_err();
        assert!(error.to_string().contains("arXiv is disabled"));
        let error = ensure_enabled(SEMANTIC_SCHOLAR, &["semanticscholar".to_string()]).unwrap_err();
        assert!(error.to_string().contains("Semantic Scholar is disabled"));
        ensure_enabled(ARXIV, &["pubmed".to_string()]).unwrap();
    }
}
