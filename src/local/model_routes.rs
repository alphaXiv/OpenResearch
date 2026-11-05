//! Model-first routing for delegated chats.
//!
//! An agent names a versioned model (`claude-opus-5.5`); orx finds every way
//! the user can run it — a route is one harness plus that harness's own model
//! id — and picks one by a default order unless the caller names a route.
//! The same model is spelled differently per route (`opus[1m]`,
//! `openrouter/anthropic/claude-opus-5.5`, `opencode/claude-opus-5-5`), so
//! routes are joined on a canonical id derived from each native id.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub profile: Option<String>,
    /// What the agent names: the OpenCode provider, else the harness id.
    pub name: String,
    pub harness: String,
    /// The harness's own model id, stored as the chat's model.
    pub model: String,
    pub free: bool,
    /// The effort levels this route takes, as the chat's reasoning level.
    pub efforts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub id: String,
    /// Default order first.
    pub routes: Vec<Route>,
}

/// Aggregators resell other providers' models and come last by default.
const AGGREGATORS: &[&str] = &["openrouter"];

/// Whether a native model id names a free tier (`opencode/…-free`,
/// OpenRouter's `…:free`).
pub fn free(id: &str) -> bool {
    id.ends_with("-free") || id.ends_with(":free")
}

/// The provider-neutral id of a native model id: vendor prefixes, dated
/// snapshots, free-tier and context suffixes are route properties, and version
/// digits join with `.` whichever separator the route used.
pub fn canonical(id: &str) -> String {
    let id = id.rsplit('/').next().unwrap_or(id);
    let id = id
        .split('[')
        .next()
        .unwrap_or(id)
        .trim()
        .to_ascii_lowercase();
    let id = id.strip_suffix(":free").unwrap_or(&id);
    let mut parts: Vec<&str> = id.split('-').collect();
    if parts.len() > 1 && parts.last() == Some(&"free") {
        parts.pop();
    }
    let snapshot = |part: &&str| part.len() >= 4 && part.bytes().all(|b| b.is_ascii_digit());
    if parts.len() > 1 && parts.last().is_some_and(snapshot) {
        parts.pop();
    }
    let mut joined: Vec<String> = Vec::new();
    for part in parts {
        let numeric = !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
        match joined.last_mut() {
            Some(version)
                if numeric
                    && version.starts_with(|c: char| c.is_ascii_digit())
                    && version.bytes().all(|b| b.is_ascii_digit() || b == b'.') =>
            {
                version.push('.');
                version.push_str(part);
            }
            _ => joined.push(part.to_string()),
        }
    }
    joined.join("-")
}

/// Lower is preferred: the vendor's own harness, then flat-rate OpenCode
/// plans, direct APIs, other harnesses, and aggregators last.
fn rank(model: &str, route: &Route) -> u8 {
    let first_party = match route.harness.as_str() {
        "claude-code" => model.starts_with("claude-"),
        "codex" => model.starts_with("gpt-"),
        _ => false,
    };
    if first_party {
        return 0;
    }
    if route.harness != "opencode" {
        return 4;
    }
    match route.name.as_str() {
        "opencode-go" => 1,
        "opencode" => 2,
        name if AGGREGATORS.contains(&name) => 5,
        _ => 3,
    }
}

/// The route name an agent passes back for a harness. ACP agents are known by
/// a slug of their name, since their ids are opaque; a slug another harness
/// already took keeps the id.
fn harness_route_name(harness: &Value, id: &str, taken: &[String]) -> String {
    if !id.starts_with("acp:") {
        return harness
            .pointer("/profile/id")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_string();
    }
    let name = harness.get("name").and_then(Value::as_str).unwrap_or(id);
    let slug = name
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() || taken.contains(&slug) {
        id.to_string()
    } else {
        slug
    }
}

/// Whether the user turned `harness`, an entry of `/api/harnesses`, off in Settings → Agents. Only
/// a found agent has a switch; turning OpenCode off drops just the providers it signed in to itself.
pub fn turned_off(harness: &Value, hidden: &[String]) -> bool {
    let id = harness["id"].as_str().unwrap_or_default();
    let kept_install =
        harness["managed"] == true && !matches!(id, "claude-code" | "codex" | "opencode");
    harness["profile"].is_null() && !kept_install && hidden.iter().any(|hidden| hidden == id)
}

/// Remove routes belonging to found agents the user turned off, before model deduplication.
pub fn filter_agents(
    catalog: &mut Value,
    hidden: &[String],
    owned_providers: &[String],
    requested: Option<&str>,
) -> Result<(), String> {
    let mut names = Vec::new();
    for harness in catalog["harnesses"].as_array_mut().into_iter().flatten() {
        let Some(id) = harness["id"].as_str().map(str::to_string) else {
            continue;
        };
        let name = harness_route_name(harness, &id, &names);
        if harness["agentReady"] == true {
            names.push(name.clone());
        }
        if !turned_off(harness, hidden) {
            continue;
        }
        let refusal = |route| {
            format!("Agent `{route}` is turned off in Settings → Agents. Choose another route or ask the user to turn it on.")
        };
        if let Some(route) =
            requested.filter(|route| id != "opencode" && (*route == id || *route == name))
        {
            return Err(refusal(route));
        }
        if let Some(entries) = harness["models"].as_array_mut() {
            if id != "opencode" {
                entries.clear();
                continue;
            }
            let is_owned = |entry: &Value| {
                entry["id"].as_str().is_some_and(|native| {
                    let provider = native.split('/').next().unwrap_or(native);
                    provider == "opencode" || owned_providers.iter().any(|id| id == provider)
                })
            };
            if let Some(route) = requested.filter(|route| {
                entries.iter().any(|entry| {
                    !is_owned(entry)
                        && entry["id"]
                            .as_str()
                            .is_some_and(|id| id.split('/').next() == Some(*route))
                })
            }) {
                return Err(refusal(route));
            }
            entries.retain(is_owned);
        }
    }
    Ok(())
}

/// Every model the ready harnesses in a `/api/harnesses` payload can run,
/// less the routes `hidden` turns off: a model's canonical id, with `:free`
/// appended for its free tier.
pub fn models(catalog: &Value, hidden: &[String]) -> Vec<Model> {
    let mut models: Vec<Model> = Vec::new();
    let mut harness_names: Vec<String> = Vec::new();
    let harnesses = catalog.get("harnesses").and_then(Value::as_array);
    for harness in harnesses.into_iter().flatten() {
        if harness.get("agentReady").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let Some(harness_id) = harness.get("id").and_then(Value::as_str) else {
            continue;
        };
        let harness_name = harness_route_name(harness, harness_id, &harness_names);
        harness_names.push(harness_name.clone());
        let entries = harness.get("models").and_then(Value::as_array);
        for entry in entries.into_iter().flatten() {
            let Some(native) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            if !crate::local::harness::concrete_model(native) {
                continue;
            }
            let resolved = entry.get("resolvedModel").and_then(Value::as_str);
            let id = canonical(resolved.unwrap_or(native));
            let name = match harness_id {
                "opencode" => native.split('/').next().unwrap_or(native),
                _ => harness_name.as_str(),
            };
            let efforts = entry
                .get("reasoningLevels")
                .or_else(|| harness.pointer("/options/reasoningLevels"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|level| level.get("id").and_then(Value::as_str))
                .filter(|id| *id != crate::local::harness::REASONING_DEFAULT_ID)
                .map(str::to_string)
                .collect();
            let route = Route {
                profile: harness
                    .pointer("/profile/id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                name: name.to_string(),
                harness: harness_id.to_string(),
                model: native.to_string(),
                free: free(native),
                efforts,
            };
            let hide_id = if route.free {
                format!("{id}:free")
            } else {
                id.clone()
            };
            if hidden.contains(&hide_id) {
                continue;
            }
            let index = match models.iter().position(|model| model.id == id) {
                Some(index) => index,
                None => {
                    models.push(Model {
                        id,
                        routes: Vec::new(),
                    });
                    models.len() - 1
                }
            };
            let routes = &mut models[index].routes;
            match routes
                .iter_mut()
                .find(|existing| existing.name == route.name)
            {
                // A paid listing beats the same provider's free tier.
                Some(existing) if existing.free && !route.free => *existing = route,
                Some(_) => {}
                None => routes.push(route),
            }
        }
    }
    for model in &mut models {
        let id = model.id.clone();
        model.routes.sort_by_key(|route| rank(&id, route));
    }
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models
}

/// The route a delegate runs on. Errors are written for the calling agent.
pub fn resolve(models: &[Model], id: &str, route: Option<&str>) -> Result<Route, String> {
    let wanted = canonical(id);
    let Some(model) = models.iter().find(|model| model.id == wanted) else {
        let query = id.trim().to_ascii_lowercase();
        let similar: Vec<&str> = models
            .iter()
            .map(|model| model.id.as_str())
            .filter(|candidate| candidate.contains(&query) || query.contains(*candidate))
            .collect();
        return Err(if similar.is_empty() {
            format!("No route can run `{id}`. Call `models` for the versioned models available.")
        } else {
            format!("Name a versioned model: {}.", similar.join(", "))
        });
    };
    match route {
        None => Ok(model.routes[0].clone()),
        Some(name) => model
            .routes
            .iter()
            .find(|candidate| candidate.name == name)
            .cloned()
            .ok_or_else(|| {
                let names: Vec<&str> = model.routes.iter().map(|r| r.name.as_str()).collect();
                format!("`{}` runs only on: {}.", model.id, names.join(", "))
            }),
    }
}

/// One line per model, routes in default order, for the agent to read.
pub fn listing(models: &[Model]) -> String {
    models
        .iter()
        .map(|model| {
            let routes: Vec<String> = model
                .routes
                .iter()
                .map(|route| {
                    if route.free {
                        format!("{} (free)", route.name)
                    } else {
                        route.name.clone()
                    }
                })
                .collect();
            let efforts = &model.routes[0].efforts;
            if efforts.is_empty() {
                format!("{}  {}", model.id, routes.join(" · "))
            } else {
                format!(
                    "{}  {}  effort: {}",
                    model.id,
                    routes.join(" · "),
                    efforts.join(" ")
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hidden_agents_keep_profiles_and_managed_opencode_providers() {
        let mut catalog = json!({"harnesses":[
            {"id":"codex","agentReady":true,"models":[{"id":"gpt-6"}]},
            {"id":"codex","profile":{"id":"codex-work"},"agentReady":true,"models":[{"id":"gpt-6"}]},
            {"id":"cursor","agentReady":true,"models":[{"id":"gpt-6"}]},
            {"id":"acp:deepseek","name":"DeepSeek Harness","agentReady":true,"models":[{"id":"deepseek-v4"}]},
            {"id":"opencode","agentReady":true,"models":[
                {"id":"own-key/gpt-6"}, {"id":"local-endpoint/gpt-6"},
                {"id":"opencode/gpt-6-free"}, {"id":"opencode/gpt-6"},
                {"id":"external-signin/gpt-6"}
            ]}
        ]});
        let hidden = ["codex", "cursor", "acp:deepseek", "opencode"].map(str::to_string);
        let owned = ["own-key", "local-endpoint"].map(str::to_string);
        for route in [
            "codex",
            "cursor",
            "acp:deepseek",
            "deepseek-harness",
            "external-signin",
        ] {
            let error =
                filter_agents(&mut catalog.clone(), &hidden, &owned, Some(route)).unwrap_err();
            assert_eq!(error, format!("Agent `{route}` is turned off in Settings → Agents. Choose another route or ask the user to turn it on."));
        }
        filter_agents(&mut catalog, &hidden, &owned, Some("codex-work")).unwrap();
        let visible = models(&catalog, &[]);
        let names: Vec<_> = visible[0]
            .routes
            .iter()
            .map(|route| route.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["codex-work", "opencode", "own-key", "local-endpoint"]
        );
        assert!(visible[0].routes.iter().all(|route| !route.free));
        assert_eq!(
            resolve(&visible, "gpt-6", None).unwrap().profile.as_deref(),
            Some("codex-work")
        );
        let free_only = models(&catalog, &["gpt-6".into()]);
        assert_eq!(free_only[0].routes.len(), 1);
        assert!(free_only[0].routes[0].free);
        filter_agents(&mut catalog, &hidden, &owned, Some("opencode")).unwrap();
    }

    #[test]
    fn hidden_agents_keep_removable_managed_installs() {
        let mut catalog = json!({"harnesses":[
            {"id":"codex","managed":true,"agentReady":true,"models":[{"id":"gpt-6"}]},
            {"id":"codex","managed":true,"profile":{"id":"codex-work"},"agentReady":true,"models":[{"id":"gpt-6"}]},
            {"id":"cursor","managed":true,"agentReady":true,"models":[{"id":"gpt-6"}]},
            {"id":"acp:dsh","name":"DeepSeek Harness","managed":true,"agentReady":true,"models":[{"id":"deepseek-v4"}]}
        ]});
        let hidden = ["codex", "cursor", "acp:dsh"].map(str::to_string);
        assert!(filter_agents(&mut catalog.clone(), &hidden, &[], Some("codex")).is_err());
        filter_agents(&mut catalog, &hidden, &[], Some("cursor")).unwrap();
        let visible = models(&catalog, &[]);
        assert!(resolve(&visible, "gpt-6", Some("cursor")).is_ok());
        assert!(resolve(&visible, "gpt-6", Some("codex-work")).is_ok());
        assert!(resolve(&visible, "gpt-6", Some("codex")).is_err());
        assert!(resolve(&visible, "deepseek-v4", Some("deepseek-harness")).is_ok());
    }

    #[test]
    fn profiles_keep_distinct_delegate_routes() {
        let catalog = json!({"harnesses": [
            {"id":"codex", "agentReady":true, "models":[{"id":"gpt-6"}]},
            {"id":"codex", "profile":{"id":"codex-012345abcd","name":"Work"}, "agentReady":true,"models":[{"id":"gpt-6"}]},
            {"id":"codex", "profile":{"id":"codex-abcdefghij","name":"Personal"}, "agentReady":true,"models":[{"id":"gpt-6"}]}
        ]});
        let models = models(&catalog, &[]);
        assert_eq!(models[0].routes.len(), 3);
        let route = resolve(&models, "gpt-6", Some("codex-abcdefghij")).unwrap();
        assert_eq!(route.harness, "codex");
        assert_eq!(route.profile.as_deref(), Some("codex-abcdefghij"));
    }

    #[test]
    fn routes_spell_one_model_differently() {
        for (native, id) in [
            ("claude-opus-5-5", "claude-opus-5.5"),
            ("anthropic/claude-opus-5.5", "claude-opus-5.5"),
            ("openrouter/anthropic/claude-opus-5.5", "claude-opus-5.5"),
            ("claude-haiku-4-5-20251001", "claude-haiku-4.5"),
            ("claude-fable-5[1m]", "claude-fable-5"),
            ("gpt-6.1-sol", "gpt-6.1-sol"),
            ("opencode/deepseek-v4.1-flash-free", "deepseek-v4.1-flash"),
            ("deepseek/deepseek-v4-pro-0813", "deepseek-v4-pro"),
            (
                "moonshotai/kimi-k3.1-pro-highspeed",
                "kimi-k3.1-pro-highspeed",
            ),
            ("meta/llama-3-1-70b", "llama-3.1-70b"),
            ("openrouter/deepseek/deepseek-r1:free", "deepseek-r1"),
        ] {
            assert_eq!(canonical(native), id, "{native}");
        }
        assert!(free("openrouter/deepseek/deepseek-r1:free"));
        assert!(free("opencode/deepseek-v4.1-flash-free"));
        assert!(!free("opencode-go/deepseek-v4.1-flash"));
    }

    fn catalog() -> Value {
        json!({"harnesses": [
            {"id": "claude-code", "agentReady": true,
             "options": {"reasoningLevels": [{"id": "default"}, {"id": "low"}, {"id": "max"}]},
             "models": [{"id": "opus[1m]", "resolvedModel": "claude-opus-5-5"}]},
            {"id": "codex", "agentReady": true,
             "options": {"reasoningLevels": [{"id": "default"}, {"id": "low"}]},
             "models": [{"id": "gpt-6-luna", "reasoningLevels": [{"id": "high"}, {"id": "xhigh"}]}]},
            {"id": "opencode", "agentReady": true, "models": [
                {"id": "openrouter/anthropic/claude-opus-5.5"},
                {"id": "opencode/deepseek-v4.1-flash-free"},
                {"id": "opencode-go/deepseek-v4.1-flash"},
                {"id": "opencode/gpt-6-luna"},
            ]},
            {"id": "cursor", "agentReady": false, "models": [{"id": "claude-opus-5-5"}]},
            {"id": "acp:1", "name": "OpenCode (ACP)", "agentReady": true, "models": [
                {"id": "opencode/deepseek-v4.1-flash-free"},
            ]},
            {"id": "acp:2", "name": "opencode acp", "agentReady": true, "models": [
                {"id": "opencode/deepseek-v4.1-flash-free"},
            ]},
        ]})
    }

    #[test]
    fn default_order_prefers_the_vendor_harness_then_plans_then_aggregators() {
        let models = models(&catalog(), &[]);
        let names = |id: &str| -> Vec<String> {
            let model = models.iter().find(|model| model.id == id).unwrap();
            model
                .routes
                .iter()
                .map(|route| route.name.clone())
                .collect()
        };
        assert_eq!(names("claude-opus-5.5"), ["claude-code", "openrouter"]);
        assert_eq!(
            names("deepseek-v4.1-flash"),
            ["opencode-go", "opencode", "opencode-acp", "acp:2"]
        );
        assert_eq!(names("gpt-6-luna"), ["codex", "opencode"]);
        let listing = listing(&models);
        assert!(listing
            .contains("deepseek-v4.1-flash  opencode-go · opencode (free) · opencode-acp (free)"));
        assert!(
            listing.contains("gpt-6-luna  codex · opencode  effort: high xhigh"),
            "a model's own levels beat its harness's"
        );
        assert!(listing.contains("claude-opus-5.5  claude-code · openrouter  effort: low max"));
    }

    #[test]
    fn resolution_translates_to_the_route_and_asks_for_a_version() {
        let models = models(&catalog(), &[]);
        let route = resolve(&models, "claude-opus-5.5", None).unwrap();
        assert_eq!(
            (route.harness.as_str(), route.model.as_str()),
            ("claude-code", "opus[1m]")
        );
        let route = resolve(&models, "claude-opus-5.5", Some("openrouter")).unwrap();
        assert_eq!(route.model, "openrouter/anthropic/claude-opus-5.5");
        assert_eq!(
            resolve(&models, "deepseek", None).unwrap_err(),
            "Name a versioned model: deepseek-v4.1-flash."
        );
        assert!(resolve(&models, "claude-opus-5.5", Some("codex"))
            .unwrap_err()
            .contains("claude-code, openrouter"));
    }

    #[test]
    fn hidden_routes_leave_delegation() {
        let catalog = json!({"harnesses": [
            {"id": "opencode", "agentReady": true, "models": [
                {"id": "opencode/deepseek-v4.1-flash"},
                {"id": "opencode/deepseek-v4.1-flash-free"},
                {"id": "openrouter/deepseek/deepseek-v4.1-flash:free"},
            ]},
        ]});
        let hidden = |ids: &[&str]| {
            models(
                &catalog,
                &ids.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
            )
        };
        let routes = |models: &[Model]| -> Vec<(String, String)> {
            models
                .iter()
                .flat_map(|model| &model.routes)
                .map(|route| (route.name.clone(), route.model.clone()))
                .collect()
        };
        assert_eq!(
            routes(&hidden(&["deepseek-v4.1-flash"])),
            [
                (
                    "opencode".into(),
                    "opencode/deepseek-v4.1-flash-free".into()
                ),
                (
                    "openrouter".into(),
                    "openrouter/deepseek/deepseek-v4.1-flash:free".into()
                ),
            ],
            "hiding the paid listing keeps its free tier"
        );
        let gone = hidden(&["deepseek-v4.1-flash", "deepseek-v4.1-flash:free"]);
        assert!(gone.is_empty());
        assert!(resolve(&gone, "deepseek-v4.1-flash", None)
            .unwrap_err()
            .starts_with("No route can run"));
    }
}
