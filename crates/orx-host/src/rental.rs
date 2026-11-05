use serde::Deserialize;
use std::io;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const COLLECTION_GRACE: Duration = Duration::from_secs(15 * 60);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rental {
    api_url: String,
    sandbox_id: String,
    token: String,
}

impl Rental {
    fn operation(&self, operation: &str) -> io::Result<()> {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .build();
        let client: ureq::Agent = config.into();
        client
            .post(format!(
                "{}/sandboxes/{}/lease/{operation}",
                self.api_url.trim_end_matches('/'),
                self.sandbox_id
            ))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "application/json")
            .header("User-Agent", "orx-host")
            .send("{}")
            .map(|_| ())
            .map_err(|_| io::Error::other(format!("rental {operation} unavailable")))
    }
}

pub(crate) struct Duty {
    finished: mpsc::Sender<()>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Duty {
    pub(crate) fn start(dir: &Path) -> io::Result<Option<Self>> {
        #[derive(Deserialize)]
        struct Config {
            rental: Rental,
        }
        let config: Config = match crate::read_json(&dir.join("rental.json")) {
            Ok(config) => config,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        config.rental.operation("activate")?;
        let (finished, receiver) = mpsc::channel();
        let dir = dir.to_owned();
        let worker = std::thread::spawn(move || {
            let mut heartbeat = Instant::now();
            let mut ended = None;
            loop {
                if heartbeat.elapsed() >= crate::stop_duration(Duration::from_secs(30)) {
                    if let Err(error) = config.rental.operation("heartbeat") {
                        eprintln!("{error}");
                    }
                    heartbeat = Instant::now();
                }
                let finished = receiver.recv_timeout(Duration::from_millis(200)).is_ok();
                if ended.is_none() && (finished || dir.join("exit.json").exists()) {
                    ended = Some(Instant::now());
                }
                if ended.is_some_and(|at| at.elapsed() >= crate::stop_duration(COLLECTION_GRACE))
                    || ended.is_some() && dir.join("final_collected").exists()
                {
                    break;
                }
            }
            loop {
                match config.rental.operation("release") {
                    Ok(()) => break,
                    Err(error) => eprintln!("{error}"),
                }
                std::thread::sleep(crate::stop_duration(Duration::from_secs(5)));
            }
        });
        Ok(Some(Self {
            finished,
            worker: Some(worker),
        }))
    }
}

impl Drop for Duty {
    fn drop(&mut self) {
        let _ = self.finished.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
