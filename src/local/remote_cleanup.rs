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
    keep_digests: BTreeSet<String>,
}

/// Groups due entries by host. The keep setting is read when a run comes due, so it covers runs
/// not yet cleaned up. Entries that need no cleanup leave the queue; ones orx cannot safely clean leave it
/// too and come back as failures.
pub fn plan(store: &Store) -> Result<(Vec<HostBatch>, Failures)> {
    let ssh = crate::config::ssh_settings()?;
    let slurm_keeps = crate::jobs::slurm::load_settings()?
        .unwrap_or_default()
        .keep_remote_runs;
    let mut hosts: BTreeMap<String, Vec<RemoteRun>> = BTreeMap::new();
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
                hosts.entry(run.host.clone()).or_default().push(run)
            }
            Ok(_) => store.finish_remote_cleanup(&entry.run_id)?,
            Err(error) => {
                store.finish_remote_cleanup(&entry.run_id)?;
                rejected.push((entry.run_id, error.to_string()));
            }
        }
    }
    let batches = hosts
        .into_iter()
        .map(|(host, runs)| {
            Ok(HostBatch {
                keep_digests: active_source_digests(store, &host)?,
                host,
                runs,
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

pub async fn clean(batches: Vec<HostBatch>) -> Vec<(String, Outcome)> {
    let mut results = Vec::new();
    for batch in batches {
        match cleanup::clean_host(&batch.host, &batch.runs, &batch.keep_digests).await {
            Ok(outcomes) => results.extend(outcomes),
            Err(error) => results.extend(
                batch
                    .runs
                    .iter()
                    .map(|run| (run.run_id.clone(), Outcome::Failed(format!("{error:#}")))),
            ),
        }
    }
    results
}

/// Records outcomes and returns failures worth reporting; failed and busy runs retry later.
pub fn record(store: &Store, results: Vec<(String, Outcome)>) -> Result<Failures> {
    let mut failures = Vec::new();
    for (run_id, outcome) in results {
        match outcome {
            Outcome::Removed => store.finish_remote_cleanup(&run_id)?,
            Outcome::Busy => {
                store.defer_remote_cleanup(&run_id, "its process group is still running")?
            }
            Outcome::Failed(error) => {
                store.defer_remote_cleanup(&run_id, &error)?;
                failures.push((run_id, error));
            }
        }
    }
    Ok(failures)
}
