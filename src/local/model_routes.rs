//! Model-first routing for delegated chats.
//!
//! An agent names a versioned model (`claude-opus-5.5`); orx finds every way
//! the user can run it — a route is one harness plus that harness's own model
//! id — and picks one by a default order unless the caller names a route.
//! The same model is spelled differently per route (`opus[1m]`,
//! `openrouter/anthropic/claude-opus-5.5`, `opencode/claude-opus-5-5`), so
//! routes are joined on a canonical id derived from each native id.

use serde::Serialize;
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

/// Effort words in rising order, as Cursor and Antigravity spell them into model ids.
const EFFORTS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max"];
/// Claude's tiers: a name that spells one is Claude's.
const CLAUDE_TIERS: &[&str] = &["mythos", "fable", "opus", "sonnet", "haiku"];
/// Words a lab reuses across versions, so a newer version of any of them makes an older one old.
const TIER_WORDS: &[&str] = &[
    "flash",
    "pro",
    "max",
    "plus",
    "mini",
    "nano",
    "lite",
    "air",
    "turbo",
    "ultra",
    "fast",
    "instant",
    "preview",
    "exp",
    "chat",
    "instruct",
    "thinking",
    "reasoning",
    "reasoner",
];

/// What an offer adds to its model: Cursor's and Antigravity's effort, speed and thinking, Claude Code's long context.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Variant {
    pub effort: Option<String>,
    pub fast: bool,
    pub thinking: bool,
    pub context: Option<String>,
}

/// One catalog entry read as a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelName {
    /// The same for one model in every agent's catalog, `:free` added for its free tier: what the picker groups and
    /// Settings hides by.
    pub key: String,
    /// `key` without its version or free tier: what a favorite stars.
    pub family: String,
    /// `family` without tier words a lab reuses across versions.
    pub series: String,
    pub version: Option<Vec<u64>>,
    /// The catalog's name without variant words, dates or asides.
    pub label: String,
    pub variant: Variant,
    /// How far the catalog's name is from its model's plain variant; the lowest runs by default.
    pub variant_rank: usize,
}

/// Cursor and Antigravity list each effort, speed and thinking mode as a model of its own.
fn spells_variants(harness: &str) -> bool {
    matches!(harness, "cursor" | "antigravity")
}

fn is_version(word: &str) -> bool {
    word.split('.')
        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

fn is_date(word: &str) -> bool {
    matches!(word.len(), 4 | 8) && word.bytes().all(|b| b.is_ascii_digit())
}

/// `1m`, `200k`: a context window.
fn is_context(word: &str) -> bool {
    word.len() > 1
        && matches!(word.as_bytes()[word.len() - 1], b'm' | b'k')
        && word[..word.len() - 1].bytes().all(|b| b.is_ascii_digit())
}

/// `35b`, `a3b`: a parameter count.
fn is_size(word: &str) -> bool {
    let digits = word
        .strip_suffix('b')
        .map(|rest| rest.strip_prefix(['a', 'e']).unwrap_or(rest));
    digits.is_some_and(|digits| {
        let (whole, fraction) = digits.split_once('.').unwrap_or((digits, "0"));
        [whole, fraction]
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// A lowercase token's words: a version split from the word it's glued to (`qwen3.8`, `k3`), a size kept whole (`a3b`).
fn token_words(token: &str) -> Vec<String> {
    let bytes = token.as_bytes();
    let mut words = vec![String::new()];
    for (index, ch) in token.char_indices() {
        let byte = bytes[index];
        let glued = index > 0 && byte.is_ascii_digit() && bytes[index - 1].is_ascii_lowercase();
        if glued {
            let digits = |from: usize| {
                from + bytes[from..]
                    .iter()
                    .take_while(|b| b.is_ascii_digit())
                    .count()
            };
            let mut end = digits(index);
            if bytes.get(end) == Some(&b'.') && bytes.get(end + 1).is_some_and(u8::is_ascii_digit) {
                end = digits(end + 1);
            }
            let size = bytes.get(end) == Some(&b'b')
                && !bytes
                    .get(end + 1)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
            if !size {
                words.push(String::new());
            }
        }
        words.last_mut().unwrap().push(ch);
    }
    words
}

/// A name read word by word: the words a label keeps and the words a key is made of.
#[derive(Default)]
struct Reading {
    kept: Vec<String>,
    keys: Vec<String>,
    variant_words: usize,
}

fn read(name: &str, spells: bool, variant: &mut Variant) -> Reading {
    let mut reading = Reading::default();
    let mut seen_version = false;
    for word in name.split_whitespace() {
        let lower = word.to_lowercase();
        if spells
            && (EFFORTS.contains(&lower.as_str())
                || matches!(lower.as_str(), "extra" | "fast" | "thinking" | "no")
                || is_context(&lower))
        {
            if is_context(&lower) {
                variant.context = Some(lower);
            }
            reading.variant_words += 1;
            continue;
        }
        if seen_version && is_date(&lower) {
            continue;
        }
        reading.kept.push(word.to_string());
        // `5-5` spells `5.5`.
        let mut joins = false;
        for token in lower.split(['-', '_']).filter(|token| !token.is_empty()) {
            if seen_version && is_date(token) {
                continue;
            }
            if joins && token.bytes().all(|b| b.is_ascii_digit()) {
                let version = reading.keys.last_mut().unwrap();
                version.push('.');
                version.push_str(token);
                continue;
            }
            let words = token_words(token);
            seen_version |= words.iter().any(|word| is_version(word));
            joins = is_version(token);
            reading.keys.extend(words);
        }
    }
    reading
}

fn id_variant(harness: &str, id: &str) -> Variant {
    let mut variant = Variant::default();
    if harness == "claude-code" {
        variant.context = id
            .strip_suffix(']')
            .and_then(|rest| rest.rsplit_once('['))
            .map(|(_, context)| context.to_ascii_lowercase())
            .filter(|context| is_context(context));
        return variant;
    }
    if !spells_variants(harness) {
        return variant;
    }
    let lower = id.to_ascii_lowercase();
    let mut parts: Vec<&str> = lower.split('-').collect();
    while parts.len() > 1 {
        match *parts.last().unwrap() {
            "fast" => variant.fast = true,
            "thinking" => variant.thinking = true,
            effort if EFFORTS.contains(&effort) && variant.effort.is_none() => {
                let extra = effort == "high" && parts[parts.len() - 2] == "extra";
                if extra {
                    parts.pop();
                }
                variant.effort = Some(if extra { "xhigh" } else { effort }.to_string());
            }
            _ => break,
        }
        parts.pop();
    }
    variant
}

/// `name` less the account a second key names after ` · `, `[1m]`, `(asides)`, `with 1M context`, zero-width
/// spaces, and the word `free` when the model is a free tier.
fn plain_name(name: &str, free: bool) -> String {
    let model = name.split(" · ").next().unwrap_or(name);
    let mut name: String = model
        .chars()
        .filter(|c| !matches!(c, '\u{200B}'..='\u{200D}' | '\u{FEFF}'))
        .collect();
    while let Some((start, end)) = name
        .find('(')
        .and_then(|start| Some((start, start + name[start..].find(')')? + 1)))
    {
        name.replace_range(start..end, " ");
    }
    if let Some(start) = name.to_ascii_lowercase().find("with ") {
        let rest = name[start + 5..].to_ascii_lowercase();
        let (size, tail) = rest.split_once(' ').unwrap_or((&rest, ""));
        if is_context(size)
            && tail.starts_with("context")
            && !tail[7..].starts_with(|c: char| c.is_ascii_alphanumeric())
        {
            name.replace_range(start..start + 5 + size.len() + 1 + "context".len(), " ");
        }
    }
    if name.ends_with(']') {
        if let Some(start) = name.rfind('[') {
            if is_context(&name[start + 1..name.len() - 1].to_ascii_lowercase()) {
                name.truncate(start);
            }
        }
    }
    if free {
        let lower = name.trim_end().to_ascii_lowercase();
        if let Some(rest) = lower.strip_suffix("free") {
            let kept = rest.trim_end_matches([' ', ':', '_', '-']);
            if kept.len() < rest.len() {
                name.truncate(kept.len());
            }
        }
    }
    name
}

/// One catalog entry read as a model: its key across catalogs, its version, and its variant. An alias is keyed
/// by `resolved`, the versioned model it runs.
pub fn parse(
    harness: &str,
    id: &str,
    display_name: Option<&str>,
    resolved: Option<&str>,
) -> ModelName {
    let free = free(id);
    let spells = spells_variants(harness);
    let mut variant = id_variant(harness, id);
    if display_name.is_some_and(|name| name.to_ascii_lowercase().contains("(thinking)")) {
        variant.thinking = true;
    }
    let tail = |id: &str| id.rsplit('/').next().unwrap_or(id).to_string();
    let name = display_name
        .filter(|name| !name.is_empty())
        .map_or_else(|| tail(id), str::to_string);
    let reading = read(&plain_name(&name, free), spells, &mut variant);
    let resolved_reading = resolved.map(|resolved| {
        read(
            &plain_name(&tail(resolved), false),
            false,
            &mut Variant::default(),
        )
    });
    let keyed = resolved_reading.as_ref().unwrap_or(&reading);

    let version_index = keyed.keys.iter().position(|key| is_version(key));
    let version = version_index.map(|index| {
        keyed.keys[index]
            .split('.')
            .map(|part| part.parse().unwrap_or(u64::MAX))
            .collect::<Vec<u64>>()
    });
    let mut family_words: Vec<&str> = keyed
        .keys
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != version_index)
        .map(|(_, word)| word.as_str())
        .collect();
    if family_words.iter().any(|word| CLAUDE_TIERS.contains(word))
        && !family_words.contains(&"claude")
    {
        family_words.push("claude");
    }
    let joined = |words: &[&str]| {
        let mut words = words.to_vec();
        words.sort_unstable();
        words.dedup();
        words.join(" ")
    };
    let family = joined(&family_words);
    let series_words: Vec<&str> = family_words
        .iter()
        .copied()
        .filter(|word| !TIER_WORDS.contains(word) && !is_size(word))
        .collect();
    let series = Some(joined(&series_words))
        .filter(|series| !series.is_empty())
        .unwrap_or_else(|| family.clone());

    let versioned_key = match &version {
        Some(version) => {
            let version: Vec<String> = version.iter().map(u64::to_string).collect();
            format!("{family}@{}", version.join("."))
        }
        None => family.clone(),
    };
    let variant_words =
        reading.variant_words + usize::from(harness == "claude-code" && variant.context.is_some());
    ModelName {
        key: if free {
            format!("{versioned_key}:free")
        } else {
            versioned_key
        },
        family,
        series,
        version,
        label: reading.kept.join(" "),
        variant_rank: variant_words * 2 + usize::from(variant.effort.as_deref() != Some("medium")),
        variant,
    }
}

/// Adds each wire model's [`ModelName`] as `parsed`, which the picker groups by and Settings hides by.
pub fn name_models(harness: &str, models: &mut Value) {
    for model in models.as_array_mut().into_iter().flatten() {
        let parsed = parse(
            harness,
            model["id"].as_str().unwrap_or_default(),
            model["displayName"].as_str(),
            model["resolvedModel"].as_str(),
        );
        model["parsed"] = serde_json::json!(parsed);
    }
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
/// less the entries `hidden` turns off by [`ModelName::key`].
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
            let display_name = entry.get("displayName").and_then(Value::as_str);
            let resolved = entry.get("resolvedModel").and_then(Value::as_str);
            if hidden.contains(&parse(harness_id, native, display_name, resolved).key) {
                continue;
            }
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
        let free_only = models(&catalog, &["gpt@6".into()]);
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

    #[test]
    fn one_model_reads_as_one_key_across_catalogs() {
        let key = |harness, id, name, resolved| parse(harness, id, name, resolved).key;
        let haiku = [
            key(
                "opencode",
                "opencode-go/claude-haiku-5-5",
                Some("Claude Haiku 5.5"),
                None,
            ),
            key(
                "cursor",
                "claude-haiku-5-5-thinking-medium",
                Some("Claude Haiku 5.5  Medium"),
                None,
            ),
            key(
                "claude-code",
                "haiku",
                Some("Haiku"),
                Some("claude-haiku-5-5"),
            ),
            key("", "claude-haiku-5.5", None, None),
        ];
        assert!(
            haiku.iter().all(|key| key == "claude haiku@5.5"),
            "{haiku:?}"
        );
        let sonnet = [
            key(
                "cursor",
                "claude-4.6-sonnet-medium",
                Some("Claude 4.6 Sonnet"),
                None,
            ),
            key(
                "antigravity",
                "claude-sonnet-4-6",
                Some("Claude Sonnet 4.6 (Thinking)"),
                None,
            ),
        ];
        assert!(
            sonnet.iter().all(|key| key == "claude sonnet@4.6"),
            "{sonnet:?}"
        );
        let flash = parse(
            "opencode",
            "deepseek/deepseek-flash",
            Some("DeepSeek V4.1 Flash 1017"),
            None,
        );
        let harness = parse(
            "acp:dee4a440",
            r#"["deepseek-official","deepseek-flash"]"#,
            Some("DeepSeek-V4.1-Flash"),
            None,
        );
        assert_eq!(
            (flash.key.as_str(), flash.label.as_str()),
            ("deepseek flash v@4.1", "DeepSeek V4.1 Flash")
        );
        assert_eq!(harness.key, flash.key);
    }

    #[test]
    fn spelled_variants_are_one_model_whose_plain_variant_runs_by_default() {
        let cursor: Vec<ModelName> = [
            ("claude-opus-5-5-low", "Claude Opus 5.5 1M Low"),
            ("claude-opus-5-5-medium", "Claude Opus 5.5 1M"),
            ("claude-opus-5-5-medium-fast", "Claude Opus 5.5 1M Fast"),
            ("claude-opus-5-5-xhigh", "Claude Opus 5.5 1M Extra High"),
        ]
        .iter()
        .map(|(id, name)| parse("cursor", id, Some(name), None))
        .collect();
        assert!(cursor
            .iter()
            .all(|name| name.key == "claude opus@5.5" && name.label == "Claude Opus 5.5"));
        let plain = cursor.iter().min_by_key(|name| name.variant_rank).unwrap();
        assert_eq!(plain.variant.effort.as_deref(), Some("medium"));
        assert_eq!(
            cursor[3].variant,
            Variant {
                effort: Some("xhigh".into()),
                context: Some("1m".into()),
                ..Variant::default()
            }
        );
        let grok = parse(
            "cursor",
            "grok-4.7-low-fast",
            Some("Grok 4.7  Low Fast\u{200b}\u{200b}"),
            None,
        );
        assert_eq!(
            (
                grok.label.as_str(),
                grok.variant.effort.as_deref(),
                grok.variant.fast
            ),
            ("Grok 4.7", Some("low"), true)
        );

        let short = parse("claude-code", "claude-opus-5-5", Some("Opus 5.5"), None);
        let long = parse(
            "claude-code",
            "opus[1m]",
            Some("Opus 5.5 with 1M context"),
            Some("claude-opus-5-5"),
        );
        assert_eq!(
            (long.key.as_str(), long.label.as_str()),
            (short.key.as_str(), "Opus 5.5")
        );
        assert_eq!(long.variant.context.as_deref(), Some("1m"));
        assert!(long.variant_rank > short.variant_rank);
    }

    #[test]
    fn a_series_spans_the_tier_words_a_lab_reuses_across_versions() {
        let name = |id, display| parse("opencode", id, Some(display), None);
        let series = |names: &[ModelName]| {
            names
                .iter()
                .map(|name| (name.series.clone(), name.version.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            series(&[
                name("go/kimi-k3", "Kimi K3"),
                name("go/kimi-k3.1-pro", "Kimi K3.1 Pro")
            ]),
            [
                ("k kimi".into(), Some(vec![3])),
                ("k kimi".into(), Some(vec![3, 1]))
            ]
        );
        assert_eq!(
            series(&[
                name("go/qwen3.8-max", "Qwen3.8 Max"),
                name("or/qwen/qwen3.6-35b-a3b", "Qwen3.6 35B A3B")
            ]),
            [
                ("qwen".into(), Some(vec![3, 8])),
                ("qwen".into(), Some(vec![3, 6]))
            ]
        );
        assert_ne!(
            name("gpt-6-luna", "GPT-6 Luna").series,
            name("gpt-6.1-sol", "GPT-6.1-Sol").series
        );
        assert_ne!(
            name("haiku", "Haiku 4.5").series,
            name("opus", "Opus 5.5").series
        );
    }

    #[test]
    fn a_free_tier_is_its_own_model_named_without_free() {
        let name = |id, display| {
            let name = parse("opencode", id, Some(display), None);
            (name.key, name.label)
        };
        assert_eq!(
            name("opencode-go/mimo-v2.6-flash", "MiMo V2.6 Flash"),
            ("flash mimo v@2.6".into(), "MiMo V2.6 Flash".into())
        );
        assert_eq!(
            name("opencode/mimo-v2.6-flash-free", "MiMo V2.6 Flash Free"),
            ("flash mimo v@2.6:free".into(), "MiMo V2.6 Flash".into())
        );
        assert_eq!(
            name("openrouter/deepseek/deepseek-r1:free", "DeepSeek R1 (free)"),
            ("deepseek r@1:free".into(), "DeepSeek R1".into())
        );
    }

    #[test]
    fn a_second_keys_account_name_is_not_part_of_its_model() {
        let own = parse("opencode", "openai/gpt-4.1", Some("GPT 4.1"), None);
        let work = parse("opencode", "openai-2/gpt-4.1", Some("GPT 4.1 · Work"), None);
        assert_eq!(work, own);
    }

    #[test]
    fn names_outside_ascii_keep_their_letters_and_case_fold() {
        let lower = parse("opencode", "acme/modele-3", Some("Modèle 3"), None);
        let upper = parse("opencode", "acme/modele-3-upper", Some("MODÈLE 3"), None);
        assert_eq!(lower.key, upper.key);
        assert!(lower.key.contains("modèle"), "{}", lower.key);
    }

    #[test]
    fn hiding_a_picker_line_hides_every_entry_it_groups() {
        let mut catalog = json!({"harnesses": [
            {"id": "claude-code", "agentReady": true, "models": [
                {"id": "opus", "displayName": "Opus", "resolvedModel": "claude-opus-5-5"},
            ]},
            {"id": "cursor", "agentReady": true, "models": [
                {"id": "claude-opus-5-5-medium", "displayName": "Claude Opus 5.5"},
                {"id": "claude-opus-5-5-high", "displayName": "Claude Opus 5.5 High"},
            ]},
            {"id": "opencode", "agentReady": true, "models": [
                {"id": "openrouter/anthropic/claude-opus-5.5", "displayName": "Claude Opus 5.5"},
                {"id": "openrouter/openai/gpt-6.1-sol", "displayName": "GPT-6.1-Sol"},
            ]},
        ]});
        for harness in catalog["harnesses"].as_array_mut().unwrap() {
            let id = harness["id"].as_str().unwrap().to_string();
            name_models(&id, &mut harness["models"]);
        }
        let keys: Vec<&str> = catalog["harnesses"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|harness| harness["models"].as_array().unwrap())
            .map(|model| model["parsed"]["key"].as_str().unwrap())
            .collect();
        assert_eq!(
            keys[..4],
            ["claude opus@5.5"; 4],
            "the picker groups these four as one line"
        );

        let left = models(&catalog, &[keys[0].to_string()]);
        let routes: Vec<&str> = left
            .iter()
            .flat_map(|model| &model.routes)
            .map(|route| route.model.as_str())
            .collect();
        assert_eq!(routes, ["openrouter/openai/gpt-6.1-sol"]);
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
            routes(&hidden(&["deepseek flash v@4.1"])),
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
        let gone = hidden(&["deepseek flash v@4.1", "deepseek flash v@4.1:free"]);
        assert!(gone.is_empty());
        assert!(resolve(&gone, "deepseek-v4.1-flash", None)
            .unwrap_err()
            .starts_with("No route can run"));
    }
}
