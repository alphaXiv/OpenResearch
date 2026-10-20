//! Local implementations of project, experiment, run, and log commands.

use std::collections::HashMap;
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
        let store = &self.store;
        let project_id = &self.id;
        let titles: HashMap<String, String> = store
            .list_experiments_by_project(project_id)?
            .into_iter()
            .map(|e| (e.id.clone(), e.display_name().to_string()))
            .collect();

        let runs = store.list_runs_by_project(project_id)?;
        let runs: Vec<Run> = runs.iter().map(Run::from).collect();
        Ok(RunListing { runs, titles })
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
                let run = Run::from(&r);
                let commit = run
                    .commit_sha
                    .as_deref()
                    .map(|s| s.chars().take(7).collect::<String>())
                    .unwrap_or_else(|| "—".to_string());
                println!(
                    "  last run: {} ({}, commit {}, ran {}, updated {})",
                    run.id,
                    run.status,
                    commit,
                    crate::output::format_duration(run.duration_secs),
                    run.updated_display
                );
                if let Ok(backend) = crate::jobs::BackendDescriptor::parse(&r.backend_json) {
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
        // Fill backend/flavor from the persisted default
        // BEFORE the flag validations below, so e.g. `--host box1` with a default
        // of `ssh` is a valid launch, and before `backend_label` is captured, so
        // telemetry records the resolved backend.
        crate::local::apply_compute_default(&mut args.backend, &mut args.flavor);
        if args.backend.is_none() {
            args.backend = Some("local".to_string());
        }
        crate::compute::validate_run_args(&args)?;
        if let Some(backend) = args.backend.as_deref() {
            warn_unreachable_trackio(backend);
        }
        args.agent_origin = crate::agent_origin();
        // Coarse backend label for analytics; the backend name is already an
        // enum, never user data. Recorded before the (borrowing) dispatch below.
        let backend_label = args.backend.clone();
        let result = match crate::local::chat::trusted_up_port()? {
            Some(port) => {
                let summary = crate::commands::up::submit_run_via_up(port, &args).await?;
                let backend = args.backend.as_deref().unwrap_or("local");
                println!("\u{2713} {backend} run submitted by orx up.");
                if let Some(job_id) = summary.job_id {
                    let label = if backend == "local" { "dir" } else { "job" };
                    println!("  {label}  {job_id}");
                }
                if let Some(resources) = summary.slurm_resources {
                    println!("  {resources}");
                }
                println!("  run  {}", summary.run_id);
                println!(
                    "{}",
                    crate::invocation::follow_up(&summary.experiment_id, &summary.run_id)
                );
                Ok(())
            }
            None => match args.backend.as_deref() {
                Some("hf") => crate::local::hf::launch_local_hf(&args).await,
                Some("modal") => crate::local::modal::launch_local_modal(&args).await,
                Some("k8s") => crate::local::k8s::launch_local_k8s(&args).await,
                Some("ssh") => crate::local::ssh::launch_local_ssh(&args).await,
                Some("slurm") => crate::local::slurm::launch_local_slurm(&args).await,
                Some("ray") => crate::local::ray::launch_local_ray(&args).await,
                Some("openresearch") => {
                    crate::local::openresearch::launch_local_openresearch(&args).await
                }
                Some("tinker" | "local") => crate::local::localrun::launch_local_run(&args).await,
                Some(other) => Err(anyhow!(
                    "Unknown --backend '{}'. Local experiments support: hf (Hugging Face Jobs), \
                     modal (Modal serverless GPUs), k8s (your Kubernetes cluster), ssh (your own box), \
                     slurm (your Slurm cluster), ray (a Ray Jobs cluster), \
                     openresearch (an ephemeral OpenResearch box), tinker (local controller with remote model compute), \
                     local (this machine).",
                    other
                )),
                None => Err(anyhow!(
                    "No --backend given and no default compute target is set. \
                     Configure a default compute target in OpenResearch, \
                     or pass one per launch: \
                     `--backend hf --flavor <flavor>` (e.g. --flavor a10g-small), \
                     `--backend modal --flavor <flavor>` (e.g. --flavor a10g), \
                     `--backend k8s` (runs the manifest committed on the branch — \
                     default .orx/k8s.yaml, or --manifest <path>), \
                     `--backend ssh --host <alias>` (an ~/.ssh/config alias), \
                     `--backend slurm [--host <alias>] [--flavor h100:2]` (your Slurm cluster), \
                     `--backend ray [--flavor gpu:1]` (a Ray Jobs cluster), \
                     `--backend openresearch --flavor <shape>` (an ephemeral OpenResearch box, \
                     e.g. --flavor h100_sxm or cpu5c; needs `orx login`), \
                     `--backend tinker` (a local controller using remote Tinker model compute), \
                     or `--backend local` (a detached process on this machine)."
                )),
            },
        };
        // Key event, fired only on a successful launch. Coarse backend only.
        // Validation above guarantees a known backend before either dispatch path.
        if result.is_ok() && backend_label.as_deref() != Some("openresearch") {
            let target = backend_label.as_deref().unwrap_or("unknown");
            crate::telemetry::capture_experiment_started("run", true, Some(target));
            // A launch out of the bundled demo is the clearest signal the demo
            // converted into real work, so it is counted separately.
            if let Some(label) = self.demo_run_label() {
                crate::telemetry::capture_demo_experiment_started("run", label);
                crate::telemetry::capture_first_action("demo", "run_experiment");
            }
        }
        result
    }

    pub async fn cancel(&self) -> Result<()> {
        let store = &self.store;
        let exp = self.experiment()?;
        let in_flight: Vec<_> = store
            .list_runs_by_experiment(&exp.id)?
            .into_iter()
            .filter(|r| !crate::local::is_terminal(&r.status))
            .collect();
        if in_flight.is_empty() {
            return Err(anyhow!("No run in flight for this experiment."));
        }
        let trusted_port = crate::local::chat::trusted_up_port()?;
        for r in &in_flight {
            match trusted_port {
                Some(port) => crate::commands::up::cancel_run_via_up(port, &r.id).await?,
                None => crate::commands::exp::request_local_run_cancel(
                    store,
                    &r.id,
                    &crate::commands::exp::exp_cancel_reason(
                        crate::local::chat::launching_chat_session().as_deref(),
                    ),
                )?,
            }
            println!("\u{2713} Cancel requested for run {}.", r.id);
        }
        Ok(())
    }

    pub async fn wait_experiment(&self, interval: Duration, deadline: Instant) -> Result<()> {
        let store = &self.store;
        let exp_id = &self.id;
        let mut last_status: Option<String> = None;
        let mut lock_notified = false;
        loop {
            store.set_busy_timeout(busy_timeout_left(deadline))?;
            let run = match store.latest_run_for_experiment(exp_id) {
                Ok(run) => run,
                Err(error) if is_db_locked(&error) => {
                    note_db_locked(&mut lock_notified);
                    sleep_until_or_timeout(interval, deadline).await?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            match run {
                None => {
                    if last_status.is_none() {
                        eprintln!("No run yet for this experiment — waiting for one to start…");
                        last_status = Some(String::new());
                    }
                }
                Some(r) => {
                    if last_status.as_deref() != Some(r.status.as_str()) {
                        eprintln!("{}  {}", r.id, r.status);
                        last_status = Some(r.status.clone());
                    }
                    if crate::local::is_terminal(&r.status) {
                        let run = Run::from(&r);
                        println!("{} {}", run.id, run.status);
                        if let Some(detail) = run.failure_detail() {
                            eprintln!("{detail}");
                        }
                        return Ok(());
                    }
                }
            }
            sleep_until_or_timeout(interval, deadline).await?;
        }
    }

    pub async fn wait_project(&self, interval: Duration, deadline: Instant) -> Result<()> {
        let store = &self.store;
        let project_id = &self.id;
        let snapshot: HashMap<String, String> = retry_on_db_lock(
            || {
                store.set_busy_timeout(busy_timeout_left(deadline))?;
                store.list_runs_by_project(project_id)
            },
            interval,
            deadline,
        )
        .await?
        .into_iter()
        .map(|r| (r.id, r.status))
        .collect();
        let in_flight = snapshot
            .values()
            .filter(|s| !crate::local::is_terminal(s))
            .count();

        if in_flight == 0 {
            eprintln!(
                "No runs in flight in this project ({} run(s), all terminal).",
                snapshot.len()
            );
            println!("drained: no runs in flight");
            return Ok(());
        }

        eprintln!(
            "Watching {} run(s) in project ({} in flight) — returning on the first completion…",
            snapshot.len(),
            in_flight
        );

        let mut lock_notified = false;
        loop {
            sleep_until_or_timeout(interval, deadline).await?;

            store.set_busy_timeout(busy_timeout_left(deadline))?;
            let current = match store.list_runs_by_project(project_id) {
                Ok(current) => current,
                Err(error) if is_db_locked(&error) => {
                    note_db_locked(&mut lock_notified);
                    continue;
                }
                Err(error) => return Err(error),
            };
            let mut completed: Vec<(String, Option<String>)> = Vec::new();
            for r in &current {
                if !crate::local::is_terminal(&r.status) {
                    continue;
                }
                let line = match snapshot.get(&r.id) {
                    Some(prev) if crate::local::is_terminal(prev) => continue,
                    Some(prev) => format!("{} {} -> {}", r.id, prev, r.status),
                    None => format!("{} {} (new)", r.id, r.status),
                };
                completed.push((line, Run::from(r).failure_detail()));
            }
            if !completed.is_empty() {
                for (line, detail) in &completed {
                    println!("{line}");
                    if let Some(detail) = detail {
                        eprintln!("{detail}");
                    }
                }
                return Ok(());
            }
        }
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
                    "Parent experiment {} not found in the local store. \
                     Choose an existing local experiment from `orx project view {}`, or omit --parent to branch off the project root.",
                    parent_id, project.id
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
        println!("To edit it, check out the branch in the project's local clone:");
        println!("  cd {}", project.repo_path);
        println!("  git checkout {}", experiment.branch_name);
        println!("  # …edit, then…");
        println!("  git commit -am \"<msg>\"");
        Ok(())
    }
}

/// hf runs get no saved variables, so only other remote backends can be misdirected.
fn warn_unreachable_trackio(backend: &str) {
    let Some(url) = crate::local::trackio::server_url() else {
        return;
    };
    let remote = crate::compute::backend(backend).is_ok_and(|b| b.capabilities().remote);
    if remote && backend != "hf" && crate::local::trackio::is_local_only(&url) {
        eprintln!(
            "warning: {} is {url}, which a {backend} run may not reach, so its Trackio metrics may not arrive. Save an address the remote machine can reach.",
            crate::local::trackio::SERVER_URL
        );
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
