//! Resumable, leased artifact searches. Limit physical workers even if the OS
//! leaves a cancelled search blocked in a network filesystem syscall.
use super::*;
use crate::local::files::{ArtifactSearch, ArtifactSearchWalk};
use std::sync::Mutex;
use std::time::Instant;

// Background browser tabs may throttle heartbeats to once a minute.
const LEASE: Duration = Duration::from_secs(5 * 60);
const BUDGETS: [Option<Duration>; 3] = [
    Some(Duration::from_secs(5)),
    Some(Duration::from_secs(15 * 60)),
    None,
];

#[derive(Clone)]
pub(super) struct Searches {
    jobs: Arc<Mutex<HashMap<String, Arc<Job>>>>,
    workers: Arc<tokio::sync::Semaphore>,
}
impl Default for Searches {
    fn default() -> Self {
        Self {
            jobs: Default::default(),
            workers: Arc::new(tokio::sync::Semaphore::new(2)),
        }
    }
}

struct Job {
    project: String,
    state: Mutex<Progress>,
}
struct Progress {
    status: &'static str,
    stage: usize,
    deadline: Option<Instant>,
    touched: Instant,
    result: Option<ArtifactSearch>,
    error: Option<String>,
}
impl Progress {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            status: "running",
            stage: 0,
            deadline: Some(now + BUDGETS[0].unwrap()),
            touched: now,
            result: None,
            error: None,
        }
    }
    fn tick(&mut self, now: Instant) {
        if now.duration_since(self.touched) >= LEASE {
            self.status = "cancelled";
        } else if self.status == "running" && self.deadline.is_some_and(|d| now >= d) {
            self.status = "paused";
        }
    }
    fn resume(&mut self) -> bool {
        if self.status != "paused" || self.stage == 2 {
            return false;
        }
        self.stage += 1;
        self.deadline = BUDGETS[self.stage].map(|budget| Instant::now() + budget);
        self.status = "running";
        true
    }
}
impl Job {
    fn running(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        state.tick(Instant::now());
        state.status == "running"
    }
    fn snapshot(&self, id: &str) -> Value {
        let mut state = self.state.lock().unwrap();
        state.tick(Instant::now());
        state.touched = Instant::now();
        json!({ "id": id, "status": state.status, "stage": state.stage,
            "result": state.result, "error": state.error })
    }
}

#[derive(Deserialize)]
pub(super) struct SearchRequest {
    q: String,
    after: Option<String>,
}

pub(super) async fn start(
    State(app): State<AppState>,
    Path(project): Path<String>,
    Json(req): Json<SearchRequest>,
) -> ApiResult {
    reject_if_moving(&app)?;
    if req.q.len() > 1024 || req.after.as_ref().is_some_and(|s| s.len() > 4096) {
        return Err(bad_request("search query is too long"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let job = Arc::new(Job {
        project: project.clone(),
        state: Mutex::new(Progress::new()),
    });
    {
        let mut jobs = app.artifact_searches.jobs.lock().unwrap();
        jobs.retain(|_, job| job.state.lock().unwrap().touched.elapsed() < LEASE);
        if jobs.len() >= 64 {
            return Err(bad_request(
                "too many artifact searches; cancel an earlier search",
            ));
        }
        jobs.insert(id.clone(), job.clone());
    }
    let response = job.snapshot(&id);
    tokio::spawn(run(job, app.artifact_searches.workers, req));
    Ok(Json(response))
}

async fn run(job: Arc<Job>, workers: Arc<tokio::sync::Semaphore>, req: SearchRequest) {
    let mut walk = None;
    loop {
        if !job.running() {
            if job.state.lock().unwrap().status == "cancelled" {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        // Waiting for a worker must remain cancellable and respect the timer.
        let Ok(Ok(permit)) =
            tokio::time::timeout(Duration::from_millis(100), workers.clone().acquire_owned()).await
        else {
            continue;
        };
        let worker_job = job.clone();
        let query = req.q.clone();
        let after = req.after.clone();
        let step = tokio::task::spawn_blocking(move || -> Result<(ArtifactSearchWalk, bool)> {
            let _permit = permit;
            let mut walk = match walk {
                Some(walk) => walk,
                None => {
                    let store = Store::open()?;
                    let project = store
                        .get_local_project(&worker_job.project)?
                        .ok_or_else(|| anyhow!("project not found"))?;
                    let base = crate::paths::canonicalize(&local::files::ensure_dir(&project)?)?;
                    ArtifactSearchWalk::new(base, query, after)
                }
            };
            let complete = walk.advance(|| worker_job.running());
            Ok((walk, complete))
        })
        .await;
        let mut state = job.state.lock().unwrap();
        if state.status == "cancelled" {
            return;
        }
        match step {
            Ok(Ok((remaining, false))) => walk = Some(remaining),
            Ok(Ok((remaining, true))) => {
                state.result = Some(remaining.finish());
                state.status = "complete";
                return;
            }
            error => {
                state.error = Some(match error {
                    Ok(Err(e)) => e.to_string(),
                    Err(e) => e.to_string(),
                    _ => unreachable!(),
                });
                state.status = "failed";
                return;
            }
        }
    }
}

fn find(app: &AppState, project: &str, id: &str) -> std::result::Result<Arc<Job>, ApiError> {
    app.artifact_searches
        .jobs
        .lock()
        .unwrap()
        .get(id)
        .filter(|job| job.project == project)
        .cloned()
        .ok_or_else(|| not_found("search"))
}
pub(super) async fn status(
    State(app): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> ApiResult {
    Ok(Json(find(&app, &project, &id)?.snapshot(&id)))
}
pub(super) async fn resume(
    State(app): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> ApiResult {
    let job = find(&app, &project, &id)?;
    {
        let mut state = job.state.lock().unwrap();
        state.tick(Instant::now());
        if state.status != "complete" && !state.resume() {
            return Err(bad_request("search is not paused"));
        }
        state.touched = Instant::now();
    }
    Ok(Json(job.snapshot(&id)))
}
pub(super) async fn cancel(
    State(app): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> ApiResult {
    let job = find(&app, &project, &id)?;
    job.state.lock().unwrap().status = "cancelled";
    app.artifact_searches.jobs.lock().unwrap().remove(&id);
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn budgets_pause_resume_and_expire_instead_of_restarting() {
        let mut progress = Progress::new();
        assert_eq!(
            progress.deadline.unwrap().duration_since(progress.touched),
            Duration::from_secs(5)
        );
        progress.tick(progress.touched + Duration::from_secs(4));
        assert_eq!(progress.status, "running");
        assert!(!progress.resume());
        progress.tick(progress.deadline.unwrap());
        assert_eq!(progress.status, "paused");
        assert!(progress.resume());
        assert_eq!(progress.stage, 1);
        assert!(
            progress.deadline.unwrap().duration_since(Instant::now()) > Duration::from_secs(899)
        );
        progress.touched = progress.deadline.unwrap(); // a polling client keeps the lease alive
        progress.tick(progress.deadline.unwrap());
        assert!(progress.resume());
        assert_eq!(progress.stage, 2);
        assert!(progress.deadline.is_none());
        assert!(!progress.resume());
        progress.tick(progress.touched + LEASE);
        assert_eq!(progress.status, "cancelled");
        assert!(!progress.resume());
    }
}
