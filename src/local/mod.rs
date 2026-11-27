//! Local mode (`orx up`) — projects/experiments live in the local SQLite
//! store, experiment branches live in local Git, and runs default to this machine.
//! Nothing under this module ever calls `client.rs` / the OpenResearch api —
//! except `openresearch`, whose *compute* is a platform box by definition
//! (the run rows still live only in the local store).
//!
//! Experiment and run identifiers resolve only when their records belong to a
//! project registered in the local store.

pub mod acp;
pub mod agent_lifecycle;
pub mod agent_profiles;
pub mod agent_skills;
#[cfg(test)]
pub(crate) mod agent_text;
pub mod bash;
pub mod boxd;
pub mod browser_cookies;
pub mod chat;
pub mod claude;
pub mod codex;
pub mod datadir;
pub mod demo;
pub mod experiments;
pub mod files;
pub mod git;
pub mod github;
pub mod harness;
pub mod hf;
pub mod k8s;
pub(crate) mod keychain;
pub mod latex;
pub mod latex_templates;
pub mod local_models;
pub mod localrun;
pub mod modal;
pub mod model;
pub mod model_routes;
pub mod native_chats;
pub mod native_store;
pub mod opencode;
pub mod opencode_signin;
pub mod openresearch;
pub mod overleaf;
pub mod overleaf_live;
pub mod portable_git;
pub mod prices;
pub mod projects;
pub mod ray;
pub mod remote_cleanup;
pub mod resolve;
pub mod share;
pub mod shell_env;
pub mod skills;
pub mod slurm;
pub mod spend;
pub mod ssh;
pub mod ssh_identity;
pub mod starter;
pub mod storage;
pub mod trackio;
pub mod usage;
pub mod user_skills;
pub mod vercel;

use crate::error::{anyhow, Result};
use crate::store::{now_ms, Store, StoredRun};

/// Terminal run states — the run is finished and won't change further.
pub fn is_terminal(status: &str) -> bool {
    crate::store::is_terminal_status(status)
}

/// The stored run, but only when it belongs to a registered local experiment.
pub fn local_run(store: &Store, run_id: &str) -> Result<Option<StoredRun>> {
    match store.get_run(run_id)? {
        Some(run) => Ok(store.get_local_experiment(&run.experiment_id)?.map(|_| run)),
        None => Ok(None),
    }
}

/// Lowercased, dash-separated slug from free text (branch- and URL-safe).
pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.trim_matches('-').chars().take(48).collect();
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "experiment".to_string()
    } else {
        out
    }
}

/// Every backend a local-mode launch can target. The canonical id list —
/// launch dispatch, the default-target validation, and the Settings UI all
/// agree on these strings.
pub const BACKENDS: &[&str] = &[
    "local",
    "tinker",
    "hf",
    "modal",
    "k8s",
    "ssh",
    "slurm",
    "ray",
    "vercel",
    "boxd",
    "openresearch",
];

/// Backends whose launches take a `--flavor` (hf/modal/openresearch require
/// one; vercel's and boxd's are an optional vCPU count; slurm's is an optional GRES spec;
/// ray's is optional resource hints).
/// k8s (manifest), ssh (host), and local (this machine's hardware) have no
/// flavor axis.
pub const FLAVORED_BACKENDS: &[&str] = &[
    "hf",
    "modal",
    "slurm",
    "ray",
    "vercel",
    "boxd",
    "openresearch",
];

/// The subset whose launches FAIL without a `--flavor` — the playbook warns
/// about these when they're the default with no saved flavor.
pub const FLAVOR_REQUIRED_BACKENDS: &[&str] = &["hf", "modal", "openresearch"];

/// The `or_run` / `or_experiment` / `or_project` labels provider jobs carry.
pub fn run_labels(
    experiment_id: &str,
    run_id: &str,
) -> Result<std::collections::HashMap<String, String>> {
    let experiment = crate::store::Store::open()?
        .get_local_experiment(experiment_id)?
        .ok_or_else(|| anyhow!("Local experiment {experiment_id} not found."))?;
    Ok(std::collections::HashMap::from([
        ("or_run".to_string(), run_id.to_string()),
        ("or_experiment".to_string(), experiment.id),
        ("or_project".to_string(), experiment.project_id),
    ]))
}

/// Fill a launch's backend/flavor from the project's default compute target when
/// the caller didn't pass them. Explicit args always win; see
/// `resolve_compute_default` for the exact precedence.
pub fn apply_compute_default(
    store: &Store,
    project_id: &str,
    backend: &mut Option<String>,
    flavor: &mut Option<String>,
) -> Result<()> {
    let (b, f) = resolve_compute_default(
        backend.take(),
        flavor.take(),
        compute_default(store, project_id)?.map(|target| target.value),
    );
    *backend = b;
    *flavor = f;
    Ok(())
}

/// A project's default compute target and whether the project or the machine set it.
#[derive(Debug, Clone)]
pub struct ComputeTarget {
    pub value: (String, Option<String>),
    pub project: bool,
}

/// The default compute target for the project's runs: its own, else the machine's.
pub fn compute_default(store: &Store, project_id: &str) -> Result<Option<ComputeTarget>> {
    Ok(match store.project_settings(project_id)?.compute {
        Some(own) => Some(ComputeTarget {
            value: (own.backend, own.flavor),
            project: true,
        }),
        None => crate::config::compute_default().map(|value| ComputeTarget {
            value,
            project: false,
        }),
    })
}

/// The settings of the project an experiment belongs to; none for an experiment the store doesn't hold.
pub fn experiment_project_settings(exp_id: &str) -> Result<crate::store::ProjectSettings> {
    let store = Store::open_for_reading()?;
    match store.get_local_experiment(exp_id)? {
        Some(experiment) => store.project_settings(&experiment.project_id),
        None => Ok(Default::default()),
    }
}

/// Literature sources turned off for a chat: its own choice, else its project's, else the machine's.
pub fn disabled_lit_sources(
    store: &Store,
    chat: Option<&str>,
    project_id: Option<&str>,
) -> Result<Vec<String>> {
    if let Some(own) = chat
        .map(|chat| store.chat_lit_sources(chat))
        .transpose()?
        .flatten()
    {
        return Ok(own);
    }
    let project = match project_id {
        Some(id) => store.project_settings(id)?.disabled_lit_sources,
        None => None,
    };
    Ok(project.unwrap_or_else(crate::config::disabled_lit_sources))
}

/// [`disabled_lit_sources`] for the chat or project this command runs in.
pub fn disabled_lit_sources_here() -> Result<Vec<String>> {
    let store = Store::open_for_reading()?;
    let project = resolve::current_project(&store)?;
    let chat = crate::local::chat::launching_chat_session();
    disabled_lit_sources(
        &store,
        chat.as_deref(),
        project.as_ref().map(|project| project.id.as_str()),
    )
}

/// Pure precedence rule for the default compute target:
/// - an explicit backend always wins; the default backend fills only when none
///   was given
/// - the default flavor fills only when the *effective* backend equals the
///   default backend AND no explicit flavor was given (a default flavor for
///   modal must never leak onto an explicit `--backend hf` launch)
/// - an explicit flavor is never overwritten
fn resolve_compute_default(
    explicit_backend: Option<String>,
    explicit_flavor: Option<String>,
    default: Option<(String, Option<String>)>,
) -> (Option<String>, Option<String>) {
    let Some((default_backend, default_flavor)) = default else {
        return (explicit_backend, explicit_flavor);
    };
    let backend = explicit_backend.unwrap_or_else(|| default_backend.clone());
    let flavor = explicit_flavor.or_else(|| {
        if backend == default_backend {
            default_flavor
        } else {
            None
        }
    });
    (Some(backend), flavor)
}

/// Validate a default-target choice before persisting it (the POST handler's
/// guard). Being *configured* is deliberately not required — config state
/// fluctuates outside orx and setup order shouldn't matter — but the backend
/// must exist and the flavor must be meaningful for it.
pub fn validate_compute_default(backend: &str, flavor: Option<&str>) -> Result<()> {
    if !BACKENDS.contains(&backend) {
        return Err(anyhow!(
            "Unknown backend '{backend}'. Valid backends: {}.",
            BACKENDS.join(", ")
        ));
    }
    if flavor.is_some_and(|f| !f.is_empty()) && !FLAVORED_BACKENDS.contains(&backend) {
        return Err(anyhow!(
            "Backend '{backend}' does not take a flavor (flavors apply to {}).",
            FLAVORED_BACKENDS.join(", ")
        ));
    }
    Ok(())
}

/// Compact relative time for local tables ("3m ago").
pub fn fmt_ago(ms: i64) -> String {
    let secs = (now_ms() - ms).max(0) / 1000;
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

/// When the backend never reported a start, the launch is the only timestamp available.
pub fn run_duration_ms(run: &StoredRun, started_at: Option<i64>) -> i64 {
    let end = run.ended_at.unwrap_or_else(|| {
        if crate::engine::Phase::parse(&run.status).is_some_and(crate::engine::Phase::is_terminal) {
            run.updated_at
        } else {
            now_ms()
        }
    });
    end.saturating_sub(started_at.unwrap_or(run.created_at))
        .max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dflt(b: &str, f: Option<&str>) -> Option<(String, Option<String>)> {
        Some((b.to_string(), f.map(str::to_string)))
    }

    #[test]
    fn resolve_no_default_passes_through() {
        assert_eq!(
            resolve_compute_default(None, None, None),
            (None, None),
            "no default + no args stays empty (caller keeps its error path)"
        );
        assert_eq!(
            resolve_compute_default(Some("hf".into()), Some("t4-small".into()), None),
            (Some("hf".into()), Some("t4-small".into()))
        );
    }

    #[test]
    fn resolve_default_fills_omitted_backend_and_flavor() {
        assert_eq!(
            resolve_compute_default(None, None, dflt("modal", Some("a10g"))),
            (Some("modal".into()), Some("a10g".into()))
        );
        assert_eq!(
            resolve_compute_default(None, None, dflt("local", None)),
            (Some("local".into()), None)
        );
    }

    #[test]
    fn resolve_explicit_backend_wins_and_blocks_default_flavor() {
        // Explicit backend differs from the default: the default flavor must
        // NOT leak onto it.
        assert_eq!(
            resolve_compute_default(Some("hf".into()), None, dflt("modal", Some("a10g"))),
            (Some("hf".into()), None)
        );
        // Explicit backend HAPPENS to equal the default: flavor still fills.
        assert_eq!(
            resolve_compute_default(Some("modal".into()), None, dflt("modal", Some("a10g"))),
            (Some("modal".into()), Some("a10g".into()))
        );
    }

    #[test]
    fn resolve_explicit_flavor_never_overwritten() {
        assert_eq!(
            resolve_compute_default(None, Some("h100".into()), dflt("modal", Some("a10g"))),
            (Some("modal".into()), Some("h100".into()))
        );
    }

    #[test]
    fn a_project_default_and_sources_override_the_machine_until_cleared() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        crate::config::set_compute_default(Some("modal".into()), Some("A10G".into())).unwrap();
        crate::telemetry::set_disabled_lit_sources(vec!["biorxiv".into()]).unwrap();
        let store = Store::open().unwrap();
        let launch = |backend: Option<&str>| {
            let (mut backend, mut flavor) = (backend.map(Into::into), None);
            apply_compute_default(&store, "p", &mut backend, &mut flavor).unwrap();
            (backend, flavor)
        };
        assert_eq!(launch(None), (Some("modal".into()), Some("A10G".into())));
        assert_eq!(
            disabled_lit_sources(&store, None, Some("p")).unwrap(),
            ["biorxiv"]
        );

        let mut project = super::agent_text::sample_project();
        project.id = "p".into();
        store.create_local_project(&project).unwrap();
        store
            .update_project_settings("p", |settings| {
                settings.compute = Some(crate::store::ComputeDefault {
                    backend: "slurm".into(),
                    flavor: Some("h100:1".into()),
                });
                settings.disabled_lit_sources = Some(Vec::new());
            })
            .unwrap();
        assert_eq!(launch(None), (Some("slurm".into()), Some("h100:1".into())));
        assert_eq!(launch(Some("modal")), (Some("modal".into()), None));
        assert!(compute_default(&store, "p").unwrap().unwrap().project);
        assert!(disabled_lit_sources(&store, None, Some("p"))
            .unwrap()
            .is_empty());
        let mut chat = crate::store::tests::chat_session_fixture("chat");
        chat.project_id = "p".into();
        store.create_chat_session(&chat).unwrap();
        assert!(disabled_lit_sources(&store, Some("chat"), Some("p"))
            .unwrap()
            .is_empty());
        let pubmed = ["pubmed".to_string()];
        assert!(store.set_chat_lit_sources("chat", Some(&pubmed)).unwrap());
        assert_eq!(
            disabled_lit_sources(&store, Some("chat"), Some("p")).unwrap(),
            pubmed
        );
        assert!(!store
            .set_chat_lit_sources("missing", Some(&pubmed))
            .unwrap());
        assert_eq!(
            disabled_lit_sources(&store, None, None).unwrap(),
            ["biorxiv"]
        );

        store
            .update_project_settings("p", |settings| *settings = Default::default())
            .unwrap();
        assert_eq!(launch(None), (Some("modal".into()), Some("A10G".into())));
    }

    #[test]
    fn validate_default_backend_ids_and_flavors() {
        for b in BACKENDS {
            assert!(validate_compute_default(b, None).is_ok(), "{b} valid");
        }
        assert!(validate_compute_default("gcp", None).is_err());
        for b in FLAVORED_BACKENDS {
            assert!(
                validate_compute_default(b, Some("x")).is_ok(),
                "{b} flavored"
            );
        }
        for b in ["k8s", "ssh", "tinker", "local"] {
            assert!(
                validate_compute_default(b, Some("x")).is_err(),
                "{b} must reject a flavor"
            );
            assert!(
                validate_compute_default(b, Some("")).is_ok(),
                "{b} tolerates an empty flavor (treated as absent)"
            );
        }
    }
}
