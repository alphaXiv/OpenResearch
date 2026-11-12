use super::api::{self, Api};
use crate::engine::*;
use crate::error::Result;
use crate::jobs::modal;
use async_trait::async_trait;

pub struct ModalAccount;

#[async_trait]
impl Api for ModalAccount {
    type Cursor = modal::LogResume;

    async fn launch(&self, intent: &RunIntent, _fence: &Fence<'_>) -> Result<Handle, LaunchError> {
        let mut spec: modal::ModalJobSpec =
            serde_json::from_value(intent.launch.options.clone()).map_err(api::rejected)?;
        spec.tags.insert("or_run".into(), intent.run.id.clone());
        spec.env.extend(crate::config::launch_env(&intent.run.id));
        modal::ensure_env().await.map_err(api::rejected)?;
        modal::run_job(&spec)
            .await
            .map(|sandbox| api::handle(intent, vec![sandbox]))
            .map_err(api::launch_error)
    }
    async fn recover(&self, _intent: &RunIntent) -> Recovery {
        api::unknown(
            "Modal has no lookup by run label; abandon the run after inspecting the provider",
        )
    }
    async fn inspect(&self, run: &WatchedRun) -> Result<Observation> {
        let state = modal::inspect_job(api::id(run, "modal_job", None, None)?).await?;
        Ok(api::stage(&state.stage, state.message, state.exit_code, None, None))
    }
    async fn cancel(&self, run: &WatchedRun) -> Result<()> {
        modal::cancel_job(api::id(run, "modal_job", None, None)?).await
    }
    async fn logs(
        &self,
        run: &WatchedRun,
        cursor: &mut modal::LogResume,
        window: std::time::Duration,
        sink: &mut (dyn for<'b, 'c> FnMut(&'b [u8], Option<&'c modal::LogResume>) + Send),
    ) -> Result<bool> {
        let id = api::id(run, "modal_job", None, None)?;
        let idle = crate::config::paced(api::STREAM_IDLE);
        let mut received = 0;
        let outcome = {
            let mut counted = |bytes: &[u8]| {
                received += bytes.len() as u64;
                sink(bytes, None);
            };
            let follow = async {
                while !modal::stream_logs(id, cursor, idle, &mut counted).await? {}
                Ok(())
            };
            tokio::time::timeout(window, follow).await
        };
        Ok(api::closed(outcome, received)?.is_some())
    }

    /// The launcher replays the whole log from its start, so the spool's length is the cursor.
    fn resume(len: u64) -> Option<modal::LogResume> {
        Some(modal::LogResume::from_start(len))
    }
}
