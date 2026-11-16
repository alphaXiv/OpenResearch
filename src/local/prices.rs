//! Model prices from models.dev, for agents that report tokens but no cost. `orx up` keeps the
//! raw list in the cache directory, at most a day old; a failed fetch keeps the last good copy.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use serde_json::Value;

use crate::error::{anyhow, Result};
use crate::store::{Cost, TokenUsage};

const URL: &str = "https://models.dev/api.json";
const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

pub(crate) fn path() -> PathBuf {
    crate::local::git::cache_root().join("models.dev.json")
}

#[derive(Deserialize)]
struct Provider {
    #[serde(default)]
    models: HashMap<String, Model>,
}

/// Kept as JSON so one malformed price rejects that model, not the list.
#[derive(Deserialize)]
struct Model {
    #[serde(default)]
    cost: Value,
    #[serde(default)]
    canonical_model_id: Value,
}

pub(crate) struct PriceList {
    providers: HashMap<String, Provider>,
    /// Model id → the providers whose own model it is, from every entry's `canonical_model_id`.
    makers: HashMap<String, HashSet<String>>,
}

impl PriceList {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        let providers: HashMap<String, Provider> = serde_json::from_slice(bytes)?;
        let mut makers: HashMap<String, HashSet<String>> = HashMap::new();
        for model in providers
            .values()
            .flat_map(|provider| provider.models.values())
        {
            if let Some((maker, id)) = model
                .canonical_model_id
                .as_str()
                .and_then(|canonical| canonical.split_once('/'))
            {
                makers.entry(id.into()).or_default().insert(maker.into());
            }
        }
        Ok(Self { providers, makers })
    }

    /// The list's price for exactly this model: the reporting provider's when it names one,
    /// else the model maker's own.
    fn rates(&self, harness: &str, model: &str, provider: Option<&str>) -> Option<&Value> {
        let entry = |provider: &str, model: &str| {
            let cost = &self.providers.get(provider)?.models.get(model)?.cost;
            cost.is_object().then_some(cost)
        };
        // Claude Code calls Anthropic's own API `firstParty`.
        let provider = provider.filter(|p| !(harness == "claude-code" && *p == "firstParty"));
        if let Some(provider) = provider {
            return entry(provider, model);
        }
        if let Some((maker, id)) = model.split_once('/') {
            return entry(maker, id);
        }
        let own = match harness {
            "claude-code" => Some("anthropic"),
            "codex" => Some("openai"),
            "antigravity" => Some("google"),
            _ => None,
        };
        if let Some(cost) = own.and_then(|maker| entry(maker, model)) {
            return Some(cost);
        }
        let makers = self.makers.get(model)?;
        let mut makers = makers.iter();
        match (makers.next(), makers.next()) {
            (Some(maker), None) => entry(maker, model),
            _ => None,
        }
    }

    /// What `usage` costs at the list's price, in USD; a cache counter the model has no price
    /// for is billed as input. Only `usage` known to be `one_request` can tell a request past a
    /// context tier, which is priced at that tier; a sum of several takes the base price.
    pub(crate) fn price(
        &self,
        harness: &str,
        model: &str,
        provider: Option<&str>,
        usage: &TokenUsage,
        one_request: bool,
    ) -> Option<Cost> {
        let base = self.rates(harness, model, provider)?;
        let input = usage.input_tokens?;
        let output = usage.output_tokens?;
        let cache_read = usage.cache_read_tokens.unwrap_or(0);
        let cache_write = usage.cache_write_tokens.unwrap_or(0);
        let plain = input.checked_sub(cache_read)?.checked_sub(cache_write)?;

        let tiers = base["tiers"].as_array().cloned().unwrap_or_else(|| {
            base.get("context_over_200k")
                .filter(|tier| tier.is_object())
                .map(|tier| {
                    let mut tier = tier.clone();
                    tier["tier"] = serde_json::json!({"type": "context", "size": 200_000});
                    vec![tier]
                })
                .unwrap_or_default()
        });
        let tier = tiers
            .iter()
            .filter(|tier| tier["tier"]["type"] == "context")
            .filter_map(|tier| Some((tier["tier"]["size"].as_u64()?, tier)))
            .filter(|(size, _)| one_request && input > *size)
            .max_by_key(|(size, _)| *size)
            .map(|(_, tier)| tier);
        let rate = |key: &str| {
            tier.and_then(|tier| tier[key].as_f64())
                .or_else(|| base[key].as_f64())
                .filter(|rate| rate.is_finite() && *rate >= 0.0)
        };
        let input_rate = rate("input")?;
        let micros = plain as f64 * input_rate
            + output as f64 * rate("output")?
            + cache_read as f64 * rate("cache_read").unwrap_or(input_rate)
            + cache_write as f64 * rate("cache_write").unwrap_or(input_rate);
        // Rates are dollars per million tokens, so tokens × rate is millionths of a dollar.
        Some(Cost {
            micros: micros.round() as u64,
            currency: "USD".into(),
        })
    }
}

/// The last good list on disk, parsed once per change of the file.
pub(crate) fn current() -> Option<Arc<PriceList>> {
    static LOADED: Mutex<Option<(SystemTime, Arc<PriceList>)>> = Mutex::new(None);
    let path = path();
    let modified = std::fs::metadata(&path).and_then(|meta| meta.modified());
    let modified = match modified {
        Ok(modified) => modified,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            eprintln!("orx: could not read {}: {error}", path.display());
            return None;
        }
    };
    let mut loaded = LOADED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((at, list)) = &*loaded {
        if *at == modified {
            return Some(list.clone());
        }
    }
    match std::fs::read(&path)
        .map_err(Into::into)
        .and_then(|bytes| PriceList::parse(&bytes))
    {
        Ok(list) => {
            let list = Arc::new(list);
            *loaded = Some((modified, list.clone()));
            Some(list)
        }
        Err(error) => {
            eprintln!(
                "orx: could not read model prices in {}: {error:#}",
                path.display()
            );
            loaded.as_ref().map(|(_, list)| list.clone())
        }
    }
}

/// Replaces the list at `path` with `url`'s, only once it parses.
async fn fetch(http: &reqwest::Client, url: &str, path: &Path) -> Result<()> {
    let response = http.get(url).send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("{url} answered {status}"));
    }
    let bytes = response.bytes().await?;
    PriceList::parse(&bytes).map_err(|error| anyhow!("{url} sent an unreadable list: {error}"))?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent)?;
    crate::local::git::atomic_write_with_mode(path, &bytes, None)
}

fn stale(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_none_or(|age| age >= MAX_AGE)
}

/// Set by the integration tests' launched `orx up`, which must not reach models.dev.
const OFF: &str = "ORX_NO_MODEL_PRICES";

/// For `orx up`'s life: fetches the list when it is missing or a day old, checking hourly.
pub async fn keep_fresh() {
    if std::env::var_os(OFF).is_some() {
        return;
    }
    let http = match crate::net::remote_client()
        .timeout(Duration::from_secs(60))
        .build()
    {
        Ok(http) => http,
        Err(error) => {
            eprintln!("orx up: could not build the model price client: {error}");
            return;
        }
    };
    let path = path();
    loop {
        if stale(&path) {
            if let Err(error) = fetch(&http, URL, &path).await {
                eprintln!("orx up: could not refresh model prices from {URL}: {error:#}");
            }
        }
        tokio::time::sleep(Duration::from_secs(60 * 60)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"{
        "openai": {"models": {
            "gpt-5.5": {"cost": {"input": 5, "output": 30, "cache_read": 0.5,
                "tiers": [{"input": 10, "output": 45, "cache_read": 1, "tier": {"type": "context", "size": 272000}}],
                "context_over_200k": {"input": 99, "output": 99}}},
            "gpt-free": {"cost": {"input": 0, "output": 0}}
        }},
        "anthropic": {"models": {
            "claude-sonnet-5-5": {"cost": {"input": 2, "output": 10, "cache_read": 0.1, "cache_write": 2.5}, "canonical_model_id": "anthropic/claude-sonnet-5-5"}
        }},
        "google": {"models": {
            "gemini-9": {"cost": {"input": 1, "output": 4, "context_over_200k": {"input": 2, "output": 8}}}
        }},
        "resale": {"models": {
            "gpt-5.5": {"cost": {"input": 6, "output": 31}, "canonical_model_id": "openai/gpt-5.5"},
            "claude-sonnet-5-5": {"cost": {"input": 3, "output": 11}, "canonical_model_id": "anthropic/claude-sonnet-5-5"},
            "twin": {"cost": {"input": 1, "output": 1}, "canonical_model_id": "a/twin"},
            "twin-b": {"cost": {"input": 1, "output": 1}, "canonical_model_id": "b/twin"},
            "broken": {"cost": "free"}
        }},
        "a": {"models": {"twin": {"cost": {"input": 1, "output": 1}}}},
        "b": {"models": {"twin": {"cost": {"input": 2, "output": 2}}}}
    }"#;

    fn tokens(input: u64, output: u64, read: u64, write: u64) -> TokenUsage {
        TokenUsage {
            input_tokens: Some(input),
            output_tokens: Some(output),
            cache_read_tokens: Some(read),
            cache_write_tokens: Some(write),
            reasoning_tokens: None,
        }
    }

    fn micros(list: &PriceList, harness: &str, model: &str, provider: Option<&str>) -> Option<u64> {
        list.price(
            harness,
            model,
            provider,
            &tokens(1_000, 100, 400, 100),
            true,
        )
        .map(|cost| cost.micros)
    }

    #[test]
    fn prices_each_counter_at_its_own_rate_in_usd() {
        let list = PriceList::parse(LIST.as_bytes()).unwrap();
        let cost = list
            .price(
                "claude-code",
                "claude-sonnet-5-5",
                None,
                &tokens(1_000, 100, 400, 100),
                true,
            )
            .unwrap();
        // 500 plain × 2 + 100 out × 10 + 400 read × 0.1 + 100 write × 2.5.
        assert_eq!(cost.micros, 1_000 + 1_000 + 40 + 250);
        assert_eq!(cost.currency, "USD");
        // OpenAI has no cache-write price: those tokens are input.
        assert_eq!(
            micros(&list, "codex", "gpt-5.5", None),
            Some(500 * 5 + 100 * 30 + 200 + 100 * 5)
        );
        assert_eq!(micros(&list, "codex", "gpt-free", None), Some(0));
    }

    #[test]
    fn a_request_past_a_context_tier_pays_that_tier() {
        let list = PriceList::parse(LIST.as_bytes()).unwrap();
        let big = TokenUsage {
            input_tokens: Some(300_000),
            output_tokens: Some(1_000),
            ..Default::default()
        };
        // `tiers` wins over the older `context_over_200k` when a model has both.
        assert_eq!(
            list.price("codex", "gpt-5.5", None, &big, true)
                .unwrap()
                .micros,
            300_000 * 10 + 1_000 * 45
        );
        let under = TokenUsage {
            input_tokens: Some(250_000),
            output_tokens: Some(1_000),
            ..Default::default()
        };
        assert_eq!(
            list.price("codex", "gpt-5.5", None, &under, true)
                .unwrap()
                .micros,
            250_000 * 5 + 1_000 * 30
        );
        assert_eq!(
            list.price("antigravity", "gemini-9", None, &under, true)
                .unwrap()
                .micros,
            250_000 * 2 + 1_000 * 8
        );
    }

    #[test]
    fn a_whole_turns_tokens_pay_the_base_price() {
        let list = PriceList::parse(LIST.as_bytes()).unwrap();
        // Three requests of 100k each sum past the tier none of them reached.
        let turn = TokenUsage {
            input_tokens: Some(300_000),
            output_tokens: Some(1_000),
            ..Default::default()
        };
        assert_eq!(
            list.price("cursor", "gpt-5.5", Some("openai"), &turn, false)
                .unwrap()
                .micros,
            300_000 * 5 + 1_000 * 30
        );
        assert_eq!(
            list.price("antigravity", "gemini-9", None, &turn, false)
                .unwrap()
                .micros,
            300_000 + 1_000 * 4
        );
    }

    #[tokio::test]
    async fn a_launched_orx_up_in_tests_never_fetches_the_list() {
        if !crate::config::in_test_child(&[OFF]) {
            return;
        }
        std::env::set_var(OFF, "1");
        tokio::time::timeout(Duration::from_secs(5), keep_fresh())
            .await
            .expect("returns at once");
        assert!(!path().exists());
    }

    #[test]
    fn matches_only_an_exact_model_of_a_known_maker() {
        let list = PriceList::parse(LIST.as_bytes()).unwrap();
        // The reporting provider's own price, and nothing when it doesn't list the model.
        assert_eq!(
            micros(&list, "opencode", "gpt-5.5", Some("resale")),
            Some(600 * 6 + 100 * 31 + 400 * 6)
        );
        assert_eq!(micros(&list, "opencode", "gpt-5.5", Some("custom")), None);
        // A bare label resolves to its maker through the canonical ids, never a reseller.
        assert_eq!(
            micros(&list, "cursor", "claude-sonnet-5-5", None),
            micros(&list, "claude-code", "claude-sonnet-5-5", None)
        );
        assert_eq!(
            micros(&list, "cursor", "openai/gpt-5.5", None),
            micros(&list, "codex", "gpt-5.5", None)
        );
        // Two makers claim the id: ambiguous.
        assert_eq!(micros(&list, "cursor", "twin", None), None);
        assert_eq!(micros(&list, "cursor", "gpt-5", None), None);
        assert_eq!(micros(&list, "cursor", "broken", Some("resale")), None);
    }

    #[test]
    fn partial_or_inconsistent_counters_have_no_price() {
        let list = PriceList::parse(LIST.as_bytes()).unwrap();
        let no_output = TokenUsage {
            input_tokens: Some(10),
            ..Default::default()
        };
        assert!(list
            .price("codex", "gpt-5.5", None, &no_output, true)
            .is_none());
        assert!(list
            .price("codex", "gpt-5.5", None, &tokens(10, 1, 20, 0), true)
            .is_none());
    }

    #[tokio::test]
    async fn a_failed_fetch_keeps_the_last_good_list() {
        use axum::http::StatusCode;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let answers = [
            (StatusCode::OK, LIST),
            (StatusCode::SERVICE_UNAVAILABLE, "busy"),
            (StatusCode::OK, "<html>"),
        ];
        let next = Arc::new(AtomicUsize::new(0));
        let served = next.clone();
        let app = axum::Router::new().route(
            "/api.json",
            axum::routing::get(move || {
                let answer = answers[served.fetch_add(1, Ordering::SeqCst)];
                async move { answer }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api.json", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = std::env::temp_dir().join(format!("orx-prices-{}", uuid::Uuid::new_v4()));
        let path = dir.join("models.dev.json");
        let http = crate::net::loopback_client().build().unwrap();

        assert!(stale(&path));
        fetch(&http, &url, &path).await.unwrap();
        assert!(!stale(&path));
        assert!(fetch(&http, &url, &path).await.is_err());
        assert!(fetch(&http, &url, &path).await.is_err());
        let kept = PriceList::parse(&std::fs::read(&path).unwrap()).unwrap();
        assert!(micros(&kept, "codex", "gpt-5.5", None).is_some());
        assert_eq!(next.load(Ordering::SeqCst), 3);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
