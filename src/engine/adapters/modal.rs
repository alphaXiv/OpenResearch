use super::api::{self, Api};
use crate::engine::*;
use crate::error::Result;
use crate::jobs::modal;
use async_trait::async_trait;

pub struct ModalAccount;

#[async_trait]
impl Api for ModalAccount {
    type Cursor = u64;

    async fn launch(&self, intent: &RunIntent, _fence: &Fence<'_>) -> Result<Handle, LaunchError> {
        let mut spec: modal::ModalJobSpec =
            serde_json::from_value(intent.launch.options.clone()).map_err(api::rejected)?;
        spec.tags.insert("or_run".into(), intent.run.id.clone());
        spec.env.extend(crate::config::launch_env(&intent.run.id));
        modal::ensure_env().await.map_err(api::rejected)?;
        modal::run_job(&spec)
            .await
            .map(|sandbox| api::handle(intent, vec![sandbox]))
            .map_err(api::interrupted)
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
        cursor: &mut u64,
        window: std::time::Duration,
        sink: &mut (dyn for<'b> FnMut(&'b [u8]) + Send),
    ) -> Result<bool> {
        let mut received = 0;
        let outcome = {
            let mut lines = api::lines(sink, &mut received);
            let pass = modal::stream_logs(
                api::id(run, "modal_job", None, None)?,
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
