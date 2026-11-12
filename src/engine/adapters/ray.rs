use super::api::{self, Api};
use crate::engine::*;
use crate::error::{anyhow, Result};
use crate::jobs::ray;
use async_trait::async_trait;

pub struct RayAccount {
    pub address: String,
}
impl RayAccount {
    fn id<'a>(&self, run: &'a WatchedRun) -> Result<&'a str> {
        api::id(run, "ray_job", Some(&self.address), None)
    }
}

#[async_trait]
impl Api for RayAccount {
    type Cursor = usize;

    async fn launch(&self, intent: &RunIntent, _fence: &Fence<'_>) -> Result<Handle, LaunchError> {
        let mut spec: ray::JobSubmission =
            serde_json::from_value(intent.launch.options.clone()).map_err(api::rejected)?;
        spec.submission_id = intent.run.id.clone();
        spec.env.extend(crate::config::launch_env(&intent.run.id));
        ray::run_job(&self.address, &spec)
            .await
            .map_err(api::launch_error)?;
        Ok(api::handle(intent, vec![intent.run.id.clone()]))
    }
    async fn recover(&self, intent: &RunIntent) -> Recovery {
        match ray::inspect_job(&self.address, &intent.run.id).await {
            Ok(job) if job.stage == "GONE" => Recovery::Absent,
            Ok(_) => Recovery::Found(api::handle(intent, vec![intent.run.id.clone()])),
            Err(error) => api::unknown(error),
        }
    }
    async fn inspect(&self, run: &WatchedRun) -> Result<Observation> {
        let (state, started_at, finished_at) = ray::inspect_job_with_times(&self.address, self.id(run)?).await?;
        Ok(api::stage(&state.stage, state.message, state.exit_code, started_at, finished_at))
    }
    async fn cancel(&self, run: &WatchedRun) -> Result<()> {
        ray::stop_job(&self.address, self.id(run)?).await
    }

    async fn logs(
        &self,
        run: &WatchedRun,
        cursor: &mut usize,
        _window: std::time::Duration,
        sink: &mut (dyn for<'b, 'c> FnMut(&'b [u8], Option<&'c usize>) + Send),
    ) -> Result<bool> {
        let bytes = ray::fetch_logs(&self.address, self.id(run)?)
            .await?
            .into_bytes();
        if *cursor > bytes.len() {
            return Err(anyhow!("Provider log was truncated"));
        }
        sink(&bytes[*cursor..], None);
        *cursor = bytes.len();
        Ok(true)
    }
}
