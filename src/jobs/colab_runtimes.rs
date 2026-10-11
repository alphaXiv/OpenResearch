//! Colab runtimes assigned to the signed-in account, and releasing them.
//!
//! A run's controller releases its runtime on every exit it can trap. A
//! controller killed outright (SIGKILL, a crash, the machine shutting down)
//! cannot, and the runtime keeps spending compute units until Colab's own idle
//! timeout. [`reap_orphans`] is the safety net: it releases runtimes OpenResearch
//! started for runs that have already ended. Runtimes it did not start (a
//! notebook open in the browser) are only listed, never stopped automatically.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::Value;

use super::colab::{session_name, token_path};
use super::colab_account::{
    accelerator_id, access_token, get_json, COLAB_DOMAIN, REQUEST_TIMEOUT, XSSI_PREFIX,
};

/// How often `orx up` looks for runtimes left behind by ended runs.
pub const REAP_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Runtime {
    pub endpoint: String,
    /// orx accelerator id (`t4`, `cpu`, …), when Colab named a known one.
    pub accelerator: Option<String>,
    pub high_mem: bool,
    /// The Colab CLI session name, when the CLI on this machine started it.
    pub name: Option<String>,
    /// The OpenResearch run that owns it, when it is one of ours.
    pub run_id: Option<String>,
    /// Status of that run (`running`, `done`, …).
    pub run_status: Option<String>,
    /// Started by OpenResearch for a run that has already ended.
    pub orphaned: bool,
}

/// Every runtime assigned to the account, labelled with its owner when known.
pub async fn runtimes() -> Result<Vec<Runtime>, String> {
    if !token_path().is_some_and(|path| path.is_file()) {
        return Ok(Vec::new());
    }
    let token = access_token().await?;
    let client = client()?;
    let body = get_json(
        &client,
        &token,
        &format!("{COLAB_DOMAIN}/tun/m/assignments?authuser=0"),
    )
    .await?;
    let names = cli_session_names();
    let runs = colab_runs();
    Ok(parse_assignments(&body, &names, &runs))
}

/// Release one runtime. Colab hands out a one-time token for the unassign.
pub async fn stop(endpoint: &str) -> Result<(), String> {
    if endpoint.is_empty()
        || !endpoint
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err("Invalid Colab runtime.".into());
    }
    let token = access_token().await?;
    let client = client()?;
    let url = format!("{COLAB_DOMAIN}/tun/m/unassign/{endpoint}?authuser=0");
    let ticket = get_json(&client, &token, &url).await?;
    let xsrf = ticket["token"]
        .as_str()
        .ok_or("Colab did not offer to release this runtime.")?;
    let response = client
        .post(&url)
        .bearer_auth(&token)
        .header("Accept", "application/json")
        .header("X-Colab-Client-Agent", "colab-cli")
        .header("X-Goog-Colab-Token", xsrf)
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Colab refused to release the runtime (HTTP {}).",
            response.status()
        ));
    }
    super::colab_account::invalidate().await;
    Ok(())
}

/// Release runtimes OpenResearch started for runs that have ended. Returns the
/// released endpoints.
pub async fn reap_orphans() -> Vec<String> {
    let Ok(runtimes) = runtimes().await else {
        return Vec::new();
    };
    let mut released = Vec::new();
    for runtime in runtimes.into_iter().filter(|r| r.orphaned) {
        match stop(&runtime.endpoint).await {
            Ok(()) => {
                eprintln!(
                    "orx: released Colab runtime {} left behind by run {}",
                    runtime.name.as_deref().unwrap_or(&runtime.endpoint),
                    runtime.run_id.as_deref().unwrap_or("?")
                );
                released.push(runtime.endpoint);
            }
            Err(error) => eprintln!(
                "orx: could not release Colab runtime {}: {error}",
                runtime.endpoint
            ),
        }
    }
    released
}

fn client() -> Result<reqwest::Client, String> {
    crate::net::remote_client()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())
}

/// Endpoint → name, from the Colab CLI's own session list.
fn cli_session_names() -> HashMap<String, String> {
    let Some(path) = dirs::home_dir().map(|home| home.join(".config/colab-cli/sessions.json"))
    else {
        return HashMap::new();
    };
    let Some(value) = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    else {
        return HashMap::new();
    };
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, session)| {
            Some((session["endpoint"].as_str()?.to_string(), name.clone()))
        })
        .collect()
}

/// Session name → (run id, status) for this store's Colab runs.
fn colab_runs() -> HashMap<String, (String, String)> {
    let Ok(runs) = crate::store::Store::open().and_then(|store| store.list_runs(1000)) else {
        return HashMap::new();
    };
    runs.into_iter()
        .filter(|run| {
            serde_json::from_str::<Value>(&run.backend_json)
                .is_ok_and(|backend| backend["kind"] == "colab_job")
        })
        .map(|run| (session_name(&run.id), (run.id, run.status)))
        .collect()
}

fn parse_assignments(
    body: &Value,
    names: &HashMap<String, String>,
    runs: &HashMap<String, (String, String)>,
) -> Vec<Runtime> {
    let body = match body.as_str() {
        Some(text) => serde_json::from_str(text.strip_prefix(XSSI_PREFIX).unwrap_or(text))
            .unwrap_or(Value::Null),
        None => body.clone(),
    };
    body["assignments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|assignment| {
            let endpoint = assignment["endpoint"].as_str()?.to_string();
            let name = names.get(&endpoint).cloned();
            let run = name.as_ref().and_then(|name| runs.get(name));
            let orphaned = run.is_some_and(|(_, status)| crate::store::is_terminal_status(status));
            Some(Runtime {
                accelerator: assignment["accelerator"].as_str().and_then(accelerator_id),
                high_mem: matches!(assignment["machineShape"].as_i64(), Some(1))
                    || assignment["machineShape"] == "HIGH_RAM",
                run_id: run.map(|(id, _)| id.clone()),
                run_status: run.map(|(_, status)| status.clone()),
                orphaned,
                name,
                endpoint,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_runtimes_of_ended_runs_are_orphaned() {
        let body = json!({"assignments": [
            {"endpoint": "e-done", "accelerator": "T4", "machineShape": 0},
            {"endpoint": "e-live", "accelerator": "A100", "machineShape": 1},
            {"endpoint": "e-browser", "accelerator": "L4", "machineShape": 0},
            {"endpoint": "e-other-cli", "accelerator": "NONE", "machineShape": 0}
        ]});
        let names = HashMap::from([
            ("e-done".to_string(), "orx-done".to_string()),
            ("e-live".to_string(), "orx-live".to_string()),
            ("e-other-cli".to_string(), "my-notebook".to_string()),
        ]);
        let runs = HashMap::from([
            (
                "orx-done".to_string(),
                ("run-done".to_string(), "done".to_string()),
            ),
            (
                "orx-live".to_string(),
                ("run-live".to_string(), "running".to_string()),
            ),
        ]);
        let runtimes = parse_assignments(&body, &names, &runs);
        let orphaned: Vec<_> = runtimes
            .iter()
            .filter(|r| r.orphaned)
            .map(|r| r.endpoint.as_str())
            .collect();
        assert_eq!(orphaned, ["e-done"]);
        let live = runtimes.iter().find(|r| r.endpoint == "e-live").unwrap();
        assert!(live.high_mem && live.run_id.as_deref() == Some("run-live"));
        assert_eq!(
            runtimes
                .iter()
                .find(|r| r.endpoint == "e-other-cli")
                .unwrap()
                .accelerator
                .as_deref(),
            Some("cpu")
        );
        assert!(runtimes
            .iter()
            .find(|r| r.endpoint == "e-browser")
            .unwrap()
            .name
            .is_none());
    }
}
