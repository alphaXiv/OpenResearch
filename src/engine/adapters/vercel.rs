use super::api::{self, Api};
use crate::engine::*;
use crate::error::{anyhow, Result};
use crate::jobs::vercel;
use async_trait::async_trait;

pub struct VercelAccount;

fn ids(run: &WatchedRun) -> Result<(&str, &str)> {
    match &run.handle {
        Handle::Api { provider, ids, .. } if provider == "vercel_job" => {
            match ids.as_slice() {
                [sandbox, process] if !sandbox.is_empty() && !process.is_empty() => {
                    Ok((sandbox, process))
                }
                _ => Err(anyhow!("Vercel handle is missing its sandbox or process ID")),
            }
        }
        Handle::Legacy { descriptor } => descriptor.vercel_ref(),
        _ => Err(anyhow!("Run does not belong to Vercel Sandbox")),
    }
}

#[async_trait]
impl Api for VercelAccount {
    type Cursor = u64;

    async fn launch(&self, intent: &RunIntent, _fence: &Fence<'_>) -> Result<Handle, LaunchError> {
        let mut spec: vercel::VercelJobSpec =
            serde_json::from_value(intent.launch.options.clone()).map_err(api::rejected)?;
        spec.name = vercel::sandbox_name(&intent.run.id);
        spec.tags.insert("or_run".into(), intent.run.id.clone());
        spec.env.extend(crate::config::launch_env(&intent.run.id));
        vercel::strip_credentials(&mut spec.env);
        vercel::ensure_env().await.map_err(api::rejected)?;
        let submitted = vercel::run_job(&spec).await.map_err(api::interrupted)?;
        Ok(api::handle(intent, vec![submitted.sandbox_name, submitted.process_id]))
    }

    async fn recover(&self, intent: &RunIntent) -> Recovery {
        api::unknown(format!(
            "Vercel did not return a process ID. Inspect sandbox {} in Vercel before abandoning this run.",
            vercel::sandbox_name(&intent.run.id)
        ))
    }

    async fn inspect(&self, run: &WatchedRun) -> Result<Observation> {
        let (sandbox, process) = ids(run)?;
        let state = vercel::inspect_job(sandbox, process).await?;
        Ok(api::stage(&state.stage, state.message, state.exit_code, None, None))
    }

    async fn cancel(&self, run: &WatchedRun) -> Result<()> {
        vercel::cancel_job(ids(run)?.0).await
    }

    async fn finish(&self, run: &WatchedRun) -> Result<()> {
        vercel::finish_job(ids(run)?.0).await
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
            let pass = vercel::stream_logs(ids(run)?.0, *cursor, window.saturating_mul(2), &mut lines);
            tokio::time::timeout(window, pass).await
        };
        let closed = api::closed(outcome, received)?;
        *cursor += received;
        Ok(closed.is_some())
    }
}
