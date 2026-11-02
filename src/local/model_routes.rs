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

/// The provider-neutral id of a native model id: vendor prefixes, dated
/// snapshots, `-free` and context suffixes are route properties, and version
/// digits join with `.` whichever separator the route used.
pub fn canonical(id: &str) -> String {
    let id = id.rsplit('/').next().unwrap_or(id);
    let id = id
        .split('[')
        .next()
        .unwrap_or(id)
        .trim()
        .to_ascii_lowercase();
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
        return id.to_string();
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

/// Every model the ready harnesses in a `/api/harnesses` payload can run.
pub fn models(catalog: &Value) -> Vec<Model> {
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
                name: name.to_string(),
                harness: harness_id.to_string(),
                model: native.to_string(),
                free: native.ends_with("-free"),
                efforts,
            };
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
        ] {
            assert_eq!(canonical(native), id, "{native}");
        }
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
        let models = models(&catalog());
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
        let models = models(&catalog());
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
}
