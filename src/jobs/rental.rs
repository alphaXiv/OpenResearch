use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{anyhow, Result};

pub const HEARTBEAT: Duration = Duration::from_secs(30);
#[cfg(any(unix, test))]
pub const COLLECTION_GRACE: Duration = Duration::from_secs(60);

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rental {
    pub api_url: String,
    pub sandbox_id: String,
    pub token: String,
}

impl std::fmt::Debug for Rental {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rental")
            .field("sandbox_id", &self.sandbox_id)
            .finish_non_exhaustive()
    }
}

impl Rental {
    pub async fn operation(&self, operation: &str) -> Result<()> {
        let response = crate::net::remote_client()
            .timeout(Duration::from_secs(10))
            .build()?
            .post(format!(
                "{}/sandboxes/{}/lease/{operation}",
                self.api_url.trim_end_matches('/'),
                self.sandbox_id
            ))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({}))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(anyhow!(
                "rental {operation} rejected ({})",
                response.status()
            ));
        }
        Ok(())
    }

    pub fn heartbeat(&self) -> Heartbeat {
        let rental = self.clone();
        Heartbeat(tokio::spawn(async move {
            loop {
                if let Err(error) = rental.operation("heartbeat").await {
                    eprintln!("rental heartbeat unavailable: {error}");
                }
                tokio::time::sleep(HEARTBEAT).await;
            }
        }))
    }
}

pub struct Heartbeat(tokio::task::JoinHandle<()>);
impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConfig {
    pub rental: Rental,
    pub directory: String,
}

#[cfg(unix)]
pub async fn supervise(config: &Path) -> Result<()> {
    use std::fs::{File, OpenOptions};
    use std::os::unix::fs::OpenOptionsExt;
    let config: RemoteConfig = serde_json::from_slice(&std::fs::read(config)?)?;
    let directory = std::path::PathBuf::from(config.directory);
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(directory.join("rental-supervisor.lock"))?;
    let mut lock = fd_lock::RwLock::new(lock_file);
    let Ok(_lock) = lock.try_write() else {
        return Ok(());
    };
    // An uncertain launch is never retried: its rental will expire instead.
    let _started = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(directory.join("launch_started"))?;
    config.rental.operation("activate").await?;
    let _heartbeat = config.rental.heartbeat();
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(directory.join("log"))?;
    let code = if directory.join("cancel_requested").exists() {
        143
    } else {
        let mut child = tokio::process::Command::new("bash")
            .arg(directory.join("payload.sh"))
            .current_dir(&directory)
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .process_group(0)
            .spawn()?;
        if let Some(id) = child.id() {
            std::fs::write(directory.join("pid"), id.to_string())?;
        }
        let status = child.wait().await?;
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
    };
    std::fs::write(directory.join("exit_code.tmp"), format!("{code}\n"))?;
    std::fs::rename(directory.join("exit_code.tmp"), directory.join("exit_code"))?;
    File::open(directory.join("log"))?.sync_all()?;
    let deadline = tokio::time::Instant::now() + COLLECTION_GRACE;
    while !directory.join("final_collected").exists() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    drop(_heartbeat);
    loop {
        if config.rental.operation("release").await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[cfg(not(unix))]
pub async fn supervise(_config: &Path) -> Result<()> {
    Err(anyhow!(
        "remote rental supervision requires a Unix experiment machine"
    ))
}

fn remote_bash_command(script: &str) -> String {
    crate::commands::up_remote::remote_login_orx_cmd(&format!(
        "bash -c {}",
        super::ssh::sh_quote(script)
    ))
}

fn remote_supervisor_command(binary: &str, directory: &str) -> String {
    use super::ssh::sh_quote;
    // An asynchronous `cd && nohup` list keeps the SSH output pipes open.
    format!(
        "cd {} || exit; nohup {} rental-supervise {} </dev/null > supervisor.log 2>&1 &",
        sh_quote(directory),
        sh_quote(binary),
        sh_quote(&format!("{directory}/rental.json"))
    )
}

fn cancel_probe_script(directory: &str) -> String {
    format!(
        "d=\"$HOME/{directory}\"; umask 077; mkdir -p \"$d\"; touch \"$d/cancel_requested\"; \
         for ((i=0;i<40;i++)); do \
         if [ -f \"$d/exit_code\" ]; then echo EXIT; exit 0; fi; \
         if [ -f \"$d/pid\" ]; then echo PID; exit 0; fi; sleep 0.25; done; \
         echo 'Waiting for remote supervisor to acknowledge cancellation' >&2; exit 1"
    )
}

pub async fn cancel(target: &super::ssh::SshTarget, directory: &str) -> Result<()> {
    use super::ssh::{sh_quote, ssh_run};
    let script = cancel_probe_script(directory);
    let state = ssh_run(target, &format!("bash -c {}", sh_quote(&script)), None).await?;
    match state.trim() {
        "EXIT" => Ok(()),
        "PID" => super::ssh::cancel_job(target, directory, None).await,
        _ => Err(anyhow!("Remote cancellation has not been acknowledged")),
    }
}

pub async fn launch(
    target: &super::ssh::SshTarget,
    run_id: &str,
    script: &str,
    env: &std::collections::HashMap<String, String>,
    rental: &Rental,
) -> Result<()> {
    use super::ssh::{sh_quote, ssh_run};
    let directory = super::openresearch::run_dir(run_id);
    let installer = crate::commands::up_remote::remote_installer(env!("CARGO_PKG_VERSION"));
    let install = format!(
        "set -eo pipefail; touch \"$HOME/.no_auto_tmux\"; \
         if ! command -v uv >/dev/null; then \
         curl --proto '=https' --tlsv1.2 -LsSf https://astral.sh/uv/0.8.22/install.sh | env UV_NO_MODIFY_PATH=1 sh; fi; \
         if ! command -v orx >/dev/null || ! orx rental-supervise --help >/dev/null 2>&1; then {installer}; fi; command -v orx"
    );
    let output = ssh_run(target, &remote_bash_command(&install), None).await?;
    let binary = output
        .lines()
        .last()
        .filter(|s| s.starts_with('/'))
        .ok_or_else(|| anyhow!("Remote supervisor binary unavailable"))?;
    let home = ssh_run(target, "printf '%s' \"$HOME\"", None).await?;
    let path = format!("{}/{directory}", home.trim().trim_end_matches('/'));
    let config = RemoteConfig {
        rental: rental.clone(),
        directory: path.clone(),
    };
    let setup = format!(
        "umask 077; mkdir -p \"$HOME/{directory}\"; cat > \"$HOME/{directory}/rental.json.tmp\" && mv \"$HOME/{directory}/rental.json.tmp\" \"$HOME/{directory}/rental.json\""
    );
    ssh_run(target, &setup, Some(&serde_json::to_string(&config)?)).await?;
    let mut payload = String::from(
        "#!/usr/bin/env bash\nexport PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:$PATH\"\n[ ! -f cancel_requested ] || exit 143\n",
    );
    for (name, value) in super::default_python_env(env) {
        payload.push_str(&format!("export {name}={}\n", sh_quote(&value)));
    }
    payload.push_str(script);
    ssh_run(
        target,
        &format!("umask 077; cat > \"$HOME/{directory}/payload.sh.tmp\" && mv \"$HOME/{directory}/payload.sh.tmp\" \"$HOME/{directory}/payload.sh\""),
        Some(&payload),
    )
    .await?;
    let command = remote_supervisor_command(binary, &path);
    ssh_run(target, &command, None).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[tokio::test]
    async fn remote_launch_returns_without_waiting_for_the_supervisor() {
        let temp = crate::local::git::TemporaryDirectory::new("orx-rental-detach").unwrap();
        std::fs::write(
            temp.path().join("rental-supervise"),
            "touch started; sleep 2\n",
        )
        .unwrap();
        let command = remote_supervisor_command("/bin/sh", temp.path().to_str().unwrap());
        let output = tokio::time::timeout(
            Duration::from_secs(1),
            tokio::process::Command::new("/bin/sh")
                .args(["-c", &command])
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(output.status.success());
        tokio::time::timeout(Duration::from_secs(1), async {
            while !temp.path().join("started").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn cancellation_waits_for_a_pid_or_terminal_acknowledgment() {
        let temp = crate::local::git::TemporaryDirectory::new("orx-rental-cancel").unwrap();
        let invoke = || {
            std::process::Command::new("bash")
                .args(["-c", &cancel_probe_script("run")])
                .env("HOME", temp.path())
                .output()
                .unwrap()
        };
        let pending = invoke();
        assert!(!pending.status.success());
        assert!(temp.path().join("run/cancel_requested").exists());
        std::fs::write(temp.path().join("run/exit_code"), "143\n").unwrap();
        let acknowledged = invoke();
        assert!(acknowledged.status.success());
        assert_eq!(String::from_utf8_lossy(&acknowledged.stdout).trim(), "EXIT");
    }

    #[cfg(unix)]
    #[test]
    fn rental_bootstrap_runs_bash_syntax_through_the_posix_login_shell() {
        let home = crate::local::git::TemporaryDirectory::new("orx-rental-shell").unwrap();
        let script = remote_bash_command("values=(first second); printf '%s' \"${values[1]}\"");
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .env("HOME", home.path())
            .env("SHELL", "/bin/sh")
            .env("ENV", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), "second");
    }

    #[test]
    fn rental_debug_does_not_expose_capability() {
        let rental = Rental {
            api_url: "https://example.invalid".into(),
            sandbox_id: "rental".into(),
            token: "secret".into(),
        };
        assert!(!format!("{rental:?}").contains("secret"));
        assert_eq!(HEARTBEAT.as_secs(), 30);
        assert_eq!(COLLECTION_GRACE.as_secs(), 60);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn remote_supervisor_releases_after_ack_without_sending_experiment_data() {
        use axum::{
            body::Bytes,
            extract::{Path, State},
            http::StatusCode,
            routing::post,
            Router,
        };
        use std::sync::{Arc, Mutex};
        type Requests = Arc<Mutex<Vec<(String, Vec<u8>)>>>;
        for (exit, acknowledge, cancel_before_spawn) in [
            (0, true, false),
            (7, true, false),
            (0, false, false),
            (143, true, true),
        ] {
            let requests = Arc::new(Mutex::new(Vec::<(String, Vec<u8>)>::new()));
            let temp = crate::local::git::TemporaryDirectory::new("orx-rental-test").unwrap();
            let directory = temp.path().to_path_buf();
            async fn record(
                Path(operation): Path<String>,
                State((requests, directory, cancel_before_spawn)): State<(
                    Requests,
                    std::path::PathBuf,
                    bool,
                )>,
                body: Bytes,
            ) -> StatusCode {
                if operation == "activate" && cancel_before_spawn {
                    std::fs::write(directory.join("cancel_requested"), "").unwrap();
                }
                requests.lock().unwrap().push((operation, body.to_vec()));
                StatusCode::NO_CONTENT
            }
            let app = Router::new()
                .route("/sandboxes/rental/lease/{operation}", post(record))
                .with_state((requests.clone(), directory.clone(), cancel_before_spawn));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });

            std::fs::write(
                directory.join("payload.sh"),
                format!("printf 'private experiment output\n'; exit {exit}\n"),
            )
            .unwrap();
            if acknowledge {
                std::fs::write(directory.join("final_collected"), "").unwrap();
            }
            let config = directory.join("rental.json");
            std::fs::write(
                &config,
                serde_json::to_vec(&RemoteConfig {
                    directory: directory.to_string_lossy().into_owned(),
                    rental: Rental {
                        api_url: format!("http://{addr}"),
                        sandbox_id: "rental".into(),
                        token: "capability".into(),
                    },
                })
                .unwrap(),
            )
            .unwrap();
            let started = tokio::time::Instant::now();
            tokio::time::timeout(
                COLLECTION_GRACE + Duration::from_secs(5),
                supervise(&config),
            )
            .await
            .unwrap()
            .unwrap();
            if !acknowledge {
                assert!(started.elapsed() >= COLLECTION_GRACE);
            }
            assert!(supervise(&config).await.is_err());
            assert_eq!(
                std::fs::read_to_string(directory.join("exit_code"))
                    .unwrap()
                    .trim(),
                exit.to_string()
            );
            let log = std::fs::read_to_string(directory.join("log")).unwrap();
            if cancel_before_spawn {
                assert!(log.is_empty());
                assert!(!directory.join("pid").exists());
            } else {
                assert!(log.contains("private experiment output"));
            }
            let captured = requests.lock().unwrap();
            assert_eq!(captured.first().unwrap().0, "activate");
            assert_eq!(captured.last().unwrap().0, "release");
            assert!(captured.iter().all(|(_, body)| body == b"{}"));
            server.abort();
        }
    }
}
