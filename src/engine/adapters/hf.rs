use super::api::{self, Api};
use crate::engine::*;
use crate::error::Result;
use crate::jobs::huggingface as hf;
use crate::local::trackio;
use async_trait::async_trait;

pub struct HfAccount {
    pub namespace: String,
}
impl HfAccount {
    fn id<'a>(&self, run: &'a WatchedRun) -> Result<&'a str> {
        api::id(run, "hf_job", Some(&self.namespace), None)
    }
}

#[async_trait]
impl Api for HfAccount {
    type Cursor = u64;

    async fn launch(&self, intent: &RunIntent, fence: &Fence<'_>) -> Result<Handle, LaunchError> {
        let mut spec: hf::JobSubmission =
            serde_json::from_value(intent.launch.options.clone()).map_err(api::rejected)?;
        let token = hf::resolve_token().map_err(api::rejected)?;
        spec.labels.insert("or_run".into(), intent.run.id.clone());
        // Tokens travel as job secrets; the environment carries the run context and Trackio's
        // space, never every saved variable.
        spec.secrets.insert("HF_TOKEN".into(), token.clone());
        for key in [trackio::SERVER_URL, trackio::WRITE_TOKEN] {
            if let Some(value) = crate::config::synced_env_var(key) {
                spec.secrets.insert(key.into(), value);
            }
        }
        spec.environment
            .extend(crate::config::run_context_env(&intent.run.id));
        if let Some(space) = crate::config::synced_env_var(trackio::SPACE_ID) {
            spec.environment.insert(trackio::SPACE_ID.into(), space);
        }
        let descriptor = crate::jobs::BackendDescriptor::parse(&intent.run.backend_json)
            .map_err(api::rejected)?;
        let source = crate::compute::SourceSnapshot::from_run(&intent.run, &descriptor)
            .map_err(api::rejected)?;
        let job = hf::run_job_with_source(
            &token,
            &self.namespace,
            &spec,
            &source.path,
            &source.digest,
            Some(fence),
        )
        .await
        .map_err(api::launch_error)?;
        Ok(api::handle(intent, vec![job.id]))
    }
    async fn recover(&self, intent: &RunIntent) -> Recovery {
        let token = match hf::resolve_token() {
            Ok(token) => token,
            Err(error) => return api::unknown(error),
        };
        match hf::find_run(&token, &self.namespace, &intent.run.id).await {
            Ok(jobs) if jobs.is_empty() => Recovery::Absent,
            Ok(jobs) if jobs.len() == 1 => {
                Recovery::Found(api::handle(intent, vec![jobs[0].id.clone()]))
            }
            Ok(_) => api::unknown("Multiple HF jobs carry this run label"),
            Err(error) => api::unknown(error),
        }
    }
    async fn inspect(&self, run: &WatchedRun) -> Result<Observation> {
        let job = hf::inspect_job(&hf::resolve_token()?, &self.namespace, self.id(run)?).await?;
        let exit_code = job.status.message.as_deref()
            .and_then(|message| message.strip_prefix("Job failed with exit code: "))
            .and_then(|message| message.split_once(". Reason:"))
            .and_then(|(code, _)| code.parse().ok());
        Ok(api::stage(&job.status.stage, job.status.message, exit_code, None, None))
    }
    async fn cancel(&self, run: &WatchedRun) -> Result<()> {
        hf::cancel_job(&hf::resolve_token()?, &self.namespace, self.id(run)?).await
    }
    async fn logs(
        &self,
        run: &WatchedRun,
        cursor: &mut u64,
        window: std::time::Duration,
        sink: &mut (dyn for<'b> FnMut(&'b [u8]) + Send),
    ) -> Result<bool> {
        let token = hf::resolve_token()?;
        let mut received = 0;
        let outcome = {
            let mut lines = api::lines(sink, &mut received);
            let pass = hf::stream_logs(
                &token,
                &self.namespace,
                self.id(run)?,
                *cursor,
                None,
                &mut lines,
            );
            tokio::time::timeout(window, pass).await
        };
        let closed = api::closed(outcome, received)?;
        *cursor += received;
        Ok(closed.is_some())
    }
}
