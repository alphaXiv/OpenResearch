use serde_json::Value;

use super::{HarnessInfo, ModelInfo};
use crate::error::Result;

#[derive(Debug)]
pub(crate) struct ModelSelectionError(String);

impl std::fmt::Display for ModelSelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ModelSelectionError {}

pub(crate) fn concrete_model(model: &str) -> bool {
    !model.trim().is_empty() && !matches!(model, "default" | "opusplan")
}

pub(crate) fn resolve_models(
    harness: &str,
    models: &mut Vec<ModelInfo>,
    configured: Option<&str>,
) -> Option<String> {
    models.retain(|model| concrete_model(&model.id));
    if let Some(configured) = configured.filter(|model| concrete_model(model)) {
        if models.iter().any(|model| model.id == configured) {
            return Some(configured.to_owned());
        }
        let mut matches = models.iter().filter(|model| {
            model.resolved_model.as_deref() == Some(configured)
                || model.display_name.as_deref() == Some(configured)
        });
        if let Some(model) = matches.next() {
            if matches.next().is_none() {
                return Some(model.id.clone());
            }
        }
        models.push(ModelInfo::new(configured));
        return Some(configured.to_owned());
    }
    let ranked: Vec<_> = models
        .iter()
        .map(|model| {
            let name = crate::local::model_routes::parse(
                harness,
                &model.id,
                model.display_name.as_deref(),
                model.resolved_model.as_deref(),
            );
            let words: Vec<&str> = name.family.split(' ').collect();
            let family = if words.contains(&"opus") {
                0
            } else if words.contains(&"sol") && words.contains(&"gpt") {
                1
            } else {
                2
            };
            let mut version = name.version.clone().unwrap_or_default();
            while version.last() == Some(&0) {
                version.pop();
            }
            (model, family, version, name)
        })
        .collect();
    let preferred = match harness {
        "codex" => 1,
        "claude-code" => 0,
        _ => ranked.iter().map(|row| row.1).min()?,
    };
    let has_preferred = ranked.iter().any(|row| row.1 == preferred);
    ranked
        .into_iter()
        .filter(|row| !has_preferred || row.1 == preferred)
        .max_by(|a, b| {
            let plain = |name: &crate::local::model_routes::ModelName| {
                (name.variant_rank, name.family != name.series)
            };
            a.2.cmp(&b.2)
                .then_with(|| plain(&b.3).cmp(&plain(&a.3)))
                .then_with(|| b.3.label.len().cmp(&a.3.label.len()))
                .then_with(|| b.3.label.cmp(&a.3.label))
                .then_with(|| b.0.id.cmp(&a.0.id))
        })
        .map(|row| row.0.id.clone())
}

pub(crate) fn resolve_model(info: &mut HarnessInfo, configured: Option<&str>) {
    info.default_model = resolve_models(&info.id, &mut info.models, configured)
        .filter(|_| info.agent_ready && !info.catalog_pending);
}

pub(crate) fn resolve_entry(entry: &mut Value, configured: Option<&str>) -> Result<()> {
    let mut models: Vec<ModelInfo> = match entry.get("models") {
        Some(models) => serde_json::from_value(models.clone())?,
        None => Vec::new(),
    };
    let model = resolve_models(entry["id"].as_str().unwrap_or(""), &mut models, configured);
    entry["models"] = serde_json::to_value(models)?;
    entry
        .as_object_mut()
        .expect("harness entry")
        .remove("defaultModel");
    if entry["agentReady"] == true && entry["catalogPending"] != true {
        if let Some(model) = model {
            entry["defaultModel"] = Value::String(model);
        }
    }
    Ok(())
}

pub(crate) fn catalog_entry<'a>(
    payload: &'a Value,
    harness: &str,
    profile: Option<&str>,
) -> Option<&'a Value> {
    payload["harnesses"].as_array()?.iter().find(|entry| {
        entry["id"].as_str() == Some(harness) && entry["profile"]["id"].as_str() == profile
    })
}

pub(crate) fn catalog_default(
    payload: &Value,
    harness: &str,
    profile: Option<&str>,
) -> Option<String> {
    let entry = catalog_entry(payload, harness, profile)?;
    (entry["agentReady"] == true && entry["catalogPending"] != true)
        .then(|| {
            entry["defaultModel"]
                .as_str()
                .filter(|m| concrete_model(m))
                .map(str::to_owned)
        })
        .flatten()
}

pub(crate) fn selected_model(
    payload: &Value,
    harness: &str,
    profile: Option<&str>,
    model: Option<&str>,
) -> Result<Option<String>> {
    let entry = catalog_entry(payload, harness, profile)
        .ok_or_else(|| ModelSelectionError("Re-check this agent in Settings → Agents.".into()))?;
    let name = entry["name"].as_str().unwrap_or(harness);
    if entry["catalogPending"] == true {
        return Err(ModelSelectionError(format!("Wait for {name} to finish checking.")).into());
    }
    if entry["authState"] == "needsLogin" {
        return Err(ModelSelectionError(format!("Sign in to {name} first.")).into());
    }
    if entry["agentReady"] != true {
        let message = entry["agentNote"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Re-check {name} in Settings → Agents."));
        return Err(ModelSelectionError(message).into());
    }
    Ok(model
        .filter(|m| concrete_model(m))
        .map(str::to_owned)
        .or_else(|| catalog_default(payload, harness, profile)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn configured_models_missing_from_catalogs_remain_listed_and_selected() {
        for (harness, configured) in [
            ("codex", "deepseek-chat"),
            ("claude-code", "bedrock-custom"),
            ("acp:test", "42"),
        ] {
            let mut info = HarnessInfo::new(harness, harness);
            info.agent_ready = true;
            info.models = vec![ModelInfo::new("gpt-6.1-sol")];
            resolve_model(&mut info, Some(configured));
            assert_eq!(info.default_model.as_deref(), Some(configured));
            assert!(info.models.iter().any(|model| model.id == configured));
        }
    }

    #[test]
    fn acp_current_model_without_a_catalog_is_a_concrete_choice() {
        for configuration in [
            json!({"models":{"currentModelId":"opaque"}}),
            json!({"configOptions":[{"category":"model","currentValue":"opaque"}]}),
        ] {
            let mut info = HarnessInfo::new("acp:test", "Test");
            info.agent_ready = true;
            info.models = super::super::acp::advertised_models(&configuration);
            resolve_model(&mut info, super::super::acp::current_model(&configuration));
            assert_eq!(info.default_model.as_deref(), Some("opaque"));
            assert_eq!(info.models.len(), 1);
            assert_eq!(info.models[0].id, "opaque");
        }
        let payload = json!({"harnesses":[{"id":"acp:test","agentReady":true,"models":[]}]});
        assert_eq!(
            selected_model(&payload, "acp:test", None, Some("native-current"))
                .unwrap()
                .as_deref(),
            Some("native-current")
        );
        assert_eq!(
            selected_model(&payload, "acp:test", None, None).unwrap(),
            None
        );
    }

    #[test]
    fn fallback_prefers_newest_family_then_base_variant_independent_of_order() {
        let mut models = vec![
            ModelInfo::new("claude-opus-5-5-low").with_label(Some("Claude Opus 5.5 1M Low"), None),
            ModelInfo::new("claude-opus-5-5-medium").with_label(Some("Claude Opus 5.5 1M"), None),
            ModelInfo::new("claude-opus-5-5-high")
                .with_label(Some("Claude Opus 5.5 1M High"), None),
            ModelInfo::new("gpt-6.1-sol-pro"),
            ModelInfo::new("gpt-6.1-sol"),
            ModelInfo::new("gpt-5.9-sol"),
            ModelInfo::new("gpt-5.10-sol"),
        ];
        for _ in 0..models.len() {
            assert_eq!(
                resolve_models("cursor", &mut models, None).as_deref(),
                Some("claude-opus-5-5-medium")
            );
            assert_eq!(
                resolve_models("codex", &mut models, None).as_deref(),
                Some("gpt-6.1-sol")
            );
            models.rotate_left(1);
        }
        let mut models = vec![
            ModelInfo::new("gpt-5.9-sol"),
            ModelInfo::new("gpt-5.10-sol"),
        ];
        assert_eq!(
            resolve_models("codex", &mut models, None).as_deref(),
            Some("gpt-5.10-sol")
        );
    }

    #[test]
    fn aliases_map_only_when_unambiguous_and_wire_sentinels_inherit() {
        let mut models = vec![ModelInfo::new("opus[1m]")];
        models[0].resolved_model = Some("claude-opus-5-5".into());
        assert_eq!(
            resolve_models("claude-code", &mut models, Some("claude-opus-5-5")).as_deref(),
            Some("opus[1m]")
        );
        let payload =
            json!({"harnesses":[{"id":"claude-code","agentReady":true,"defaultModel":"opus[1m]"}]});
        for model in [None, Some(""), Some("default"), Some("opusplan")] {
            assert_eq!(
                selected_model(&payload, "claude-code", None, model)
                    .unwrap()
                    .as_deref(),
                Some("opus[1m]")
            );
        }
    }
}
