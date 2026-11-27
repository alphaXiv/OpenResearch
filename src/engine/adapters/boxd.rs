use super::api::{self, Api};
use crate::engine::*;
use crate::error::{anyhow, Result};
use crate::jobs::boxd::{self, Placed};
use async_trait::async_trait;

pub struct BoxdAccount;

/// The handle's ids are the machine id, its region and its vCPUs.
fn placed(run: &WatchedRun) -> Result<Placed> {
    match &run.handle {
        Handle::Api { provider, ids, .. } if provider == "boxd_job" => match ids.as_slice() {
            [vm_id, region, vcpus] if !vm_id.is_empty() => Ok(Placed {
                vm_id: vm_id.clone(),
                region: region.clone(),
                vcpus: vcpus.parse().unwrap_or(0),
            }),
            _ => Err(anyhow!("boxd handle is missing its machine ID")),
        },
        _ => Err(anyhow!("Run does not belong to boxd")),
    }
}

fn handle(intent: &RunIntent, placed: Placed) -> Handle {
    api::handle(intent, vec![placed.vm_id, placed.region, placed.vcpus.to_string()])
}

fn unseen(vm_id: &str) -> String {
    format!(
        "boxd has no machine {vm_id} for the API key orx uses now, so this doesn't prove the run \
         ended. Check the machine in the boxd console, then abandon the run."
    )
}

#[async_trait]
impl Api for BoxdAccount {
    /// Bytes of the job's log already received.
    type Cursor = u64;

    async fn launch(&self, intent: &RunIntent, fence: &Fence<'_>) -> Result<Handle, LaunchError> {
        let mut spec: boxd::BoxdJobSpec =
            serde_json::from_value(intent.launch.options.clone()).map_err(api::rejected)?;
        spec.labels.insert("or_run".into(), intent.run.id.clone());
        spec.env.extend(crate::config::launch_env(&intent.run).map_err(api::rejected)?);
        for key in [boxd::API_KEY, "BOXD_TOKEN"] {
            spec.env.remove(key);
        }
        let account = boxd::Account::from_env().map_err(api::rejected)?;
        let placed = boxd::launch(&account, &spec, &|| fence.cancelled())
            .await
            .map_err(api::launch_error)?;
        Ok(handle(intent, placed))
    }

    /// The machine's size as made, so a later change to the org's default can't misprice it.
    async fn quote(&self, run: &WatchedRun, launch: &LaunchSpec) -> Result<Option<Quote>> {
        let spec: boxd::BoxdJobSpec = serde_json::from_value(launch.options.clone())?;
        let vcpus = spec.vcpus.unwrap_or(placed(run)?.vcpus);
        Ok((vcpus > 0).then(|| boxd::hourly_price(vcpus)))
    }

    async fn recover(&self, intent: &RunIntent) -> Recovery {
        let found = async {
            let spec: boxd::BoxdJobSpec = serde_json::from_value(intent.launch.options.clone())?;
            boxd::find_run(&boxd::Account::from_env()?, &spec, &intent.run.id).await
        };
        match found.await {
            Ok(mut machines) if machines.len() <= 1 => match machines.pop() {
                Some(placed) => Recovery::Found(handle(intent, placed)),
                None => Recovery::Absent,
            },
            Ok(machines) => api::unknown(format!(
                "{} boxd machines carry this run's or_run label: {}. Delete the extra ones in the \
                 boxd console, then abandon or retry the run.",
                machines.len(),
                machines.iter().map(|m| m.vm_id.as_str()).collect::<Vec<_>>().join(", ")
            )),
            Err(error) => api::unknown(error),
        }
    }

    async fn inspect(&self, run: &WatchedRun) -> Result<Observation> {
        let placed = placed(run)?;
        let Some(state) = boxd::inspect(&boxd::Account::from_env()?, &placed).await? else {
            return Ok(Observation::Unreconciled {
                detail: unseen(&placed.vm_id),
            });
        };
        Ok(api::stage(state.stage, state.message, state.exit_code, None, None))
    }

    async fn cancel(&self, run: &WatchedRun) -> Result<()> {
        let placed = placed(run)?;
        if boxd::cancel(&boxd::Account::from_env()?, &placed).await? {
            Ok(())
        } else {
            Err(anyhow!(unseen(&placed.vm_id)))
        }
    }

    async fn finish(&self, run: &WatchedRun) -> Result<()> {
        boxd::finish(&boxd::Account::from_env()?, &placed(run)?).await
    }

    async fn logs(
        &self,
        run: &WatchedRun,
        cursor: &mut u64,
        window: std::time::Duration,
        sink: &mut (dyn for<'b, 'c> FnMut(&'b [u8], Option<&'c u64>) + Send),
    ) -> Result<bool> {
        let placed = placed(run)?;
        let account = boxd::Account::from_env()?;
        let idle = crate::config::paced(api::STREAM_IDLE);
        let mut received = 0;
        let outcome = {
            let mut counted = |bytes: &[u8], offset: u64| {
                received += bytes.len() as u64;
                sink(bytes, Some(&offset));
            };
            let follow = async {
                while !boxd::stream_logs(&account, &placed, cursor, idle, &mut counted).await? {}
                Ok(())
            };
            tokio::time::timeout(window, follow).await
        };
        Ok(api::closed(outcome, received)?.is_some())
    }

    /// Log bytes are positions in the job's log file, so a spool's length is where it goes on.
    fn resume(len: u64) -> Option<u64> {
        Some(len)
    }
}
