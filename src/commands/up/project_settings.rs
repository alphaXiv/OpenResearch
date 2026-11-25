//! Settings a project or chat sets for itself; each one it leaves unset follows the level above.

use super::*;
use crate::local::agent_profiles::present;
use crate::store::{ComputeDefault, SlurmDefaults};

/// A change to some of a project's settings; `null` returns one to the machine's.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectSettingsEdit {
    #[serde(default, deserialize_with = "present")]
    compute: Option<Option<ComputeDefault>>,
    #[serde(default, deserialize_with = "present")]
    agent: Option<Option<StoredAgentSelectionReq>>,
    #[serde(default, deserialize_with = "present")]
    disabled_lit_sources: Option<Option<Vec<String>>>,
    /// Replaces all three; an empty one follows the machine's.
    #[serde(default)]
    slurm: Option<SlurmDefaults>,
    #[serde(default, deserialize_with = "present")]
    k8s_namespace: Option<Option<String>>,
}

pub(super) async fn project_settings(Path(id): Path<String>) -> ApiResult {
    let store = Store::open()?;
    store
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    Ok(Json(json!(store.project_settings(&id)?)))
}

pub(super) async fn update_project_settings(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(edit): Json<ProjectSettingsEdit>,
) -> ApiResult {
    let agent = match edit.agent {
        Some(Some(mut selection)) => {
            selection.model = selected_chat_model(
                &state,
                &selection.harness,
                selection.profile.as_deref(),
                selection.model.as_deref(),
            )
            .await
            .map_err(bad_request)?;
            Some(Some(
                stored_agent_selection(selection).map_err(bad_request)?,
            ))
        }
        Some(None) => Some(None),
        None => None,
    };
    let compute = edit
        .compute
        .map(|compute| compute.map(compute_default).transpose())
        .transpose()
        .map_err(bad_request)?;
    let disabled_lit_sources = edit
        .disabled_lit_sources
        .map(|sources| sources.map(lit_sources).transpose())
        .transpose()
        .map_err(bad_request)?;

    let slurm = edit
        .slurm
        .map(slurm_defaults)
        .transpose()
        .map_err(bad_request)?;
    let k8s_namespace = edit
        .k8s_namespace
        .map(|namespace| namespace.and_then(nonempty).map(k8s_namespace).transpose())
        .transpose()
        .map_err(bad_request)?;

    let store = Store::open()?;
    store
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    let saved = store.update_project_settings(&id, |settings| {
        if let Some(slurm) = slurm {
            settings.slurm = slurm;
        }
        if let Some(namespace) = k8s_namespace {
            settings.k8s_namespace = namespace;
        }
        if let Some(compute) = compute {
            settings.compute = compute;
        }
        if let Some(agent) = agent {
            settings.agent = agent;
        }
        if let Some(disabled) = disabled_lit_sources {
            settings.disabled_lit_sources = disabled;
        }
    })?;
    Ok(Json(json!(saved)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ChatLitSources {
    /// `null` returns the chat to its project's sources.
    disabled_lit_sources: Option<Vec<String>>,
}

pub(super) async fn chat_lit_sources(Path(id): Path<String>) -> ApiResult {
    let store = Store::open()?;
    store
        .get_chat_session(&id)?
        .ok_or_else(|| not_found("chat"))?;
    Ok(Json(
        json!({ "disabledLitSources": store.chat_lit_sources(&id)? }),
    ))
}

pub(super) async fn set_chat_lit_sources(
    Path(id): Path<String>,
    Json(req): Json<ChatLitSources>,
) -> ApiResult {
    let disabled = req
        .disabled_lit_sources
        .map(lit_sources)
        .transpose()
        .map_err(bad_request)?;
    if !Store::open()?.set_chat_lit_sources(&id, disabled.as_deref())? {
        return Err(not_found("chat"));
    }
    Ok(Json(json!({ "disabledLitSources": disabled })))
}

pub(super) fn compute_default(target: ComputeDefault) -> Result<ComputeDefault> {
    let backend = target.backend.trim().to_string();
    let flavor = target
        .flavor
        .map(|flavor| flavor.trim().to_string())
        .filter(|flavor| !flavor.is_empty());
    local::validate_compute_default(&backend, flavor.as_deref())?;
    Ok(ComputeDefault { backend, flavor })
}

fn nonempty(value: String) -> Option<String> {
    Some(value.trim().to_string()).filter(|value| !value.is_empty())
}

fn slurm_defaults(defaults: SlurmDefaults) -> Result<SlurmDefaults> {
    let checked = |flag: &str, value: Option<String>| -> Result<Option<String>> {
        let Some(value) = value.and_then(nonempty) else {
            return Ok(None);
        };
        crate::jobs::slurm::parse_value(flag, &value)?;
        Ok(Some(value))
    };
    Ok(SlurmDefaults {
        account: checked("--account", defaults.account)?,
        partition: checked("--partition", defaults.partition)?,
        qos: checked("--qos", defaults.qos)?,
    })
}

/// A Kubernetes namespace is a DNS label: lowercase letters, digits and `-`, at most 63.
fn k8s_namespace(namespace: String) -> Result<String> {
    let label = namespace.len() <= 63
        && namespace
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !namespace.starts_with('-')
        && !namespace.ends_with('-');
    if !label {
        return Err(anyhow!(
            "'{namespace}' isn't a Kubernetes namespace. Use lowercase letters, digits and -, up to 63 characters."
        ));
    }
    Ok(namespace)
}

pub(super) fn lit_sources(mut names: Vec<String>) -> Result<Vec<String>> {
    if let Some(unknown) = names.iter().find(|name| {
        !<crate::LitSource as clap::ValueEnum>::value_variants()
            .iter()
            .any(|source| source.as_str() == name.as_str())
    }) {
        return Err(anyhow!("Unknown literature source '{unknown}'."));
    }
    names.sort();
    names.dedup();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edit_tells_set_from_reset_from_untouched() {
        let edit: ProjectSettingsEdit =
            serde_json::from_value(json!({"compute": null, "disabledLitSources": ["pubmed"]}))
                .unwrap();
        assert!(matches!(edit.compute, Some(None)));
        assert!(edit.agent.is_none());
        assert_eq!(
            edit.disabled_lit_sources,
            Some(Some(vec!["pubmed".to_string()]))
        );
    }

    #[test]
    fn each_bad_setting_is_refused_with_its_own_reason() {
        let target = |backend: &str, flavor: Option<&str>| {
            compute_default(ComputeDefault {
                backend: backend.into(),
                flavor: flavor.map(Into::into),
            })
        };
        assert_eq!(
            target(" modal ", Some(" A10G ")).unwrap(),
            ComputeDefault {
                backend: "modal".into(),
                flavor: Some("A10G".into())
            }
        );
        assert!(target("nowhere", None)
            .unwrap_err()
            .to_string()
            .contains("Unknown backend"));
        assert!(target("local", Some("A10G"))
            .unwrap_err()
            .to_string()
            .contains("does not take a flavor"));

        assert_eq!(
            lit_sources(vec!["pubmed".into(), "biorxiv".into(), "pubmed".into()]).unwrap(),
            ["biorxiv", "pubmed"]
        );
        assert!(lit_sources(vec!["scholar".into()])
            .unwrap_err()
            .to_string()
            .contains("'scholar'"));

        assert_eq!(k8s_namespace("research-1".into()).unwrap(), "research-1");
        for bad in ["Research", "-lab", "lab_1", &"a".repeat(64)] {
            assert!(k8s_namespace(bad.into()).is_err(), "{bad}");
        }
        let slurm = slurm_defaults(SlurmDefaults {
            account: Some(" grant42 ".into()),
            partition: Some(String::new()),
            qos: None,
        })
        .unwrap();
        assert_eq!(slurm.account.as_deref(), Some("grant42"));
        assert_eq!(slurm.partition, None);
        assert!(slurm_defaults(SlurmDefaults {
            account: Some("a b".into()),
            ..Default::default()
        })
        .is_err());
    }
}
