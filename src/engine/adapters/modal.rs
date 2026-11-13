use super::api::{self, Api};
use crate::engine::*;
use crate::error::{anyhow, Result};
use crate::jobs::modal;
use async_trait::async_trait;

pub struct ModalAccount;

fn unseen(sandbox: &str) -> String {
    format!(
        "Modal can't find sandbox {sandbox} with the token and environment orx uses now. orx \
         doesn't record which workspace and environment launched it, so this doesn't prove the \
         sandbox was deleted. Check it in the Modal dashboard."
    )
}

#[async_trait]
impl Api for ModalAccount {
    type Cursor = modal::LogResume;

    async fn launch(&self, intent: &RunIntent, fence: &Fence<'_>) -> Result<Handle, LaunchError> {
        let mut spec: modal::ModalJobSpec =
            serde_json::from_value(intent.launch.options.clone()).map_err(api::rejected)?;
        spec.tags.insert("or_run".into(), intent.run.id.clone());
        spec.env.extend(crate::config::launch_env(&intent.run.id));
        modal::ensure_env().await.map_err(api::rejected)?;
        modal::run_job(&spec, Some(fence))
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
        let id = api::id(run, "modal_job", None, None)?;
        let Some(state) = modal::inspect_job(id).await? else {
            return Ok(Observation::Unreconciled { detail: unseen(id) });
        };
        Ok(api::stage(&state.stage, state.message, state.exit_code, None, None))
    }
    async fn cancel(&self, run: &WatchedRun) -> Result<()> {
        let id = api::id(run, "modal_job", None, None)?;
        if modal::cancel_job(id).await? {
            Ok(())
        } else {
            Err(anyhow!(unseen(id)))
        }
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
