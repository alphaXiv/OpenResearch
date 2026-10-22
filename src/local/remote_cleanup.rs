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

/// Groups due entries by host. Entries that need no cleanup leave the queue; ones orx
/// cannot safely clean leave it too and come back as (run, reason).
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
            Ok(RemoteRun::from_descriptor(&entry.run_id, &descriptor)?.zip(Some(keeps)))
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
                keep_digests: store.active_remote_source_digests(&host)?,
                host,
                runs,
            })
        })
        .collect::<Result<_>>()?;
    Ok((batches, rejected))
}

pub type Results = Vec<(String, std::result::Result<Outcome, String>)>;

pub async fn clean(batches: Vec<HostBatch>) -> Results {
    let mut results = Vec::new();
    for batch in batches {
        match cleanup::clean_host(&batch.host, &batch.runs, &batch.keep_digests).await {
            Ok(outcomes) => results.extend(outcomes.into_iter().map(|(id, o)| (id, Ok(o)))),
            Err(error) => results.extend(
                batch
                    .runs
                    .iter()
                    .map(|run| (run.run_id.clone(), Err(format!("{error:#}")))),
            ),
        }
    }
    results
}

/// Records outcomes and returns failures worth reporting; failed and busy runs retry later.
pub fn record(store: &Store, results: Results) -> Result<Failures> {
    let mut failures = Vec::new();
    for (run_id, result) in results {
        let error = match result {
            Ok(Outcome::Removed) => {
                store.finish_remote_cleanup(&run_id)?;
                continue;
            }
            Ok(Outcome::Busy) => {
                store.defer_remote_cleanup(&run_id, "its process group is still running")?;
                continue;
            }
            Ok(Outcome::Failed) => "the remote dir could not be removed".to_string(),
            Err(error) => error,
        };
        store.defer_remote_cleanup(&run_id, &error)?;
        failures.push((run_id, error));
    }
    Ok(failures)
}
