//! Background removal of queued remote run dirs; the remote half lives in `jobs::ssh::cleanup`.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::Result;
use crate::jobs::ssh::cleanup::{self, Outcome, RemoteRun};
use crate::jobs::BackendDescriptor;
use crate::store::Store;

/// Bounds one pass; the rest wait for the next.
const BATCH: usize = 50;

/// (run id, reason) pairs for the log.
pub type Failures = Vec<(String, String)>;

pub struct HostBatch {
    host: String,
    runs: Vec<RemoteRun>,
    /// Storage roots whose source cache this pass sweeps.
    roots: BTreeSet<Option<String>>,
    keep_digests: BTreeSet<String>,
}

pub struct HostResult {
    host: String,
    roots: BTreeSet<Option<String>>,
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
    let mut hosts: BTreeMap<String, (Vec<RemoteRun>, BTreeSet<Option<String>>)> = BTreeMap::new();
    let mut rejected = Vec::new();
    for entry in store.due_remote_cleanups(BATCH)? {
        let parsed = BackendDescriptor::parse(&entry.backend_json).and_then(|descriptor| {
            let keeps = match descriptor.kind.as_str() {
                "slurm_job" => slurm_keeps,
                _ => descriptor
                    .namespace
                    .as_deref()
                    .and_then(|host| ssh.hosts.get(host))
                    .is_some_and(|saved| saved.keep_remote_runs),
            };
            Ok(
                RemoteRun::from_descriptor(&entry.run_id, &descriptor, entry.deleted)?
                    .map(|run| (run, keeps)),
            )
        });
        match parsed {
            Ok(Some((run, keeps))) if entry.deleted || !keeps => {
                let (runs, roots) = hosts.entry(run.host.clone()).or_default();
                roots.insert(run.root().map(Into::into));
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
        hosts.entry(host).or_default().1.insert(root);
    }
    let batches = hosts
        .into_iter()
        .map(|(host, (runs, roots))| {
            Ok(HostBatch {
                keep_digests: active_source_digests(store, &host)?,
                host,
                runs,
                roots,
            })
        })
        .collect::<Result<_>>()?;
    Ok((batches, rejected))
}

/// Source digests that unfinished SSH/Slurm runs on `host` may still stage.
fn active_source_digests(store: &Store, host: &str) -> Result<BTreeSet<String>> {
    let mut digests = BTreeSet::new();
    for run in store.list_active_runs()? {
        let Ok(descriptor) = BackendDescriptor::parse(&run.backend_json) else {
            continue;
        };
        if matches!(descriptor.kind.as_str(), "ssh_job" | "slurm_job")
            && descriptor.namespace.as_deref() == Some(host)
        {
            digests.extend(descriptor.source_digest.filter(|digest| {
                digest.len() == 64 && digest.bytes().all(|c| c.is_ascii_hexdigit())
            }));
        }
    }
    Ok(digests)
}

pub async fn clean(batches: Vec<HostBatch>) -> Vec<HostResult> {
    let mut results = Vec::new();
    for batch in batches {
        let outcomes =
            match cleanup::clean_host(&batch.host, &batch.runs, &batch.roots, &batch.keep_digests)
                .await
            {
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
            roots: batch.roots,
            runs,
        });
    }
    results
}

/// Records outcomes and returns failures worth reporting; failed and busy runs retry later.
pub fn record(store: &Store, results: Vec<HostResult>) -> Result<Failures> {
    let mut failures = Vec::new();
    for result in results {
        for root in &result.roots {
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
