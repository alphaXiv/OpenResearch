//! The model feed openresearch.sh publishes: the models the picker lists as Free first, the nudge
//! it shows people with free models on, the models released lately, and the free endpoints OpenCode
//! runs as providers. `orx up` keeps the feed in the cache directory as served, and the dashboard
//! reads it from `/api/model-feed`.

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
    /// Read where OpenCode starts, and by the picker as models of their own.
    #[serde(default, deserialize_with = "known_items", skip_serializing)]
    endpoints: Vec<HostedEndpoint>,
}

/// An OpenAI-compatible endpoint whose models are free to everyone with free models on.
#[derive(Debug, Deserialize)]
#[serde(try_from = "EndpointWire")]
pub(crate) struct HostedEndpoint {
    /// OpenCode's provider id for it, which outlives a change to its address.
    pub(crate) id: String,
    pub(crate) base_url: reqwest::Url,
    pub(crate) key: String,
    pub(crate) models: Vec<HostedModel>,
}

#[derive(Deserialize)]
struct EndpointWire {
    id: String,
    #[serde(rename = "baseURL")]
    base_url: String,
    auth: EndpointAuth,
    models: Vec<HostedModel>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum EndpointAuth {
    Key { key: String },
}

impl TryFrom<EndpointWire> for HostedEndpoint {
    type Error = String;

    fn try_from(wire: EndpointWire) -> std::result::Result<Self, String> {
        let EndpointAuth::Key { key } = wire.auth;
        let id_ok = wire
            .id
            .starts_with(crate::local::model_routes::HOSTED_PROVIDER_PREFIX)
            && crate::local::local_models::is_provider_id(&wire.id);
        if !id_ok {
            return Err(format!("{:?} is not a hosted endpoint id", wire.id));
        }
        let base_url = reqwest::Url::parse(&wire.base_url)
            .ok()
            .filter(|url| url.scheme() == "https")
            .ok_or_else(|| format!("{:?} is not an https URL", wire.base_url))?;
        Ok(Self {
            id: wire.id,
            base_url,
            key,
            models: wire.models,
        })
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct HostedModel {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) context: u64,
    pub(crate) output: u64,
    #[serde(default, deserialize_with = "known_item")]
    icon: Option<ModelIcon>,
}

/// An image that replaces the mark the picker matches from the model's name.
#[derive(Debug, Serialize, Deserialize)]
struct ModelIcon {
    #[serde(rename = "type")]
    kind: IconType,
    data: String,
}

#[derive(Debug, Serialize, Deserialize)]
enum IconType {
    #[serde(rename = "image/png")]
    Png,
    #[serde(rename = "image/webp")]
    Webp,
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

/// The cached feed, or the empty one before the first fetch lands.
fn read_cache() -> Result<ModelFeed> {
    let path = cache_path();
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ModelFeed::default())
        }
        Err(error) => return Err(anyhow!("Could not read {}: {error}", path.display())),
    };
    serde_json::from_str(&body)
        .map_err(|error| anyhow!("{} is not a model feed: {error}", path.display()))
}

/// The cached feed where losing it costs nothing: the next fetch rewrites an unreadable cache.
fn cached() -> Option<ModelFeed> {
    read_cache().ok()
}

/// The endpoints OpenCode is given as providers: none while free models are off.
pub(crate) fn hosted_endpoints() -> Vec<HostedEndpoint> {
    if !crate::local::harness::opencode::free_models_offered() {
        return Vec::new();
    }
    cached().map(|feed| feed.endpoints).unwrap_or_default()
}

/// Fails a turn on a hosted model OpenCode isn't given now, saying why: OpenCode itself would only go idle.
pub(crate) fn ensure_offered(model: &str) -> Result<()> {
    let Some((provider, id)) = model
        .split_once('/')
        .filter(|_| crate::local::model_routes::hosted(model))
    else {
        return Ok(());
    };
    let feed = read_cache()?;
    let listed = feed
        .endpoints
        .iter()
        .filter(|endpoint| endpoint.id == provider)
        .flat_map(|endpoint| &endpoint.models)
        .find(|hosted| hosted.id == id);
    match listed {
        None => Err(anyhow!(
            "{model} is no longer one of OpenResearch's free models. Choose another model for this chat."
        )),
        Some(hosted) if !crate::local::harness::opencode::free_models_offered() => Err(anyhow!(
            "{} is one of OpenResearch's free models, which are turned off. Turn on Free Models in Settings → Agents, or choose another model for this chat.",
            hosted.name
        )),
        Some(_) => Ok(()),
    }
}

/// Adds to the agents' models what the feed knows of them. Claude Code gets each Anthropic release it runs
/// but doesn't list yet, beside the version it does: it lists the models its build knew of and runs any id,
/// so a release reaches the picker, and sub-agents, before an update lists it. A hosted model gets its icon.
pub(super) fn annotate_models(payload: &mut Value) {
    if let Some(feed) = cached() {
        annotate(payload, &feed);
    }
}

fn annotate(payload: &mut Value, feed: &ModelFeed) {
    let harnesses = payload["harnesses"].as_array_mut().into_iter().flatten();
    for harness in harnesses {
        let claude = harness["id"] == "claude-code";
        let Some(models) = harness["models"].as_array_mut() else {
            continue;
        };
        if claude {
            for release in &feed.releases {
                if let Some((sibling, model)) = unlisted_release(models, &release.model) {
                    models.insert(sibling, model);
                }
            }
        }
        for model in models.iter_mut() {
            if let Some(icon) =
                hosted_icon(&feed.endpoints, model["id"].as_str().unwrap_or_default())
            {
                model["icon"] = serde_json::to_value(icon).unwrap_or_default();
            }
        }
    }
}

fn hosted_icon<'a>(endpoints: &'a [HostedEndpoint], id: &str) -> Option<&'a ModelIcon> {
    let (provider, model) = id.split_once('/')?;
    endpoints
        .iter()
        .filter(|endpoint| endpoint.id == provider)
        .flat_map(|endpoint| &endpoint.models)
        .find(|hosted| hosted.id == model)?
        .icon
        .as_ref()
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
/// retried sooner, backing off to the usual interval. A new feed re-reads the agents' models, since
/// the releases it dates can add one to Claude Code's, and re-detects OpenCode, whose providers
/// include its endpoints.
pub(super) fn spawn_refresh(state: super::AppState) {
    tokio::spawn(async move {
        let mut retry = FIRST_RETRY;
        loop {
            let before = std::fs::read(cache_path()).ok();
            let wait = match refresh().await {
                Ok(()) => {
                    if std::fs::read(cache_path()).ok() != before {
                        super::recheck_opencode(&state).await;
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
    Ok(Json(read_cache()?))
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
        for feed in &served {
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
        let hosted = served
            .iter()
            .flat_map(|feed| {
                serde_json::from_value::<ModelFeed>(feed.clone())
                    .unwrap()
                    .endpoints
            })
            .map(|endpoint| endpoint.id)
            .collect::<Vec<_>>();
        assert_eq!(
            hosted,
            ["openresearch-k3v9q2"],
            "an alphaXiv sign-in endpoint is dropped"
        );
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
    fn an_endpoint_needs_a_hosted_id_https_and_a_key_while_an_unknown_icon_drops_only_itself() {
        let model = |icon: Value| json!({ "id": "glm-5.3", "name": "GLM 5.3", "context": 128000, "output": 16000, "icon": icon });
        let png = json!({ "type": "image/png", "data": "iVBORw0KGgo=" });
        let endpoint = |id: &str, url: &str, auth: Value| json!({ "id": id, "baseURL": url, "auth": auth, "models": [model(png.clone())] });
        let key = json!({ "kind": "key", "key": "public" });
        let feed: ModelFeed = serde_json::from_value(json!({
            "featured": [],
            "nudge": null,
            "endpoints": [
                endpoint("openresearch-a1", "https://api.openresearch.sh/v1", key.clone()),
                endpoint("openresearch-a2", "https://example.com/v1", json!({ "kind": "alphaxiv" })),
                endpoint("openresearch-a3", "http://example.com/v1", key.clone()),
                endpoint("anthropic", "https://example.com/v1", key.clone()),
                { "id": "openresearch-a4", "baseURL": "https://example.com/v1", "auth": key, "models": [model(json!({ "type": "image/svg+xml", "data": "PHN2Zz4=" }))] },
            ],
        }))
        .unwrap();

        let kept: Vec<_> = feed
            .endpoints
            .iter()
            .map(|endpoint| endpoint.id.as_str())
            .collect();
        assert_eq!(kept, ["openresearch-a1", "openresearch-a4"]);
        assert_eq!(feed.endpoints[0].key, "public");
        assert!(feed.endpoints[0].models[0].icon.is_some());
        assert!(feed.endpoints[1].models[0].icon.is_none());
        assert_eq!(serde_json::to_value(&feed).unwrap().get("endpoints"), None);

        let mut payload = json!({ "harnesses": [{ "id": "opencode", "models": [
            { "id": "openresearch-a1/glm-5.3" }, { "id": "openresearch-a4/glm-5.3" }, { "id": "opencode/glm-5.3-free" },
        ] }] });
        annotate(&mut payload, &feed);
        let models = &payload["harnesses"][0]["models"];
        assert_eq!(models[0]["icon"], png);
        assert_eq!(models[1].get("icon"), None);
        assert_eq!(models[2].get("icon"), None);
    }

    #[test]
    fn a_turn_on_a_hosted_model_not_offered_now_says_why() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let feed = json!({ "featured": [], "nudge": null, "endpoints": [{
            "id": "openresearch-a1", "baseURL": "https://api.openresearch.sh/v1", "auth": { "kind": "key", "key": "" },
            "models": [{ "id": "research-1", "name": "OpenResearch 1", "context": 1000, "output": 100 }],
        }] });
        std::fs::create_dir_all(cache_path().parent().unwrap()).unwrap();
        std::fs::write(cache_path(), feed.to_string()).unwrap();

        ensure_offered("openresearch-a1/research-1").unwrap();
        ensure_offered("opencode/deepseek-v4.1-flash-free").unwrap();
        let gone = ensure_offered("openresearch-a1/research-0")
            .unwrap_err()
            .to_string();
        assert!(
            gone.contains("openresearch-a1/research-0") && gone.contains("no longer"),
            "{gone}"
        );

        std::fs::write(cache_path(), "{").unwrap();
        let unreadable = ensure_offered("openresearch-a1/research-1")
            .unwrap_err()
            .to_string();
        assert!(
            unreadable.contains("model-feed.json") && !unreadable.contains("no longer"),
            "{unreadable}"
        );
        std::fs::write(cache_path(), feed.to_string()).unwrap();

        crate::store::Store::open()
            .unwrap()
            .set_opencode_free_models(false)
            .unwrap();
        let off = ensure_offered("openresearch-a1/research-1")
            .unwrap_err()
            .to_string();
        assert!(
            off.contains("OpenResearch 1") && off.contains("Free Models"),
            "{off}"
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
