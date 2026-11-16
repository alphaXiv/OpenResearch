//! The model feed openresearch.sh publishes: the models the picker lists as Free first, the nudge
//! it shows people with free models on, and the models released lately. `orx up` keeps the feed
//! in the cache directory as served, and the dashboard reads it from `/api/model-feed`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use axum::Json;
use serde::de::{DeserializeOwned, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

use super::ApiError;
use crate::error::{anyhow, Context, Result};

const REFRESH: Duration = Duration::from_secs(6 * 60 * 60);
const FIRST_RETRY: Duration = Duration::from_secs(60);

/// What orx reads of the feed. Its fields only grow, so unknown ones are ignored, and an item
/// holding a value this orx doesn't know is dropped, never the whole feed.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct ModelFeed {
    #[serde(deserialize_with = "known_items")]
    featured: Vec<FeaturedModel>,
    #[serde(deserialize_with = "known_item")]
    nudge: Option<ModelNudge>,
    /// Served as each release's line key and day, which the picker marks New by.
    #[serde(
        default,
        deserialize_with = "known_items",
        serialize_with = "release_days"
    )]
    releases: Vec<ModelRelease>,
}

#[derive(Debug, Deserialize)]
struct ModelRelease {
    /// models.dev's canonical `provider/id`.
    model: String,
    date: Day,
}

/// A calendar day, `YYYY-MM-DD`, so days order as strings.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String")]
struct Day(String);

impl TryFrom<String> for Day {
    type Error = String;

    fn try_from(day: String) -> std::result::Result<Self, String> {
        let dated = day.len() == 10
            && day.bytes().enumerate().all(|(index, byte)| match index {
                4 | 7 => byte == b'-',
                _ => byte.is_ascii_digit(),
            });
        if dated {
            Ok(Self(day))
        } else {
            Err(format!("{day:?} is not a YYYY-MM-DD day"))
        }
    }
}

/// Releases keyed the way the picker groups models, each with the earliest day any names it.
fn release_days<S: Serializer>(
    releases: &[ModelRelease],
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    let mut days = BTreeMap::<String, &Day>::new();
    for release in releases {
        let key = crate::local::model_routes::parse("opencode", &release.model, None, None).key;
        let day = days.entry(key).or_insert(&release.date);
        *day = (*day).min(&release.date);
    }
    days.serialize(serializer)
}

#[derive(Debug, Serialize, Deserialize)]
struct FeaturedModel {
    model: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ModelNudge {
    id: String,
    model: String,
    kind: NudgeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    until: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    colors: NudgeColors,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum NudgeKind {
    New,
    Trial,
}

#[derive(Debug, Serialize, Deserialize)]
struct NudgeColors {
    from: String,
    to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    icon: Option<String>,
}

fn known_items<'de, D: Deserializer<'de>, T: DeserializeOwned>(
    items: D,
) -> std::result::Result<Vec<T>, D::Error> {
    Ok(Vec::<Value>::deserialize(items)?
        .into_iter()
        .filter_map(|item| serde_json::from_value(item).ok())
        .collect())
}

fn known_item<'de, D: Deserializer<'de>, T: DeserializeOwned>(
    item: D,
) -> std::result::Result<Option<T>, D::Error> {
    Ok(Option::<Value>::deserialize(item)?.and_then(|item| serde_json::from_value(item).ok()))
}

/// Adds to Claude Code's models each Anthropic release it runs but doesn't list yet, beside the version it
/// does. Claude Code lists the models its build knew of and runs any id, so a release reaches the picker,
/// and sub-agents, before an update lists it.
pub(super) fn offer_unlisted_releases(payload: &mut Value) {
    let Some(feed) = std::fs::read_to_string(cache_path())
        .ok()
        .and_then(|body| serde_json::from_str::<ModelFeed>(&body).ok())
    else {
        return;
    };
    let claude = payload["harnesses"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .filter(|harness| harness["id"] == "claude-code");
    for harness in claude {
        let Some(models) = harness["models"].as_array_mut() else {
            continue;
        };
        for release in &feed.releases {
            if let Some((sibling, model)) = unlisted_release(models, &release.model) {
                models.insert(sibling, model);
            }
        }
    }
}

/// Where `release` goes among Claude Code's `models`, and the model it adds: none when it's listed, isn't
/// Anthropic's, or has no older version listed to stand beside.
fn unlisted_release(models: &[Value], release: &str) -> Option<(usize, Value)> {
    use crate::local::model_routes::{canonical, parse};

    let id = release.strip_prefix("anthropic/")?;
    let released = parse("claude-code", id, None, Some(id));
    let listed = |model: &Value| {
        parse(
            "claude-code",
            model["id"].as_str().unwrap_or_default(),
            model["displayName"].as_str(),
            model["resolvedModel"].as_str(),
        )
    };
    if models.iter().any(|model| listed(model).key == released.key) {
        return None;
    }
    released.version.as_ref()?;
    let (sibling, _) = models
        .iter()
        .enumerate()
        .map(|(index, model)| (index, listed(model)))
        .filter(|(_, name)| {
            name.series == released.series
                && name.version.is_some()
                && name.version < released.version
        })
        .max_by(|(_, a), (_, b)| a.version.cmp(&b.version))?;

    let name = canonical(id)
        .split('-')
        .map(|word| {
            let mut letters = word.chars();
            letters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(letters).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut model = serde_json::json!({ "id": id, "displayName": name, "resolvedModel": id });
    if let Some(levels) = models[sibling].get("reasoningLevels") {
        model["reasoningLevels"] = levels.clone();
    }
    Some((sibling, model))
}

fn cache_path() -> PathBuf {
    crate::local::git::cache_root().join("model-feed.json")
}

/// Fetches the feed at start, then every six hours. A failed fetch keeps the cached feed and is
/// retried sooner, backing off to the usual interval. A new feed re-reads the agents' models,
/// since the releases it dates can add one to Claude Code's.
pub(super) fn spawn_refresh(chat: std::sync::Arc<crate::local::chat::ChatHost>) {
    tokio::spawn(async move {
        let mut retry = FIRST_RETRY;
        loop {
            let before = std::fs::read(cache_path()).ok();
            let wait = match refresh().await {
                Ok(()) => {
                    if std::fs::read(cache_path()).ok() != before {
                        chat.emit_event("harness.catalog", serde_json::json!({}));
                    }
                    retry = FIRST_RETRY;
                    REFRESH
                }
                Err(error) => {
                    eprintln!("orx up: could not refresh the model feed: {error:#}");
                    let wait = retry;
                    retry = (retry * 2).min(REFRESH);
                    wait
                }
            };
            tokio::time::sleep(crate::config::paced(wait)).await;
        }
    });
}

/// Caches the feed byte for byte, so an orx updated later reads the fields this one ignores.
async fn refresh() -> Result<()> {
    let api = crate::config::openresearch_api_url().await;
    let body = crate::client::model_feed(&api).await?;
    serde_json::from_str::<ModelFeed>(&body)
        .with_context(|| format!("{api}/models/feed did not answer with a model feed"))?;

    let cache = crate::local::git::cache_root();
    std::fs::create_dir_all(&cache)
        .with_context(|| format!("Could not create {}", cache.display()))?;
    let path = cache_path();
    crate::local::git::atomic_write_with_mode(&path, body.as_bytes(), None)
        .with_context(|| format!("Could not write {}", path.display()))
}

/// The cached feed, or the empty one before the first fetch lands.
pub(super) async fn model_feed() -> std::result::Result<Json<ModelFeed>, ApiError> {
    let path = cache_path();
    let body = match tokio::fs::read_to_string(&path).await {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Json(ModelFeed::default()))
        }
        Err(error) => return Err(anyhow!("Could not read {}: {error}", path.display()).into()),
    };
    let feed = serde_json::from_str(&body)
        .map_err(|error| anyhow!("{} is not a model feed: {error}", path.display()))?;
    Ok(Json(feed))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::get;
    use axum::Router;
    use serde_json::json;

    use super::*;

    #[test]
    fn reads_every_feed_openresearch_serves() {
        let served: Vec<Value> =
            serde_json::from_str(include_str!("../../../tests/contracts/model-feed.json")).unwrap();
        for feed in served {
            let read =
                serde_json::to_value(serde_json::from_value::<ModelFeed>(feed.clone()).unwrap())
                    .unwrap();
            assert_eq!(
                (&read["featured"], &read["nudge"]),
                (&feed["featured"], &feed["nudge"])
            );
            let releases = feed["releases"].as_array().map_or(0, Vec::len);
            assert_eq!(read["releases"].as_object().unwrap().len(), releases);
        }
    }

    #[test]
    fn releases_are_keyed_like_the_agents_offering_them_and_dated_by_the_first() {
        let feed: ModelFeed = serde_json::from_value(json!({
            "featured": [],
            "nudge": null,
            "releases": [
                { "model": "anthropic/claude-fable-5-2", "date": "2026-11-09" },
                { "model": "openai/gpt-6.1-sol", "date": "2026-09-29" },
                { "model": "deepseek/deepseek-v4.1-flash-1017", "date": "2026-10-17" },
                { "model": "deepseek/deepseek-v4.1-flash", "date": "2026-09-10" },
                { "model": "google/gemini-4-carbon", "date": "2026-11" },
                { "model": "google/gemini-4-hydrogen" },
            ],
        }))
        .unwrap();

        let codex =
            crate::local::model_routes::parse("codex", "gpt-6.1-sol", Some("GPT-6.1-Sol"), None);
        let free = crate::local::model_routes::parse(
            "opencode",
            "opencode/deepseek-v4.1-flash-free",
            Some("DeepSeek V4.1 Flash Free"),
            None,
        );
        assert_eq!(
            serde_json::to_value(feed).unwrap()["releases"],
            json!({
                "claude fable@5.2": "2026-11-09",
                codex.key: "2026-09-29",
                free.key.strip_suffix(":free").unwrap(): "2026-09-10",
            })
        );
    }

    #[test]
    fn a_value_this_orx_does_not_know_drops_only_the_item_holding_it() {
        let feed: ModelFeed = serde_json::from_value(json!({
            "featured": [{ "model": "opencode/a-free", "badge": "hot" }, { "model": 7 }, { "model": "opencode/b-free" }],
            "nudge": { "id": "n", "model": "opencode/a-free", "kind": "sale", "colors": { "from": "#000000", "to": "#ffffff" } },
            "endpoints": [{ "id": "openresearch-x", "auth": { "kind": "oauth" } }],
            "banner": { "text": "Hello" },
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(feed).unwrap(),
            json!({ "featured": [{ "model": "opencode/a-free" }, { "model": "opencode/b-free" }], "nudge": null, "releases": {} })
        );
    }

    #[test]
    fn claude_code_runs_an_anthropic_release_it_does_not_list_yet_beside_the_version_it_does() {
        let efforts =
            json!([{ "id": "default", "label": "Adaptive" }, { "id": "high", "label": "High" }]);
        let models = vec![
            json!({ "id": "opus", "displayName": "Opus 5.5", "resolvedModel": "claude-opus-5-5" }),
            json!({ "id": "claude-fable-5-1[1m]", "displayName": "Fable 5.1", "resolvedModel": "claude-fable-5-1", "reasoningLevels": efforts }),
            json!({ "id": "haiku", "displayName": "Haiku 5.5", "resolvedModel": "claude-haiku-5-5" }),
        ];

        let (at, added) = unlisted_release(&models, "anthropic/claude-fable-5-2").unwrap();
        assert_eq!(at, 1);
        assert_eq!(
            added,
            json!({ "id": "claude-fable-5-2", "displayName": "Claude Fable 5.2", "resolvedModel": "claude-fable-5-2", "reasoningLevels": efforts })
        );
        let key = |model: &Value| {
            crate::local::model_routes::parse(
                "claude-code",
                model["id"].as_str().unwrap(),
                model["displayName"].as_str(),
                model["resolvedModel"].as_str(),
            )
            .key
        };
        assert_eq!(key(&added), "claude fable@5.2");

        for listed_or_foreign in [
            "anthropic/claude-opus-5-5",
            "anthropic/claude-fable-5-1",
            "anthropic/claude-mythos-6",
            "openai/gpt-6.1-sol",
        ] {
            assert_eq!(
                unlisted_release(&models, listed_or_foreign),
                None,
                "{listed_or_foreign}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_cache_keeps_the_feed_as_served_through_failed_fetches() {
        if !crate::config::in_test_child(&["OPENRESEARCH_API_URL"]) {
            return;
        }

        let answer: Arc<Mutex<Option<String>>> = Arc::default();
        let served = answer.clone();
        let api = Router::new().route(
            "/models/feed",
            get(move || {
                let body = served.lock().unwrap().clone();
                async move {
                    match body {
                        Some(body) => body.into_response(),
                        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, api).await.unwrap() });
        std::env::set_var("OPENRESEARCH_API_URL", format!("http://{address}"));
        let shown = || async { serde_json::to_value(model_feed().await.unwrap().0).unwrap() };

        assert_eq!(
            shown().await,
            json!({ "featured": [], "nudge": null, "releases": {} })
        );

        let body = json!({
            "featured": [{ "model": "opencode/a-free" }],
            "nudge": null,
            "endpoints": [],
            "addedByANewerServer": true,
        })
        .to_string();
        *answer.lock().unwrap() = Some(body.clone());
        refresh().await.unwrap();
        assert_eq!(std::fs::read_to_string(cache_path()).unwrap(), body);
        let cached =
            json!({ "featured": [{ "model": "opencode/a-free" }], "nudge": null, "releases": {} });
        assert_eq!(shown().await, cached);

        *answer.lock().unwrap() = None;
        let error = format!("{:#}", refresh().await.unwrap_err());
        assert!(
            error.contains("/models/feed") && error.contains("503"),
            "{error}"
        );
        *answer.lock().unwrap() = Some("<html>Down for maintenance</html>".into());
        let error = format!("{:#}", refresh().await.unwrap_err());
        assert!(
            error.contains("did not answer with a model feed"),
            "{error}"
        );
        assert_eq!(shown().await, cached);

        std::fs::write(cache_path(), "{").unwrap();
        let error = model_feed().await.unwrap_err();
        assert_eq!(error.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(error.1.contains("model-feed.json"), "{}", error.1);
    }
}
