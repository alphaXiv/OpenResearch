use super::api::{self, Api};
use crate::engine::*;
use crate::error::{anyhow, Result};
use crate::jobs::kubernetes as k8s;
use async_trait::async_trait;

pub struct K8sAccount {
    pub context: Option<String>,
    pub namespace: String,
}
impl K8sAccount {
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
            .map_err(api::interrupted)?;
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
        let state =
            k8s::inspect_job(self.context.as_deref(), &self.namespace, &self.job(run)?).await?;
        if state.stage == "DELETED" {
            let selector = format!("job-name={}", self.job(run)?);
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
    async fn logs(
        &self,
        run: &WatchedRun,
        cursor: &mut k8s::LogResume,
        window: std::time::Duration,
        sink: &mut (dyn for<'b> FnMut(&'b [u8]) + Send),
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
        let state = k8s::inspect_job(self.context.as_deref(), &self.namespace, &job).await?;
        if !crate::jobs::is_terminal_stage(&state.stage) {
            k8s::stage_source(
                self.context.as_deref(),
                &self.namespace,
                &job,
                &spec.source_archive,
            )
            .await?;
        }
        Ok(Some(Self::handle(intent, expected, &job)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adopted_runs_keep_the_recorded_primary_and_pre_resource_cancel_target() {
        let account = K8sAccount {
            context: None,
            namespace: "default".into(),
        };
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
