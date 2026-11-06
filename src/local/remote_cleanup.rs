//! Background removal of queued remote run dirs; the remote half lives in `jobs::ssh::cleanup`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;

use crate::error::Result;
use crate::jobs::ssh::cleanup::{self, Outcome, RemoteRun, SourceCache};
use crate::jobs::BackendDescriptor;
use crate::store::Store;

/// Bounds one pass; the rest wait for the next.
const BATCH: usize = 50;

/// (run id, reason) pairs for the log.
pub type Failures = Vec<(String, String)>;

pub struct HostBatch {
    host: String,
    runs: Vec<RemoteRun>,
    roots: BTreeSet<SourceCache>,
}

pub struct HostResult {
    host: String,
    roots: BTreeSet<SourceCache>,
    /// (run id, deleted flag the pass acted on, outcome).
    runs: Vec<(String, bool, Outcome)>,
}

/// Groups due entries and source sweeps by host. The keep setting is read when a run comes due,
/// so it covers runs not yet cleaned up. Entries that need no cleanup leave the queue; ones orx
/// cannot safely clean leave it too and come back as failures.
pub fn plan(store: &Store) -> Result<(Vec<HostBatch>, Failures)> {
    let ssh = crate::config::ssh_settings()?;
    let slurm_keeps = crate::jobs::slurm::load_settings()?
        .unwrap_or_default()
        .keep_remote_runs;
    let mut hosts: BTreeMap<String, (Vec<RemoteRun>, BTreeSet<SourceCache>)> = BTreeMap::new();
    let mut rejected = Vec::new();
    for entry in store.due_remote_cleanups(BATCH)? {
        let parsed = RemoteRun::from_backend(&entry.run_id, &entry.backend_json, entry.deleted);
        let keeps = |run: &RemoteRun| match run.slurm {
            true => slurm_keeps,
            false => ssh
                .hosts
                .get(&run.host)
                .is_some_and(|saved| saved.keep_remote_runs),
        };
        match parsed {
            Ok(Some(run)) if entry.deleted || !keeps(&run) => {
                let (runs, roots) = hosts.entry(run.host.clone()).or_default();
                roots.insert(run.cache());
                runs.push(run);
            }
            Ok(_) => store.finish_remote_cleanup(&entry.run_id, entry.deleted)?,
            Err(error) => {
                store.finish_remote_cleanup(&entry.run_id, entry.deleted)?;
                rejected.push((entry.run_id, error.to_string()));
            }
        }
    }
    for (host, root) in store.due_source_sweeps()? {
        hosts.entry(host).or_default().1.insert((root, false));
    }
    let batches = hosts
        .into_iter()
        .map(|(host, (runs, roots))| HostBatch { host, runs, roots })
        .collect();
    Ok((batches, rejected))
}

/// Source digests that unfinished SSH/Slurm runs on `host` may still stage.
fn active_source_digests(store: &Store, host: &str) -> Result<BTreeSet<String>> {
    let mut digests = BTreeSet::new();
    for run in store.list_engine_runs()? {
        let Ok(descriptor) = BackendDescriptor::parse(&run.backend_json) else {
            continue;
        };
        let target = match crate::engine::is_engine_row(&run) {
            true => store
                .engine_run_state(&run.id)?
                .and_then(|state| state.launch)
                .filter(|launch| matches!(launch.kind.as_str(), "ssh_job" | "slurm_job"))
                .and_then(|launch| launch.target),
            false => descriptor
                .namespace
                .clone()
                .filter(|_| matches!(descriptor.kind.as_str(), "ssh_job" | "slurm_job")),
        };
        if target.as_deref() == Some(host) {
            digests.extend(descriptor.source_digest.filter(|digest| {
                digest.len() == 64 && digest.bytes().all(|c| c.is_ascii_hexdigit())
            }));
        }
    }
    Ok(digests)
}

/// Over the engine's connection to each host when `orx up` runs one.
pub async fn clean(
    batches: Vec<HostBatch>,
    moving: &AtomicBool,
    gate: &tokio::sync::Mutex<()>,
) -> Vec<HostResult> {
    let mut results = Vec::new();
    for mut batch in batches {
        let had_runs = !batch.runs.is_empty();
        let candidates = std::mem::take(&mut batch.runs);
        let host = batch.host.clone();
        let eligible = crate::commands::up::with_gated_store(moving, gate, move |store| {
            let mut runs = Vec::new();
            for run in candidates {
                if store.remote_cleanup_eligible(&run.run_id, run.deleted())? {
                    runs.push(run);
                }
            }
            Ok((runs, active_source_digests(store, &host)?))
        })
        .await;
        let (runs, keep_digests) = match eligible {
            Some(Ok(eligible)) => eligible,
            None => break,
            Some(Err(error)) => {
                eprintln!("orx up: remote cleanup eligibility: {error:#}");
                continue;
            }
        };
        if had_runs && runs.is_empty() {
            continue;
        }
        batch.runs = runs;
        let slurm = batch.runs.iter().any(|run| run.slurm);
        let (outcomes, roots) = match crate::commands::up::api_v2::host_target(&batch.host, slurm) {
            Ok(target) => (
                cleanup::clean_host(&target, &batch.runs, &batch.roots, &keep_digests).await,
                batch.roots,
            ),
            Err(error) => (Err(error), BTreeSet::new()),
        };
        let outcomes = match outcomes {
            Ok(outcomes) => outcomes,
            Err(error) => vec![Outcome::Failed(format!("{error:#}")); batch.runs.len()],
        };
        let runs = batch
            .runs
            .iter()
            .zip(outcomes)
            .map(|(run, outcome)| (run.run_id.clone(), run.deleted(), outcome))
            .collect();
        results.push(HostResult {
            host: batch.host,
            roots,
            runs,
        });
    }
    results
}

/// Records outcomes and returns failures worth reporting; failed and busy runs retry later.
pub fn record(store: &Store, results: Vec<HostResult>) -> Result<Failures> {
    let mut failures = Vec::new();
    for result in results {
        // Engine sources are swept by the next pass that cleans a run on their host.
        for (root, _) in result.roots.iter().filter(|(_, engine)| !engine) {
            // Run passes can leave tars too fresh to remove; a sweep-only pass is best effort.
            if result.runs.is_empty() {
                store.finish_source_sweep(&result.host, root.as_deref())?;
            } else {
                store.schedule_source_sweep(&result.host, root.as_deref())?;
            }
        }
        for (run_id, deleted, outcome) in result.runs {
            match outcome {
                Outcome::Removed => store.finish_remote_cleanup(&run_id, deleted)?,
                Outcome::Busy => store.defer_remote_cleanup(
                    &run_id,
                    deleted,
                    "its process group is still running",
                )?,
                Outcome::Failed(error) => {
                    store.defer_remote_cleanup(&run_id, deleted, &error)?;
                    failures.push((run_id, error));
                }
            }
        }
    }
    Ok(failures)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Contact, StoredRun};

    #[tokio::test]
    async fn queued_retention_rechecks_the_kill_before_contacting_the_host() {
        let dir = std::env::temp_dir().join(format!("orx-retention-{}", uuid::Uuid::new_v4()));
        const CHILD: &str = "ORX_TEST_RETENTION_GATE";
        if std::env::var_os(CHILD).is_none() {
            let output = crate::process::command(std::env::current_exe().unwrap())
                .args(["--exact", "local::remote_cleanup::tests::queued_retention_rechecks_the_kill_before_contacting_the_host", "--nocapture"])
                .env(CHILD, "1")
                .env("ORX_DATA_DIR", &dir)
                .output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let _ = std::fs::remove_dir_all(dir);
            return;
        }
        let dir = crate::store::data_dir();
        let store = Store::open_at(dir.clone()).unwrap();
        let backend =
            r#"{"kind":"engine","handle":{"type":"Ssh","host":"never-connect","root":"/orx"}}"#;
        let first = uuid::Uuid::from_u128(1).to_string();
        for n in 0..5 {
            let id = uuid::Uuid::from_u128(n as u128 + 1).to_string();
            store
                .upsert_run(&StoredRun {
                    id: id.clone(),
                    experiment_id: "exp".into(),
                    project_id: "project".into(),
                    status: "lost".into(),
                    backend_json: backend.into(),
                    command: "true".into(),
                    created_at: n,
                    updated_at: n,
                    ended_at: Some(n),
                    exit_code: None,
                    commit_sha: None,
                    result_markdown: None,
                    cancel_requested: false,
                    cancel_reason: None,
                    chat_session_id: None,
                })
                .unwrap();
            store.queue_finished_remote_cleanup(&id).unwrap();
        }
        assert!(store.remote_cleanup_eligible(&first, false).unwrap());
        let run = RemoteRun::from_backend(&first, backend, false)
            .unwrap()
            .unwrap();
        let batch = HostBatch {
            host: run.host.clone(),
            roots: BTreeSet::from([run.cache()]),
            runs: vec![run.clone()],
        };
        store
            .set_run_contact(
                &first,
                &Contact::Unreconciled {
                    since: 1,
                    detail: "pending stop".into(),
                },
            )
            .unwrap();
        let moving = AtomicBool::new(true);
        let gate = tokio::sync::Mutex::new(());
        assert!(clean(vec![batch], &moving, &gate).await.is_empty());
        moving.store(false, std::sync::atomic::Ordering::SeqCst);
        let batch = HostBatch {
            host: run.host.clone(),
            roots: BTreeSet::from([run.cache()]),
            runs: vec![run],
        };
        assert!(clean(vec![batch], &moving, &gate).await.is_empty());
        store.set_run_contact(&first, &Contact::Live).unwrap();
        assert!(store.remote_cleanup_eligible(&first, false).unwrap());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
