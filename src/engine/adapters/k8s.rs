use super::api::{self, Api};
use crate::engine::*;
use crate::error::{anyhow, Result};
use crate::jobs::kubernetes as k8s;
use async_trait::async_trait;
use futures::future::{BoxFuture, FutureExt as _, Shared};
use std::collections::HashMap;
use std::time::Duration;

/// How long one inspect waits for a source copy before reporting it still running.
const COPY_WAIT: Duration = Duration::from_secs(5);

pub struct K8sAccount {
    context: Option<String>,
    namespace: String,
    /// By run. A copy outlives the inspect that started it, so an inspect timeout never starts a
    /// second one; a finished copy stays, so a staged run is not copied again.
    copies: std::sync::Mutex<HashMap<String, Shared<BoxFuture<'static, Copied>>>>,
}

#[derive(Clone)]
enum Copied {
    Staged,
    Unstartable(String),
    Failed(String),
}

impl K8sAccount {
    pub fn new(context: Option<String>, namespace: String) -> Self {
        Self {
            context,
            namespace,
            copies: Default::default(),
        }
    }

    fn resources(&self, run: &WatchedRun) -> Result<Vec<String>> {
        let id = api::id(
            run,
            "k8s_job",
            Some(&self.namespace),
            self.context.as_deref(),
        )?;
        Ok(match &run.handle {
            Handle::Api { ids, .. } => ids.clone(),
            Handle::Legacy { descriptor } => descriptor
                .resources
                .clone()
                .unwrap_or_else(|| vec![format!("job/{id}")]),
            _ => unreachable!(),
        })
    }

    fn spec(&self, intent: &RunIntent) -> Result<k8s::ManifestSpec> {
        let mut spec: k8s::ManifestSpec = serde_json::from_value(intent.launch.options.clone())?;
        spec.labels.insert("or_run".into(), intent.run.id.clone());
        spec.env.extend(crate::config::list_synced_env());
        if let Ok(token) = crate::jobs::huggingface::resolve_token() {
            spec.env.entry("HF_TOKEN".into()).or_insert(token);
        }
        spec.pod_env = crate::config::run_context_env(&intent.run.id);
        Ok(spec)
    }
    fn handle(intent: &RunIntent, mut resources: Vec<String>, job: &str) -> Handle {
        let primary = format!("job/{job}");
        if let Some(index) = resources.iter().position(|resource| resource == &primary) {
            resources.swap(0, index);
        }
        api::handle(intent, resources)
    }
    fn job(&self, run: &WatchedRun) -> Result<String> {
        let resources = self.resources(run)?;
        if let Handle::Legacy { descriptor } = &run.handle {
            return Ok(descriptor.k8s_ref()?.1.to_owned());
        }
        resources
            .iter()
            .find_map(|resource| resource.strip_prefix("job/").map(str::to_owned))
            .ok_or_else(|| anyhow!("Kubernetes handle has no Job"))
    }
}
#[async_trait]
impl Api for K8sAccount {
    type Cursor = k8s::LogResume;

    async fn launch(&self, intent: &RunIntent, fence: &Fence<'_>) -> Result<Handle, LaunchError> {
        let spec = self.spec(intent).map_err(api::rejected)?;
        let submitted = k8s::run_manifest(self.context.as_deref(), &self.namespace, &spec, Some(fence))
            .await
            .map_err(api::launch_error)?;
        Ok(Self::handle(
            intent,
            submitted.resources,
            &submitted.job_name,
        ))
    }
    async fn recover(&self, intent: &RunIntent) -> Recovery {
        match self.resume(intent).await {
            Ok(Some(handle)) => Recovery::Found(handle),
            Ok(None) => Recovery::Absent,
            Err(error) => api::unknown(error),
        }
    }
    async fn inspect(&self, run: &WatchedRun) -> Result<Observation> {
        let job = self.job(run)?;
        let state = k8s::inspect_job(self.context.as_deref(), &self.namespace, &job).await?;
        if state.stage == "DELETED" {
            let selector = format!("job-name={job}");
            let body = k8s::kubectl(
                self.context.as_deref(),
                &[
                    "get",
                    "pods",
                    "-n",
                    &self.namespace,
                    "-l",
                    &selector,
                    "-o",
                    "json",
                ],
                None,
            )
            .await?;
            let pods: serde_json::Value = serde_json::from_str(&body)?;
            return Ok(if pods["items"].as_array().is_some_and(Vec::is_empty) {
                Observation::Gone {
                    reason:
                        "Outcome unavailable: the Job was deleted before orx recorded its outcome"
                            .into(),
                }
            } else {
                Observation::Unreconciled {
                    detail: "Deleted Job still has pods".into(),
                }
            });
        }
        let mut unstartable = state.unstartable;
        // Runs from 0.2 were staged before their handle was recorded.
        if state.stage == "RUNNING" && matches!(run.handle, Handle::Api { .. }) {
            let copy = self.copy(run, &job);
            match tokio::time::timeout(crate::config::paced(COPY_WAIT), copy).await {
                Err(_) => {
                    return Ok(Observation::Queued {
                        detail: "Copying the source".into(),
                    })
                }
                Ok(Copied::Failed(error)) => return Err(anyhow!(error)),
                Ok(Copied::Unstartable(reason)) => unstartable = Some(reason),
                Ok(Copied::Staged) => {}
            }
        }
        if let Some(reason) = unstartable {
            // A waiting pod keeps its node's resources until the Job's deadline.
            k8s::delete_resources(
                self.context.as_deref(),
                &self.namespace,
                &self.resources(run)?,
            )
            .await?;
            return Ok(Observation::Failed {
                finished_at: None,
                reason,
            });
        }
        Ok(api::stage(&state.stage, state.message, state.exit_code, None, None))
    }
    async fn cancel(&self, run: &WatchedRun) -> Result<()> {
        k8s::delete_resources(
            self.context.as_deref(),
            &self.namespace,
            &self.resources(run)?,
        )
        .await
    }
    async fn finish(&self, run: &WatchedRun) -> Result<()> {
        lock(&self.copies).remove(&run.id);
        Ok(())
    }
    async fn logs(
        &self,
        run: &WatchedRun,
        cursor: &mut k8s::LogResume,
        window: std::time::Duration,
        sink: &mut (dyn for<'b, 'c> FnMut(&'b [u8], Option<&'c k8s::LogResume>) + Send),
    ) -> Result<bool> {
        let mut resume = cursor.clone();
        let job = self.job(run)?;
        let mut received = 0;
        let outcome = {
            let mut lines = api::lines(sink, &mut received);
            let pass = k8s::stream_logs(
                self.context.as_deref(),
                &self.namespace,
                &job,
                &mut resume,
                None,
                &mut lines,
            );
            tokio::time::timeout(window, pass).await
        };
        let closed = api::closed(outcome, received)?;
        *cursor = resume;
        Ok(closed == Some(true))
    }
}
impl K8sAccount {
    async fn resume(&self, intent: &RunIntent) -> Result<Option<Handle>> {
        let spec = self.spec(intent)?;
        let (docs, job) =
            k8s::prepared_manifest(self.context.as_deref(), &self.namespace, &spec).await?;
        let expected: Vec<String> = docs.iter().map(k8s::resource_handle).collect();
        let kinds: std::collections::BTreeSet<_> = expected
            .iter()
            .filter_map(|resource| resource.split_once('/').map(|(kind, _)| kind))
            .collect();
        let kinds = kinds.into_iter().collect::<Vec<_>>().join(",");
        let selector = format!("or_run={}", intent.run.id);
        let body = k8s::kubectl(
            self.context.as_deref(),
            &[
                "get",
                &kinds,
                "-n",
                &self.namespace,
                "-l",
                &selector,
                "-o",
                "json",
            ],
            None,
        )
        .await?;
        let value: serde_json::Value = serde_json::from_str(&body)?;
        let found: Vec<String> = value["items"]
            .as_array()
            .ok_or_else(|| anyhow!("Missing Kubernetes items"))?
            .iter()
            .map(k8s::resource_handle)
            .collect();
        if found.is_empty() {
            return Ok(None);
        }
        if found.len() != expected.len()
            || !expected.iter().all(|resource| found.contains(resource))
        {
            if !found.contains(&format!("job/{job}")) {
                return Err(anyhow!("Kubernetes Job is missing while other run resources remain; it may have completed"));
            }
            k8s::delete_resources(self.context.as_deref(), &self.namespace, &found).await?;
            let remaining = k8s::kubectl(
                self.context.as_deref(),
                &[
                    "get",
                    &kinds,
                    "-n",
                    &self.namespace,
                    "-l",
                    &selector,
                    "-o",
                    "json",
                ],
                None,
            )
            .await?;
            let remaining: serde_json::Value = serde_json::from_str(&remaining)?;
            if !remaining["items"].as_array().is_some_and(Vec::is_empty) {
                return Err(anyhow!(
                    "Partial Kubernetes resources are still terminating"
                ));
            }
            return Ok(None);
        }
        Ok(Some(Self::handle(intent, expected, &job)))
    }

    /// The run's copy in flight or done, or a new one when there is none or the last failed.
    fn copy(&self, run: &WatchedRun, job: &str) -> Shared<BoxFuture<'static, Copied>> {
        let mut copies = lock(&self.copies);
        let current = copies
            .get(&run.id)
            .filter(|copy| !matches!(copy.peek(), Some(Copied::Failed(_))));
        if let Some(copy) = current {
            return copy.clone();
        }
        let (context, namespace) = (self.context.clone(), self.namespace.clone());
        let (job, id) = (job.to_owned(), run.id.clone());
        let task = tokio::spawn(async move {
            stage(context.as_deref(), &namespace, &job, &id)
                .await
                .unwrap_or_else(|error| Copied::Failed(format!("{error:#}")))
        });
        let copy = async move {
            task.await
                .unwrap_or_else(|error| Copied::Failed(format!("The source copy stopped: {error}")))
        }
        .boxed()
        .shared();
        copies.insert(run.id.clone(), copy.clone());
        copy
    }
}

async fn stage(context: Option<&str>, namespace: &str, job: &str, run: &str) -> Result<Copied> {
    let pod = k8s::leader_pod(context, namespace, job)
        .await?
        .ok_or_else(|| anyhow!("Job {namespace}/{job} has no pod to copy the source into"))?;
    let spec = launch_spec(run)?;
    // Before the copy, so the command never runs without its deadline.
    if let Err(error) = k8s::start_deadline(context, namespace, job, &pod, spec.timeout_seconds).await
    {
        let forbidden = error
            .downcast_ref::<crate::jobs::Refused>()
            .is_some_and(|refused| refused.status == Some(403));
        if forbidden {
            return Ok(Copied::Unstartable(format!(
                "{error:#}. Allow patching Jobs in namespace {namespace}, then run the experiment \
                 again."
            )));
        }
        return Err(error);
    }
    if k8s::source_staged(context, namespace, &pod).await {
        return Ok(Copied::Staged);
    }
    let archive = spec.source_archive;
    let missing = match tokio::fs::metadata(&archive).await {
        Ok(metadata) => !metadata.is_file(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => {
            return Err(anyhow!(
                "Could not read the source snapshot at {}: {error}",
                archive.display()
            ))
        }
    };
    if missing {
        return Ok(Copied::Unstartable(format!(
            "The source snapshot for this run is missing at {}, so its pod can never start. Run \
             the experiment again.",
            archive.display()
        )));
    }
    k8s::stage_source(context, namespace, &pod, &archive).await?;
    Ok(Copied::Staged)
}

fn launch_spec(run: &str) -> Result<k8s::ManifestSpec> {
    let launch = crate::store::Store::open()?
        .engine_run_state(run)?
        .and_then(|state| state.launch)
        .ok_or_else(|| anyhow!("Run {run} has no launch spec to start it from"))?;
    Ok(serde_json::from_value(launch.options)?)
}

fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adopted_runs_keep_the_recorded_primary_and_pre_resource_cancel_target() {
        let account = K8sAccount::new(None, "default".into());
        for resources in [
            serde_json::Value::Null,
            serde_json::json!(["job/worker", "job/primary"]),
        ] {
            let descriptor = crate::jobs::BackendDescriptor::parse(&serde_json::json!({
                "kind":"k8s_job", "namespace":"default", "jobId":"primary", "resources":resources
            }).to_string()).unwrap();
            let run = WatchedRun {
                id: "run".into(),
                handle: Handle::Legacy {
                    descriptor: Box::new(descriptor),
                },
            };
            assert_eq!(account.job(&run).unwrap(), "primary");
            let expected = if resources.is_null() {
                vec!["job/primary"]
            } else {
                vec!["job/worker", "job/primary"]
            };
            assert_eq!(account.resources(&run).unwrap(), expected);
        }
    }
}
