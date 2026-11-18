use super::api::{self, Api};
use crate::engine::*;
use crate::error::{anyhow, Result};
use crate::jobs::huggingface as hf;
use crate::local::trackio;
use async_trait::async_trait;

pub struct HfAccount {
    pub namespace: String,
    whoami: tokio::sync::Mutex<std::collections::HashMap<[u8; 32], hf::WhoAmI>>,
}
impl HfAccount {
    pub fn new(namespace: String) -> Self {
        Self {
            namespace,
            whoami: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    async fn unseen(&self, token: &str, id: &str) -> Result<Option<String>> {
        use sha2::{Digest, Sha256};
        let key: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let mut cache = self.whoami.lock().await;
        if let std::collections::hash_map::Entry::Vacant(entry) = cache.entry(key) {
            let identity = tokio::time::timeout(std::time::Duration::from_secs(15), hf::who(token))
                .await
                .map_err(|_| anyhow!("Timed out checking the Hugging Face account"))??;
            entry.insert(identity);
        }
        Ok(hf::unseen(&cache[&key], &self.namespace, id))
    }
    fn id<'a>(&self, run: &'a WatchedRun) -> Result<&'a str> {
        api::id(run, "hf_job", Some(&self.namespace), None)
    }
}

#[async_trait]
impl Api for HfAccount {
    type Cursor = hf::LogResume;

    async fn launch(&self, intent: &RunIntent, fence: &Fence<'_>) -> Result<Handle, LaunchError> {
        let mut spec: hf::JobSubmission =
            serde_json::from_value(intent.launch.options.clone()).map_err(api::rejected)?;
        let token = hf::resolve_token().map_err(api::rejected)?;
        spec.labels.insert("or_run".into(), intent.run.id.clone());
        let mut saved = crate::config::saved_run_env(&intent.run).map_err(api::rejected)?;
        // The HF launcher only forwards saved Trackio settings.
        spec.secrets.insert("HF_TOKEN".into(), token.clone());
        for key in [trackio::SERVER_URL, trackio::WRITE_TOKEN] {
            if let Some(value) = saved.remove(key) {
                spec.secrets.insert(key.into(), value);
            }
        }
        spec.environment
            .extend(crate::config::run_context_env(&intent.run, &saved));
        if let Some(space) = saved.remove(trackio::SPACE_ID) {
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
    async fn quote(&self, _run: &WatchedRun, launch: &LaunchSpec) -> Result<Option<Quote>> {
        let flavor = launch.options["flavor"]
            .as_str()
            .ok_or_else(|| anyhow!("The run's launch names no Hugging Face flavor"))?;
        hf::hardware_price(&hf::endpoint(), flavor).await.map(Some)
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
        let token = hf::resolve_token()?;
        let id = self.id(run)?;
        let Some(job) = hf::inspect_job(&token, &self.namespace, id).await? else {
            return Ok(match self.unseen(&token, id).await? {
                Some(cause) => Observation::NeedsAuth { cause },
                None => api::stage("DELETED", None, None, None, None),
            });
        };
        let exit_code = job.status.message.as_deref()
            .and_then(|message| message.strip_prefix("Job failed with exit code: "))
            .and_then(|message| message.split_once(". Reason:"))
            .and_then(|(code, _)| code.parse().ok());
        Ok(api::stage(&job.status.stage, job.status.message, exit_code, None, None))
    }
    async fn cancel(&self, run: &WatchedRun) -> Result<()> {
        let token = hf::resolve_token()?;
        let id = self.id(run)?;
        if hf::cancel_job(&token, &self.namespace, id).await? {
            return Ok(());
        }
        match self.unseen(&token, id).await? {
            Some(cause) => Err(anyhow!(cause)),
            None => Ok(()),
        }
    }
    async fn logs(
        &self,
        run: &WatchedRun,
        cursor: &mut hf::LogResume,
        window: std::time::Duration,
        sink: &mut (dyn for<'b, 'c> FnMut(&'b [u8], Option<&'c hf::LogResume>) + Send),
    ) -> Result<bool> {
        let token = hf::resolve_token()?;
        let id = self.id(run)?;
        let idle = crate::config::paced(api::STREAM_IDLE);
        let mut received = 0;
        let outcome = {
            let mut counted = |bytes: &[u8], resume: &hf::LogResume| {
                received += bytes.len() as u64;
                sink(bytes, Some(resume));
            };
            let follow = async {
                while !hf::stream_logs(&token, &self.namespace, id, cursor, idle, &mut counted)
                    .await?
                {}
                Ok(())
            };
            tokio::time::timeout(window, follow).await
        };
        Ok(api::closed(outcome, received)?.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::State,
        http::{HeaderMap, StatusCode},
        routing::get,
        Json, Router,
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[tokio::test]
    async fn missing_job_identity_is_cached_per_token_and_failures_retry() {
        if !crate::config::in_test_child(&["HF_ENDPOINT"]) {
            return;
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/api/whoami-v2", get(|State(calls): State<Arc<AtomicUsize>>, headers: HeaderMap| async move {
                calls.fetch_add(1, Ordering::SeqCst);
                match headers.get("authorization").and_then(|value| value.to_str().ok()) {
                    Some("Bearer member") => (StatusCode::OK, Json(serde_json::json!({"name":"me", "orgs":[{"name":"org"}]}))),
                    Some("Bearer other") => (StatusCode::OK, Json(serde_json::json!({"name":"other", "orgs":[]}))),
                    _ => (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({}))),
                }
            }))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        std::env::set_var(
            "HF_ENDPOINT",
            format!("http://{}", listener.local_addr().unwrap()),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let account = HfAccount::new("org".into());
        std::env::set_var("HF_TOKEN", "member");
        let run = WatchedRun {
            id: "run".into(),
            handle: api::test_handle("hf_job", Some("org"), None, vec!["job".into()]),
        };
        let (first, second) = tokio::join!(account.inspect(&run), account.inspect(&run));
        assert!(matches!(first.unwrap(), Observation::Gone { .. }));
        assert!(matches!(second.unwrap(), Observation::Gone { .. }));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let message = account.unseen("other", "three").await.unwrap().unwrap();
        assert!(message.contains("three") && message.contains("other"));
        assert!(account.unseen("member", "four").await.unwrap().is_none());
        assert!(account.unseen("bad", "five").await.is_err());
        assert!(account.unseen("bad", "six").await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        server.abort();
    }

    #[tokio::test]
    async fn stalled_identity_lookup_releases_cached_tokens() {
        if !crate::config::in_test_child(&["HF_ENDPOINT"]) {
            return;
        }
        let started = Arc::new(tokio::sync::Notify::new());
        let app = Router::new()
            .route("/api/whoami-v2", get(|State(started): State<Arc<tokio::sync::Notify>>, headers: HeaderMap| async move {
                if headers.get("authorization").and_then(|value| value.to_str().ok()) == Some("Bearer stalled") {
                    started.notify_one();
                    std::future::pending::<()>().await;
                }
                Json(serde_json::json!({"name":"me", "orgs":[{"name":"org"}]}))
            }))
            .with_state(started.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        std::env::set_var("HF_ENDPOINT", format!("http://{}", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let account = Arc::new(HfAccount::new("org".into()));
        assert!(account.unseen("member", "first").await.unwrap().is_none());
        let stalled_account = account.clone();
        let stalled = tokio::spawn(async move { stalled_account.unseen("stalled", "second").await });
        started.notified().await;
        tokio::time::pause();
        let cached_account = account.clone();
        let cached = tokio::spawn(async move { cached_account.unseen("member", "third").await });
        tokio::time::advance(std::time::Duration::from_secs(15)).await;
        assert!(stalled.await.unwrap().is_err());
        assert!(cached.await.unwrap().unwrap().is_none());
        server.abort();
    }
}
