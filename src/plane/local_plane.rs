//! Local implementations of project, experiment, run, and log commands.

use std::time::{Duration, Instant};

use super::{CreateExperimentSpec, DescInput, ProjectEdit, Run, RunListing};
use crate::error::{anyhow, Result};
use crate::local::model::{LocalExperiment, LocalProject};
use crate::store::Store;
use crate::ExpRunArgs;

/// The local-store plane. `project`/`experiment` carry the row the resolver
/// already fetched (present for project-/experiment-keyed commands); `id` is the
/// resolved project/experiment/run id for the run-keyed and re-lookup paths.
pub struct LocalPlane {
    pub(super) store: Store,
    pub(super) project: Option<LocalProject>,
    pub(super) experiment: Option<LocalExperiment>,
    pub(super) id: String,
}

impl LocalPlane {
    /// The resolved project row, or an error when called on a plane built by a
    /// different keyed resolver.
    fn project(&self) -> Result<&LocalProject> {
        self.project
            .as_ref()
            .ok_or_else(|| anyhow!("internal: local plane missing its project row"))
    }

    /// The resolved experiment row, analogous to `project`.
    fn experiment(&self) -> Result<&LocalExperiment> {
        self.experiment
            .as_ref()
            .ok_or_else(|| anyhow!("internal: local plane missing its experiment row"))
    }

    /// Demo analytics label for a launch, keyed on the experiment's owning project.
    pub(super) fn demo_run_label(&self) -> Option<&'static str> {
        self.experiment
            .as_ref()
            .and_then(crate::local::demo::run_label)
    }

    pub async fn list_runs(&self) -> Result<RunListing> {
        let value = ApiClient::discover()
            .await?
            .json(
                reqwest::Method::GET,
                &format!("runs?project={}", urlencoding::encode(&self.id)),
                None,
            )
            .await?;
        Ok(serde_json::from_value(value)?)
    }

    pub async fn view_project(&self, all: bool) -> Result<()> {
        let store = &self.store;
        let project = self.project()?;
        println!("{} (local)", project.name);
        println!("  id:      {}", project.id);
        println!("  repo:    {}", project.repo_path);
        println!("  branch:  {} (baseline)", project.baseline_branch);
        match project
            .run_command
            .as_deref()
            .filter(|c| !c.trim().is_empty())
        {
            Some(cmd) => println!("  command: {}", cmd),
            None => println!(
                "  command: — (not set — `orx project edit {} --run-command '<cmd>'`)",
                project.id
            ),
        }

        let experiments: Vec<_> = store
            .list_experiments_by_project(&project.id)?
            .into_iter()
            .filter(|e| all || !e.archived)
            .collect();
        println!("\nExperiments");
        if experiments.is_empty() {
            println!("  (none)");
        } else {
            for e in &experiments {
                let root = if e.parent_experiment_id.is_none() {
                    " [root]"
                } else {
                    ""
                };
                println!(
                    "  {}  {}{}{}  ({})",
                    e.id,
                    e.display_name(),
                    root,
                    if e.archived { " [archived]" } else { "" },
                    e.branch_name
                );
            }
        }
        Ok(())
    }

    pub async fn edit_project(&self, edit: ProjectEdit) -> Result<()> {
        let mut project = self.project()?.clone();
        let name = edit.name;
        let run_command = edit.run_command;
        if name.is_none() && run_command.is_none() {
            return Err(anyhow!(
                "Nothing to change. Pass at least one of --name or --run-command."
            ));
        }
        if let Some(name) = name {
            if name.trim().is_empty() {
                return Err(anyhow!("--name cannot be empty."));
            }
            project.name = name.trim().to_string();
        }
        if let Some(cmd) = run_command {
            project.run_command = Some(cmd).filter(|c| !c.trim().is_empty());
        }
        self.store.update_local_project(&project)?;

        println!("\u{2713} Project updated.");
        println!("  id:      {}", project.id);
        println!("  name:    {}", project.name);
        match project.run_command.as_deref() {
            Some(cmd) => println!("  command: {}", cmd),
            None => println!("  command: — (empty)"),
        }
        Ok(())
    }

    // --- experiment -------------------------------------------------------

    pub async fn experiment_status(&self, scheduler: bool) -> Result<()> {
        let store = &self.store;
        let exp = self.experiment()?;
        println!("{}  ({})  [local]", exp.display_name(), exp.agent_status);
        println!("  id:       {}", exp.id);
        println!("  branch:   {}", exp.branch_name);
        if exp.archived {
            println!("  archived: yes");
        }
        match &exp.parent_experiment_id {
            Some(parent_id) => match store.get_local_experiment(parent_id)? {
                Some(parent) => {
                    println!("  parent:   {} (branch {})", parent_id, parent.branch_name)
                }
                None => println!("  parent:   {}", parent_id),
            },
            None => println!("  parent:   — (root experiment)"),
        }
        let project = store.get_local_project(&exp.project_id)?;
        let project_command = project.as_ref().and_then(|p| p.run_command.as_deref());
        match crate::local::experiments::effective_run_command(exp, project_command) {
            Some(cmd) if !exp.run_command.trim().is_empty() => println!("  command:  {cmd}"),
            Some(cmd) => println!("  command:  {cmd} (project default)"),
            None => println!(
                "  command:  — (not set — `orx project edit {} --run-command '<cmd>'`)",
                exp.project_id
            ),
        }

        match store.latest_run_for_experiment(&exp.id)? {
            Some(r) => {
                let state = store.engine_run_state(&r.id)?;
                let run = Run::from_store(store, &r)?;
                let commit = run
                    .commit_sha
                    .as_deref()
                    .map(|s| s.chars().take(7).collect::<String>())
                    .unwrap_or_else(|| "—".to_string());
                println!(
                    "  last run: {} ({}, commit {}, ran {}, updated {})",
                    run.id,
                    run.display_status(),
                    commit,
                    crate::output::format_duration(run.duration_secs),
                    run.updated_display
                );
                for detail in [run.waiting_detail(), run.contact_detail()]
                    .into_iter()
                    .flatten()
                {
                    println!("  {detail}");
                }
                if let Some(next) = run.next_step() {
                    println!("  {next}");
                }
                let launch = state.and_then(|state| state.launch);
                let backend = crate::store::public_backend(&r.backend_json, launch.as_ref())
                    .map(|backend| backend.to_string())
                    .unwrap_or_default();
                if let Ok(backend) = crate::jobs::BackendDescriptor::parse(&backend) {
                    if backend.kind == "slurm_job" {
                        println!(
                            "  requested time limit: {}",
                            backend
                                .timeout_secs
                                .map(|s| format!("{s}s"))
                                .unwrap_or_else(|| "unknown (older run or cluster default)".into())
                        );
                        if backend.job_id.is_some() {
                            println!("  {}", crate::local::slurm::status_resources(&backend));
                        }
                        if let Some(error) = backend.monitoring_error.as_deref() {
                            println!("  {error}");
                        }
                        if scheduler {
                            match backend.slurm_ref() {
                                Ok((host, job_id)) => match tokio::time::timeout(
                                    std::time::Duration::from_secs(15),
                                    crate::jobs::slurm::diagnostics(host, job_id),
                                )
                                .await
                                {
                                    Ok(Ok(details)) => println!("  scheduler: {details}"),
                                    Ok(Err(error)) => println!("  scheduler: unknown ({error})"),
                                    Err(_) => println!("  scheduler: unknown (query timed out)"),
                                },
                                Err(_) => println!("  scheduler: unknown (job not submitted)"),
                            }
                        }
                    }
                }
                if let Some(detail) = run.failure_detail() {
                    println!("  {detail}");
                }
                if let Some(sha) = &run.commit_sha {
                    println!("  commit:   {}", sha);
                }
            }
            None => println!("  last run: — (never run)"),
        }
        // A forced relaunch can leave an older run live; monitoring alerts send agents here.
        for older in store.list_runs_by_experiment(&exp.id)?.iter().skip(1) {
            let run = Run::from_store(store, older)?;
            if matches!(run.contact, crate::store::Contact::Stopping { .. })
                || run.cancel_unconfirmed.is_some()
            {
                println!("  run {}: {}", run.id, run.display_status());
            } else {
                for detail in [run.waiting_detail(), run.contact_detail(), run.next_step()]
                    .into_iter()
                    .flatten()
                {
                    println!("  run {}: {detail}", run.id);
                }
            }
            if !matches!(older.status.as_str(), "starting" | "running") {
                continue;
            }
            let error = crate::jobs::BackendDescriptor::parse(&older.backend_json)
                .ok()
                .and_then(|backend| backend.monitoring_error);
            if let Some(error) = error {
                println!("  older live run {}: {error}", older.id);
            }
        }
        Ok(())
    }

    pub async fn experiment_desc(&self, set: Option<String>, stdin: bool) -> Result<()> {
        let input = DescInput::resolve(set, stdin).await?;
        let mut exp = self.experiment()?.clone();
        match input {
            DescInput::Set(description) => {
                exp.description = Some(description);
                self.store.update_local_experiment(&exp)?;
                println!("\u{2713} Description saved.");
            }
            DescInput::Get => match exp.description.as_deref().filter(|d| !d.trim().is_empty()) {
                Some(d) => println!("{d}"),
                None => eprintln!(
                    "No description set. Add one with `orx exp desc {} --set \"…\"` \
                     or pipe a file: `cat notes.md | orx exp desc {} --stdin`.",
                    exp.id, exp.id
                ),
            },
        }
        Ok(())
    }

    pub async fn launch(&self, mut args: ExpRunArgs) -> Result<()> {
        // Resolved before validation so e.g. `--host box1` with a default of `ssh` is valid,
        // and before telemetry records the backend.
        crate::local::apply_compute_default(&mut args.backend, &mut args.flavor);
        let backend = args.backend.get_or_insert_with(|| "local".into()).clone();
        crate::compute::validate_run_args(&args)?;
        warn_unreachable_trackio(&backend);
        let experiment = self.experiment()?;
        if let Some(warning) = self
            .store
            .get_local_project(&experiment.project_id)?
            .and_then(|project| {
                crate::local::experiments::legacy_root_warning(&project, experiment)
            })
        {
            eprintln!("{warning}");
        }
        ApiClient::discover().await?.launch(args).await?;
        // Key event, fired only on a successful launch. Coarse backend only.
        if backend != "openresearch" {
            crate::telemetry::capture_experiment_started("run", true, Some(&backend));
            // A launch out of the bundled demo is the clearest signal the demo
            // converted into real work, so it is counted separately.
            if let Some(label) = self.demo_run_label() {
                crate::telemetry::capture_demo_experiment_started("run", label);
                crate::telemetry::capture_first_action("demo", "run_experiment");
            }
        }
        Ok(())
    }

    pub async fn cancel(&self, run: Option<&str>) -> Result<()> {
        ApiClient::discover()
            .await?
            .cancel(Some(&self.id), run)
            .await
    }

    /// One SSE request held until the condition; nothing polls.
    pub async fn wait(&self, deadline: Instant) -> Result<()> {
        let filter = crate::commands::up::api_v2::Filter {
            experiment: self
                .experiment
                .as_ref()
                .map(|experiment| experiment.id.clone()),
            project: self.project.as_ref().map(|project| project.id.clone()),
            wait: true,
            ..Default::default()
        };
        tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), async {
            ApiClient::discover().await?.wait(filter).await
        })
        .await
        .map_err(|_| anyhow!("Timed out waiting for a run state change."))?
    }

    // --- create-experiment ------------------------------------------------

    pub async fn create_experiment(&self, spec: CreateExperimentSpec) -> Result<()> {
        let store = &self.store;
        let project = self.project()?;
        let CreateExperimentSpec {
            title,
            parent,
            baseline,
            description,
            run_command,
        } = spec;

        let mut defaulted_to_root = false;
        let parent_exp = match &parent {
            Some(parent_id) => Some(store.get_local_experiment(parent_id)?.ok_or_else(|| {
                anyhow!(
                    "Parent experiment {} was not found in the orx store at {}. \
                     Choose an existing local experiment from `orx project view {}`, or omit --parent to branch off the project root.",
                    parent_id,
                    store.data_dir().display(),
                    project.id
                )
            })?),
            None if baseline => None,
            None => {
                let root = crate::local::experiments::project_root(store, &project.id)?;
                defaulted_to_root = root.is_some();
                root
            }
        };
        let kind = if parent_exp.is_some() {
            "child"
        } else {
            "baseline"
        };

        let experiment = crate::local::experiments::create_experiment(
            store,
            project,
            parent_exp.as_ref(),
            None,
            Some(title),
            description,
            run_command,
        )?;

        if project.github_enabled() {
            if let Err(error) = crate::local::git::spawn_branch_publication(
                std::path::Path::new(&project.repo_path),
                &experiment.branch_name,
                &project.github_owner,
                &project.github_repo,
            ) {
                eprintln!(
                    "  warning: experiment created locally, but GitHub sync could not start: {error}"
                );
            }
        }

        println!("\u{2713} Created local {} experiment", kind);
        if defaulted_to_root {
            let root = parent_exp.as_ref().unwrap();
            println!("  parent:  {} (project root, defaulted)", root.id);
        }
        if let Some(warning) = parent_exp
            .as_ref()
            .and_then(|p| crate::local::experiments::legacy_root_warning(project, p))
        {
            eprintln!("  {warning}");
        }
        println!("  id:      {}", experiment.id);
        println!("  title:   {}", experiment.display_name());
        println!("  slug:    {}", experiment.slug);
        println!("  branch:  {}", experiment.branch_name);
        if experiment.run_command.is_empty() {
            println!(
                "  command: — (none inherited — set one with `orx project edit {} --run-command '<cmd>'`)",
                project.id
            );
        } else {
            println!("  command: {}", experiment.run_command);
        }
        println!();
        super::checkout_hint::print(&project.repo_path, &experiment.branch_name);
        Ok(())
    }
}

fn experiment_wait_progress(run: Option<(&str, &str)>, last_status: &mut Option<String>) {
    match run {
        Some((id, status)) if last_status.as_deref() != Some(status) => {
            eprintln!("{id}  {status}");
            *last_status = Some(status.into());
        }
        None if last_status.is_none() => {
            eprintln!("No run yet for this experiment — waiting for one to start…");
            *last_status = Some(String::new());
        }
        _ => {}
    }
}

fn project_wait_progress(total: usize, in_flight: usize) {
    if in_flight == 0 {
        eprintln!("No runs in flight in this project ({total} run(s), all terminal).");
    } else {
        eprintln!("Watching {total} run(s) in project ({in_flight} in flight) — returning on the first completion…");
    }
}

fn warn_unreachable_trackio(backend: &str) {
    use crate::local::trackio::{self, Target};
    if !crate::compute::backend(backend).is_ok_and(|b| b.capabilities().remote) {
        return;
    }
    match trackio::target() {
        Target::Server(url) if trackio::is_local_only(&url) => eprintln!(
            "warning: {} is {url}, which a {backend} run may not reach, so its Trackio metrics may not arrive. Save an address the remote machine can reach.",
            trackio::SERVER_URL
        ),
        // Only for someone already using Trackio, so non-users see no noise.
        Target::Local if trackio::used_locally() => eprintln!(
            "warning: no {} or {} is saved, so this {backend} run's Trackio metrics stay on that machine and orx will not show them.",
            trackio::SPACE_ID,
            trackio::SERVER_URL
        ),
        _ => {}
    }
}

/// True when `error` is a transient SQLite lock ("database is locked") —
/// contention a wait command should outlast, not die on.
pub(crate) fn is_db_locked(error: &crate::error::Error) -> bool {
    matches!(
        error.downcast_ref::<rusqlite::Error>(),
        Some(rusqlite::Error::SqliteFailure(failure, _))
            if failure.code == rusqlite::ErrorCode::DatabaseBusy
    )
}

fn note_db_locked(notified: &mut bool) {
    if !*notified {
        eprintln!("orx.db is locked by another process — retrying within the wait timeout…");
        *notified = true;
    }
}

/// Lock wait for the next store call: the connection default, shrunk to the
/// time left before `deadline` so one blocked operation can't overshoot it.
pub(crate) fn busy_timeout_left(deadline: Instant) -> Duration {
    deadline
        .saturating_duration_since(Instant::now())
        .min(crate::store::BUSY_TIMEOUT)
}

/// Run `attempt`, retrying while the store reports a transient lock, until
/// `deadline` — after which it reports the wait's usual timeout error.
pub(crate) async fn retry_on_db_lock<T>(
    mut attempt: impl FnMut() -> Result<T>,
    interval: Duration,
    deadline: Instant,
) -> Result<T> {
    let mut notified = false;
    loop {
        match attempt() {
            Err(error) if is_db_locked(&error) => note_db_locked(&mut notified),
            Err(error) => return Err(error),
            Ok(value) => {
                // An attempt that finished only after the deadline timed out.
                sleep_until_or_timeout(Duration::ZERO, deadline).await?;
                return Ok(value);
            }
        }
        sleep_until_or_timeout(interval, deadline).await?;
    }
}

async fn sleep_until_or_timeout(interval: Duration, deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline {
        return Err(anyhow!("Timed out waiting for a run state change."));
    }
    let nap = interval.min(deadline.saturating_duration_since(Instant::now()));
    tokio::time::sleep(nap).await;
    if Instant::now() >= deadline {
        return Err(anyhow!("Timed out waiting for a run state change."));
    }
    Ok(())
}

#[derive(Clone)]
pub(crate) struct ApiClient {
    server: crate::commands::remote_host::ServerDescriptor,
    http: reqwest::Client,
}

impl ApiClient {
    pub async fn discover() -> Result<Self> {
        Self::new(crate::commands::remote_host::discover().await?)
    }

    pub(crate) fn new(server: crate::commands::remote_host::ServerDescriptor) -> Result<Self> {
        Ok(Self {
            server,
            http: crate::net::loopback_client().build()?,
        })
    }

    pub fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(
                method,
                format!("http://127.0.0.1:{}/api/v2/{path}", self.server.port),
            )
            .bearer_auth(&self.server.token)
    }

    pub async fn json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let request = |client: &Self| {
            let mut request = client.request(method.clone(), path);
            if let Some(body) = body {
                request = request.json(body);
            }
            request
        };
        let response = request(self).send().await;
        let retry = match &response {
            Ok(response) => response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE,
            Err(error) => {
                error.is_connect() || method == reqwest::Method::GET && error.is_request()
            }
        };
        let response = if retry {
            drop(response);
            request(&Self::discover().await?).send().await?
        } else {
            response?
        };
        if !response.status().is_success() {
            return Err(anyhow!("{}", response.text().await?));
        }
        Ok(response.json().await?)
    }

    pub async fn connect(
        &self,
        mut body: serde_json::Value,
        prompt: crate::engine::transport::Prompt,
    ) -> Result<serde_json::Value> {
        use axum::response::IntoResponse as _;
        use sha2::{Digest as _, Sha256};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let token = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        body["prompt"] = serde_json::json!({"port":listener.local_addr()?.port(), "token":token});
        let auth = crate::commands::remote_host::RemoteAuth::new(&token);
        let app = axum::Router::new().route(
            "/prompt",
            axum::routing::post(
                move |headers: axum::http::HeaderMap, axum::Json(question): axum::Json<String>| {
                    let auth = auth.clone();
                    let prompt = prompt.clone();
                    async move {
                        let authorized = headers
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .and_then(|value| value.strip_prefix("Bearer "))
                            .is_some_and(|token| {
                                auth.matches_callback(&Sha256::digest(token.as_bytes()))
                            });
                        if !authorized {
                            return axum::http::StatusCode::UNAUTHORIZED.into_response();
                        }
                        match prompt(question).await {
                            Ok(answer) => axum::Json(answer).into_response(),
                            Err(error) => (axum::http::StatusCode::BAD_REQUEST, error.to_string())
                                .into_response(),
                        }
                    }
                },
            ),
        );
        let mut server = tokio::task::JoinSet::new();
        server.spawn(async move { axum::serve(listener, app).await });
        self.json(reqwest::Method::POST, "compute/connect", Some(&body))
            .await
    }

    pub async fn launch(&self, mut args: ExpRunArgs) -> Result<()> {
        args.chat_session_id = args.launching_chat_session();
        args.agent_origin = crate::agent_origin();
        args.caller_config_dir = Some(crate::commands::up::absolute_config_dir());
        args.caller_python_overrides = crate::jobs::caller_python_overrides();
        args.caller_compute_settings = Some(
            crate::commands::up::launch_settings(&args)
                .await?
                .filter(|(_, set)| *set)
                .map(|(settings, _)| settings)
                .unwrap_or_default(),
        );
        args.invocation_context = args
            .invocation_identity()?
            .map(|identity| serde_json::to_string(&identity))
            .transpose()?;
        let run: Run = serde_json::from_value(
            self.json(
                reqwest::Method::POST,
                "runs",
                Some(&serde_json::to_value(args)?),
            )
            .await?,
        )?;
        if run.status == "failed" {
            return Err(anyhow!(
                "Compute submission failed for run {}: {}",
                run.id,
                run.failure_detail()
                    .unwrap_or_else(|| "no reason recorded".into())
            ));
        }
        println!("\u{2713} Run {} submitted ({}).", run.id, run.status);
        println!(
            "{}",
            crate::invocation::follow_up(&run.experiment_id, &run.id)
        );
        Ok(())
    }

    pub async fn abandon(&self, run: &str) -> Result<()> {
        let path = format!("runs/{}/abandon", urlencoding::encode(run));
        self.json(reqwest::Method::POST, &path, None).await?;
        println!("Run {run} abandoned; its cancel stays pending until the job is confirmed gone.");
        Ok(())
    }

    pub async fn cancel(&self, experiment: Option<&str>, selected: Option<&str>) -> Result<()> {
        let path = match (experiment, selected) {
            (_, Some(run)) => format!("runs/{}", urlencoding::encode(run)),
            (Some(experiment), None) => {
                format!("runs?experiment={}", urlencoding::encode(experiment))
            }
            (None, None) => return Err(anyhow!("Pass an experiment id or --run <id>.")),
        };
        let response = self.json(reqwest::Method::GET, &path, None).await?;
        let rows = if selected.is_some() {
            if experiment
                .is_some_and(|experiment| response["experimentId"].as_str() != Some(experiment))
            {
                return Err(anyhow!("Run does not belong to this experiment."));
            }
            vec![response]
        } else {
            response["runs"]
                .as_array()
                .ok_or_else(|| anyhow!("Server returned no runs"))?
                .clone()
        };
        let in_flight: Vec<_> = rows
            .iter()
            .filter(|run| {
                !crate::engine::Phase::parse(run["status"].as_str().unwrap_or_default())
                    .is_some_and(crate::engine::Phase::is_terminal)
            })
            .map(|run| {
                run["id"]
                    .as_str()
                    .ok_or_else(|| anyhow!("Server returned no run id"))
            })
            .collect::<Result<_>>()?;
        if in_flight.is_empty() {
            return Err(match selected {
                Some(id) => anyhow!("Run {id} has already finished."),
                None => anyhow!("No run in flight for this experiment."),
            });
        }
        println!("Cancelling runs:");
        for id in &in_flight {
            println!("  {id}");
        }
        for id in in_flight {
            let request = serde_json::json!({
                "chatSessionId": crate::local::chat::launching_chat_session(),
            });
            let run: Run = serde_json::from_value(
                self.json(
                    reqwest::Method::POST,
                    &format!("runs/{}/cancel", urlencoding::encode(id)),
                    Some(&request),
                )
                .await?,
            )?;
            println!(
                "\u{2713} Cancel requested for run {id}: {}.",
                run.display_status()
            );
            for line in [run.contact_detail(), run.failure_detail(), run.next_step()]
                .into_iter()
                .flatten()
            {
                println!("  {line}");
            }
        }
        Ok(())
    }

    pub async fn wait(&self, filter: crate::commands::up::api_v2::Filter) -> Result<()> {
        use futures::StreamExt;
        let mut client = self.clone();
        let mut original_in_flight: Option<std::collections::HashSet<String>> = None;
        let mut last_status = None;
        loop {
            let mut events = client.events(&filter).await?;
            while let Some(event) = events.next().await {
                let mut value = match event {
                    Ok(value) => value,
                    Err(error)
                        if error.chain().any(|cause| {
                            cause.is::<std::io::Error>() || cause.is::<reqwest::Error>()
                        }) =>
                    {
                        break
                    }
                    Err(error) => return Err(error),
                };
                if let Some(rows) = value["snapshot"].as_array() {
                    let first = original_in_flight.is_none();
                    let original = original_in_flight.get_or_insert_with(|| {
                        rows.iter()
                            .filter(|run| {
                                !crate::engine::Phase::parse(
                                    run["status"].as_str().unwrap_or_default(),
                                )
                                .is_some_and(crate::engine::Phase::is_terminal)
                            })
                            .filter_map(|run| run["id"].as_str().map(str::to_owned))
                            .collect()
                    });
                    if filter.project.is_some() {
                        if first {
                            project_wait_progress(rows.len(), original.len());
                        }
                        if let Some(run) = rows.iter().find(|run| {
                            run["id"].as_str().is_some_and(|id| original.contains(id))
                                && crate::engine::Phase::parse(
                                    run["status"].as_str().unwrap_or_default(),
                                )
                                .is_some_and(crate::engine::Phase::is_terminal)
                        }) {
                            value = run.clone();
                        } else if first && original.is_empty() {
                            println!("drained: no runs in flight");
                            return Ok(());
                        } else {
                            continue;
                        }
                    } else if let Some(run) = rows.first() {
                        value = run.clone();
                    } else {
                        experiment_wait_progress(None, &mut last_status);
                        continue;
                    }
                }
                if value["drained"] == true {
                    println!("drained: no runs in flight");
                    return Ok(());
                }
                let run: Run = serde_json::from_value(value)?;
                if crate::engine::Phase::parse(&run.status)
                    .is_some_and(crate::engine::Phase::is_terminal)
                {
                    println!("{} {}", run.id, run.display_status());
                    if let Some(detail) = run.contact_detail() {
                        eprintln!("{detail}");
                    }
                    if let Some(detail) = run.failure_detail() {
                        eprintln!("{detail}");
                    }
                    return Ok(());
                }
                if filter.project.is_none() {
                    experiment_wait_progress(
                        Some((&run.id, &run.display_status())),
                        &mut last_status,
                    );
                }
            }
            drop(events);
            client = Self::discover().await?;
        }
    }

    pub async fn events(
        &self,
        filter: &crate::commands::up::api_v2::Filter,
    ) -> Result<impl futures::Stream<Item = Result<serde_json::Value>>> {
        use futures::{StreamExt, TryStreamExt};
        let response = self
            .request(reqwest::Method::GET, "events")
            .query(filter)
            .send()
            .await;
        let retry = match &response {
            Ok(response) => response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE,
            Err(error) => error.is_connect() || error.is_request(),
        };
        let response = if retry {
            drop(response);
            Self::discover()
                .await?
                .request(reqwest::Method::GET, "events")
                .query(filter)
                .send()
                .await?
        } else {
            response?
        };
        if !response.status().is_success() {
            return Err(anyhow!("{}", response.text().await?));
        }
        let stream = response.bytes_stream().map_err(std::io::Error::other);
        let reader = tokio_util::io::StreamReader::new(stream);
        let lines = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(reader));
        Ok(futures::stream::try_unfold(
            (lines, String::new(), String::new()),
            |(mut lines, mut data, mut kind)| async move {
                while let Some(line) = lines.next_line().await? {
                    if line.is_empty() {
                        if data.is_empty() {
                            kind.clear();
                            continue;
                        }
                        if kind == "error" {
                            return Err(anyhow!("{data}"));
                        }
                        if !matches!(kind.as_str(), "run" | "snapshot") {
                            data.clear();
                            kind.clear();
                            continue;
                        }
                        let value: serde_json::Value = serde_json::from_str(&data)?;
                        return Ok(Some((value, (lines, String::new(), String::new()))));
                    }
                    if let Some(value) = line.strip_prefix("data:") {
                        if !data.is_empty() {
                            data.push('\n');
                        }
                        data.push_str(value.strip_prefix(' ').unwrap_or(value));
                    } else if let Some(value) = line.strip_prefix("event:") {
                        kind = value.trim().into();
                    }
                    if data.len() > 2 * 1024 * 1024 {
                        return Err(anyhow!("Server event exceeds 2 MiB"));
                    }
                }
                Ok(None)
            },
        )
        .boxed())
    }

    pub async fn logs(&self, id: &str, follow: bool) -> Result<()> {
        let result = self.logs_to(id, follow, &mut std::io::stdout()).await;
        // Of this call's I/O, only writing stdout can break a pipe.
        if let Err(error) = &result {
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::BrokenPipe)
            {
                crate::output::stdout_closed();
            }
        }
        result
    }

    pub(crate) async fn logs_to(
        &self,
        id: &str,
        follow: bool,
        output: &mut (impl std::io::Write + Send),
    ) -> Result<()> {
        use futures::StreamExt;
        let filter = crate::commands::up::api_v2::Filter {
            run: Some(id.into()),
            follow: true,
            ..Default::default()
        };
        let mut client = self.clone();
        let mut events = if follow {
            Some(client.events(&filter).await?)
        } else {
            None
        };
        let mut cursor = 0;
        let mut identity: Option<crate::engine::FileIdent> = None;
        let mut finished = !follow;
        let mut backoff = Duration::from_millis(250);
        'follow: loop {
            let mut reset = false;
            loop {
                let mut request = client
                    .request(
                        reqwest::Method::GET,
                        &format!("runs/{}/log", urlencoding::encode(id)),
                    )
                    .timeout(Duration::from_secs(35))
                    .query(&[
                        ("start", cursor),
                        ("end", cursor + orx_host_proto::MAX_PAGE_LEN),
                    ]);
                if let Some(ident) = &identity {
                    request = request.query(&[("ident", String::from(ident.clone()))]);
                }
                let response = match request.send().await {
                    Ok(response) => response,
                    Err(error)
                        if follow
                            && (error.is_connect() || error.is_request() || error.is_body()) =>
                    {
                        client = Self::discover().await?;
                        events = Some(client.events(&filter).await?);
                        continue 'follow;
                    }
                    Err(error) => return Err(error.into()),
                };
                if follow && response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
                    drop(response);
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(5));
                    continue;
                }
                if !response.status().is_success() {
                    return Err(anyhow!("{}", response.text().await?));
                }
                let bytes = match response.bytes().await {
                    Ok(bytes) => bytes,
                    Err(error) if follow && error.is_body() => {
                        client = Self::discover().await?;
                        events = Some(client.events(&filter).await?);
                        continue 'follow;
                    }
                    Err(error) => return Err(error.into()),
                };
                backoff = Duration::from_millis(250);
                let page: crate::engine::LogPage = serde_json::from_slice(&bytes)?;
                match page {
                    crate::engine::LogPage::Data {
                        len: _,
                        ident,
                        start,
                        bytes,
                        eof,
                    } => {
                        if start != cursor
                            || identity.as_ref().is_some_and(|expected| *expected != ident)
                        {
                            return Err(anyhow!(
                                "Server returned an unexpected log generation or offset"
                            ));
                        }
                        identity = Some(ident);
                        output.write_all(&bytes)?;
                        cursor += bytes.len() as u64;
                        if eof || bytes.is_empty() {
                            break;
                        }
                    }
                    crate::engine::LogPage::Reset { info } => {
                        if reset {
                            return Err(anyhow!("Log keeps changing; try again"));
                        }
                        reset = true;
                        if cursor != 0 || identity.is_some() {
                            writeln!(output, "\n[log replaced; starting new generation]")?;
                        }
                        cursor = 0;
                        identity = info.as_ref().map(|info| info.ident.clone());
                        if info.is_none() {
                            break;
                        }
                    }
                }
            }
            output.flush()?;
            if finished {
                return Ok(());
            }
            let event = events
                .as_mut()
                .expect("following has an event stream")
                .next()
                .await;
            match event {
                Some(Ok(event)) => {
                    finished =
                        crate::engine::Phase::parse(event["status"].as_str().unwrap_or_default())
                            .is_some_and(crate::engine::Phase::is_terminal)
                }
                Some(Err(error))
                    if !error.chain().any(|cause| {
                        cause.is::<std::io::Error>() || cause.is::<reqwest::Error>()
                    }) =>
                {
                    return Err(error)
                }
                _ => {
                    client = Self::discover().await?;
                    events = Some(client.events(&filter).await?);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_busy() -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            Some("database is locked".to_string()),
        )
    }

    #[test]
    fn is_db_locked_matches_only_sqlite_busy() {
        let busy: crate::error::Error = db_busy().into();
        assert!(is_db_locked(&busy));

        let busy_snapshot: crate::error::Error = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY_SNAPSHOT),
            None,
        )
        .into();
        assert!(is_db_locked(&busy_snapshot));

        assert!(!is_db_locked(&anyhow!("some other error")));
    }

    #[tokio::test]
    async fn retry_on_db_lock_retries_busy_until_success() {
        let mut attempts = 0;
        let result = retry_on_db_lock(
            || {
                attempts += 1;
                if attempts < 3 {
                    Err(db_busy().into())
                } else {
                    Ok(attempts)
                }
            },
            Duration::from_millis(1),
            Instant::now() + Duration::from_secs(10),
        )
        .await;
        assert_eq!(result.unwrap(), 3);
    }

    #[tokio::test]
    async fn retry_on_db_lock_propagates_other_errors() {
        let result: Result<()> = retry_on_db_lock(
            || Err(anyhow!("not a lock")),
            Duration::from_millis(1),
            Instant::now() + Duration::from_secs(10),
        )
        .await;
        assert_eq!(result.unwrap_err().to_string(), "not a lock");
    }

    #[tokio::test]
    async fn retry_on_db_lock_times_out_while_locked() {
        let result: Result<()> = retry_on_db_lock(
            || Err(db_busy().into()),
            Duration::from_millis(1),
            Instant::now() + Duration::from_millis(30),
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("Timed out"));
    }

    #[tokio::test]
    async fn retry_on_db_lock_reports_timeout_when_unblocked_past_deadline() {
        let result: Result<()> = retry_on_db_lock(
            || {
                std::thread::sleep(Duration::from_millis(30));
                Ok(())
            },
            Duration::from_millis(1),
            Instant::now() + Duration::from_millis(10),
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("Timed out"));
    }
}
