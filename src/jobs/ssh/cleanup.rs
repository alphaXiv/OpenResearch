//! Removing finished runs' remote dirs and unused cached source tars on SSH and Slurm hosts.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use super::{
    remote_run_dir, remote_source_dir, sh_quote, shell_path, ssh_run, validate_remote_root,
    SshTarget, HOST_PROCESS_HELPERS,
};
use crate::error::{anyhow, Result};
use crate::jobs::BackendDescriptor;

/// Cached tars untouched this long are no longer being staged by any orx install.
pub(crate) const SOURCE_IDLE_MINUTES: i64 = 60;
const HOST_TIMEOUT: Duration = Duration::from_secs(120);

/// A run's remote dir, checked against the layout orx writes before anything removes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRun {
    pub(crate) run_id: String,
    pub(crate) host: String,
    root: Option<String>,
    dir: String,
    /// Docker id and in-container run dir for container runs.
    container: Option<(String, String)>,
    /// Queued by a project or experiment delete; otherwise `orx tensorboard` still reads the
    /// finished run's remote `tensorboard/`, so it stays.
    deleted: bool,
}

impl RemoteRun {
    /// `Ok(None)` when orx owns no remote dir for the run; `deleted` also removes TensorBoard logs.
    pub fn from_descriptor(
        run_id: &str,
        descriptor: &BackendDescriptor,
        deleted: bool,
    ) -> Result<Option<Self>> {
        // A submission that failed before choosing a host left nothing remote.
        let Some(host) = descriptor.namespace.clone().filter(|host| !host.is_empty()) else {
            return Ok(None);
        };
        if !matches!(descriptor.kind.as_str(), "ssh_job" | "slurm_job") {
            return Ok(None);
        }
        if uuid::Uuid::parse_str(run_id).is_err() {
            return Err(anyhow!("Unexpected run id {run_id}"));
        }
        let root = descriptor.remote_root.clone();
        if let Some(root) = &root {
            validate_remote_root(root)?;
        }
        let dir = remote_run_dir(root.as_deref(), run_id);
        // SSH records its dir; a different one was not created by this layout.
        if descriptor.kind == "ssh_job" && descriptor.job_id.as_deref().is_some_and(|d| d != dir) {
            return Err(anyhow!("Run {run_id} has an unexpected remote dir"));
        }
        let container = match &descriptor.ssh_container {
            Some(container)
                if container.run_dir.starts_with('/')
                    && container.run_dir.ends_with(&format!("/.orx/runs/{run_id}")) =>
            {
                Some((container.id.clone(), container.run_dir.clone()))
            }
            Some(_) => return Err(anyhow!("Run {run_id} has an unexpected container dir")),
            None => None,
        };
        Ok(Some(Self {
            run_id: run_id.into(),
            host,
            root,
            dir,
            container,
            deleted,
        }))
    }

    pub(crate) fn root(&self) -> Option<&str> {
        self.root.as_deref()
    }

    pub(crate) fn deleted(&self) -> bool {
        self.deleted
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Removed,
    /// The run's process group is still alive on the host.
    Busy,
    Failed(String),
}

/// One script per host: remove each run dir whose process group is gone, then cached tars
/// under `roots` that no active run references and nothing has touched recently.
fn host_script(
    runs: &[RemoteRun],
    roots: &BTreeSet<Option<String>>,
    keep_digests: &BTreeSet<String>,
) -> String {
    let mut script = format!("{HOST_PROCESS_HELPERS}\n");
    for run in runs {
        // A container that exists but can't run the removal (stopped), or a Docker daemon that
        // can't say, keeps the run queued; only a container Docker reports gone counts as cleaned.
        let container_failed = run.container.as_ref().map_or_else(
            || "false".to_string(),
            |(id, dir)| {
                format!(
                    "{{ if docker container inspect -- {id} </dev/null >/dev/null 2>&1; then \
                     ! cerr=$(docker exec {id} rm -rf -- {dir} </dev/null 2>&1); \
                     elif ! command -v docker >/dev/null 2>&1 || docker info </dev/null >/dev/null 2>&1; then false; \
                     else cerr='Docker is unavailable'; fi; }}",
                    id = sh_quote(id),
                    dir = sh_quote(dir)
                )
            },
        );
        let remove = if !run.deleted {
            "if [ -d \"$d/tensorboard\" ]; then find \"$d\" -mindepth 1 -maxdepth 1 ! -name tensorboard -exec rm -rf -- {} +; else rm -rf -- \"$d\"; fi"
        } else {
            "rm -rf -- \"$d\""
        };
        script.push_str(&format!(
            "d={dir}; p=$(cat \"$d/pid\" 2>/dev/null); case \"$p\" in ''|*[!0-9]*) p=;; esac; \
             if [ -n \"$p\" ] && host_group_alive \"$p\"; then echo 'BUSY {id}'; \
             else cf=; if {container_failed}; then cf=1; fi; \
             if ! err=$({{ {remove}; }} 2>&1); then printf 'FAILED {id} %s\\n' \"$(printf '%s' \"$err\" | head -n 1)\"; \
             elif [ -n \"$cf\" ]; then printf 'FAILED {id} container: %s\\n' \"$(printf '%s' \"$cerr\" | head -n 1)\"; \
             else echo 'REMOVED {id}'; fi; fi\n",
            dir = shell_path(&run.dir),
            id = run.run_id,
        ));
    }
    let keep = keep_digests
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    for root in roots {
        // Only names orx writes (`<sha256>.tar` and its upload temps): a root may hold the user's own tars.
        script.push_str(&format!(
            "s={source}; if [ -d \"$s\" ]; then \
             find \"$s\" -maxdepth 1 -type f -mmin +{SOURCE_IDLE_MINUTES} | while IFS= read -r f; do \
             case \"$f\" in \"$s\"/*) ;; *) continue;; esac; n=${{f##*/}}; b=${{n%%.tar*}}; \
             case \"$b\" in ''|*[!0-9a-f]*) continue;; esac; [ ${{#b}} -eq 64 ] || continue; \
             case \"$n\" in \"$b.tar\"|\"$b.tar.tmp.\"*) ;; *) continue;; esac; \
             case ' {keep} ' in *\" $b \"*) continue;; esac; \
             [ -n \"$(find \"$f\" -mmin +{SOURCE_IDLE_MINUTES})\" ] && rm -f -- \"$f\"; done; fi\n",
            source = shell_path(&remote_source_dir(root.as_deref())),
        ));
    }
    script.push_str("exit 0\n");
    script
}

/// One outcome per run, in `runs` order.
fn parse_outcomes(runs: &[RemoteRun], output: &str) -> Vec<Outcome> {
    let mut reported = BTreeMap::new();
    for line in output.lines() {
        let mut words = line.splitn(3, ' ');
        let (Some(tag), Some(id)) = (words.next(), words.next()) else {
            continue;
        };
        let outcome = match tag {
            "REMOVED" => Outcome::Removed,
            "BUSY" => Outcome::Busy,
            "FAILED" => Outcome::Failed(words.next().unwrap_or_default().into()),
            _ => continue,
        };
        reported.insert(id.to_string(), outcome);
    }
    runs.iter()
        .map(|run| {
            reported
                .remove(&run.run_id)
                .unwrap_or_else(|| Outcome::Failed("no result from the host".into()))
        })
        .collect()
}

/// Removes `runs` (all on `host`) and that host's unreferenced cached tars under `roots`.
pub async fn clean_host(
    host: &str,
    runs: &[RemoteRun],
    roots: &BTreeSet<Option<String>>,
    keep_digests: &BTreeSet<String>,
) -> Result<Vec<Outcome>> {
    // Over stdin: a large batch would overrun Windows' command-line limit.
    let script = host_script(runs, roots, keep_digests);
    let output = tokio::time::timeout(
        HOST_TIMEOUT,
        ssh_run(&SshTarget::alias(host), "bash -s", Some(&script)),
    )
    .await
    .map_err(|_| anyhow!("Timed out cleaning up runs on {host}"))??;
    Ok(parse_outcomes(runs, &output))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: &str = "6f1c8f0e-6b5e-4c41-9a0e-6d3f4c1b2a10";

    fn descriptor(json: &str) -> BackendDescriptor {
        BackendDescriptor::parse(json).unwrap()
    }

    fn cleanable(json: &str) -> Result<Option<RemoteRun>> {
        RemoteRun::from_descriptor(RUN, &descriptor(json), false)
    }

    #[test]
    fn only_orx_layout_dirs_are_cleanable() {
        let ssh = format!(r#"{{"kind":"ssh_job","namespace":"lab","jobId":".orx/runs/{RUN}"}}"#);
        assert_eq!(
            cleanable(&ssh).unwrap().unwrap().dir,
            format!(".orx/runs/{RUN}")
        );
        let rooted = format!(
            r#"{{"kind":"ssh_job","namespace":"lab","jobId":"/scratch/orx/runs/{RUN}","remoteRoot":"/scratch/orx"}}"#
        );
        assert!(cleanable(&rooted).unwrap().is_some());
        let slurm = r#"{"kind":"slurm_job","namespace":"login","remoteRoot":"/scratch/orx"}"#;
        assert_eq!(
            cleanable(slurm).unwrap().unwrap().dir,
            format!("/scratch/orx/runs/{RUN}")
        );
        for json in [
            r#"{"kind":"ssh_job","namespace":"lab","jobId":"/home/me"}"#.to_string(),
            r#"{"kind":"slurm_job","namespace":"login","remoteRoot":"/a b"}"#.to_string(),
            format!(
                r#"{{"kind":"ssh_job","namespace":"lab","jobId":".orx/runs/{RUN}","sshContainer":{{"reference":"c","id":"i","startedAt":"t","runDir":"/root"}}}}"#
            ),
        ] {
            assert!(cleanable(&json).is_err(), "{json}");
        }
        assert!(RemoteRun::from_descriptor("../x", &descriptor(&ssh), false).is_err());
        let unsubmitted = format!(r#"{{"kind":"ssh_job","jobId":".orx/runs/{RUN}"}}"#);
        assert!(cleanable(&unsubmitted).unwrap().is_none());
        for kind in ["openresearch_job", "k8s_job", "local_job"] {
            assert!(
                cleanable(&format!(r#"{{"kind":"{kind}","namespace":"x"}}"#))
                    .unwrap()
                    .is_none()
            );
        }
    }

    fn outcomes_by_id(runs: &[RemoteRun], output: &str) -> BTreeMap<String, Outcome> {
        runs.iter()
            .map(|run| run.run_id.clone())
            .zip(parse_outcomes(runs, output))
            .collect()
    }

    fn run(id: &str, root: Option<&str>, deleted: bool) -> RemoteRun {
        RemoteRun {
            run_id: id.into(),
            host: "lab".into(),
            root: root.map(Into::into),
            dir: remote_run_dir(root, id),
            container: None,
            deleted,
        }
    }

    #[cfg(unix)]
    #[test]
    fn host_script_removes_finished_dirs_and_stale_orx_tars_only() {
        use std::os::unix::process::CommandExt as _;
        let home = crate::local::git::TemporaryDirectory::new("orx-cleanup-home").unwrap();
        let runs = home.path().join(".orx/runs");
        let source = home.path().join(".orx/source");
        for id in ["done", "busy", "charted", "deleted"] {
            std::fs::create_dir_all(runs.join(id).join("repo")).unwrap();
        }
        for id in ["charted", "deleted"] {
            std::fs::create_dir_all(runs.join(id).join("tensorboard")).unwrap();
        }
        let mut sleeper = std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        std::fs::write(runs.join("busy/pid"), sleeper.id().to_string()).unwrap();
        std::fs::write(runs.join("done/pid"), "999999").unwrap();
        std::fs::create_dir_all(&source).unwrap();
        let (old, kept, fresh) = ("a".repeat(64), "b".repeat(64), "c".repeat(64));
        let names = [
            format!("{old}.tar"),
            format!("{old}.tar.tmp.12"),
            format!("{kept}.tar"),
            format!("{fresh}.tar"),
            "dataset.tar".to_string(),
            format!("{old}.tar.bak"),
        ];
        for name in &names {
            std::fs::write(source.join(name), "x").unwrap();
            if !name.starts_with(&fresh) {
                assert!(std::process::Command::new("touch")
                    .args(["-t", "202001010000"])
                    .arg(source.join(name))
                    .status()
                    .unwrap()
                    .success());
            }
        }
        let batch = [
            run("done", None, false),
            run("busy", None, false),
            run("never", None, false),
            run("charted", None, false),
            run("deleted", None, true),
        ];
        let out = std::process::Command::new("bash")
            .args([
                "-c",
                &host_script(
                    &batch,
                    &BTreeSet::from([None]),
                    &BTreeSet::from([kept.clone()]),
                ),
            ])
            .env("HOME", home.path())
            .output()
            .unwrap();
        let _ = sleeper.kill();
        let _ = sleeper.wait();
        assert!(out.status.success());
        let outcomes = outcomes_by_id(&batch, &String::from_utf8(out.stdout).unwrap());
        for id in ["done", "never", "charted", "deleted"] {
            assert_eq!(outcomes[id], Outcome::Removed, "{id}");
        }
        assert_eq!(outcomes["busy"], Outcome::Busy);
        assert!(!runs.join("done").exists());
        assert!(runs.join("busy/repo").exists());
        assert!(runs.join("charted/tensorboard").is_dir());
        assert!(!runs.join("charted/repo").exists());
        assert!(!runs.join("deleted").exists());
        let mut left: Vec<_> = std::fs::read_dir(&source)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        let mut expected = vec![
            format!("{kept}.tar"),
            format!("{fresh}.tar"),
            "dataset.tar".to_string(),
            format!("{old}.tar.bak"),
        ];
        expected.sort();
        assert_eq!(left, expected);
    }

    #[cfg(unix)]
    #[test]
    fn container_runs_stay_queued_until_docker_reports_the_container_gone() {
        use std::os::unix::fs::PermissionsExt as _;
        // (inspect exit, info exit, expected outcome): stopped, removed, daemon down.
        for (inspect, info, expected) in [
            (
                0,
                0,
                Outcome::Failed("container: container is not running".into()),
            ),
            (1, 0, Outcome::Removed),
            (
                1,
                1,
                Outcome::Failed("container: Docker is unavailable".into()),
            ),
        ] {
            let home = crate::local::git::TemporaryDirectory::new("orx-cleanup-container").unwrap();
            let bin = home.path().join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            let docker = bin.join("docker");
            std::fs::write(
                &docker,
                format!(
                    "#!/bin/sh\ncase \"$1\" in container) exit {inspect};; info) exit {info};; esac\n\
                     echo 'container is not running' >&2\nexit 1\n"
                ),
            )
            .unwrap();
            std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755)).unwrap();
            let mut container = run("c", None, false);
            container.container = Some(("abc".into(), "/root/.orx/runs/c".into()));
            let dir = home.path().join(".orx/runs/c");
            std::fs::create_dir_all(dir.join("repo")).unwrap();
            let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
            let batch = std::slice::from_ref(&container);
            let out = std::process::Command::new("bash")
                .args([
                    "-c",
                    &host_script(batch, &BTreeSet::new(), &BTreeSet::new()),
                ])
                .env("HOME", home.path())
                .env("PATH", path)
                .output()
                .unwrap();
            let outcomes = outcomes_by_id(batch, &String::from_utf8(out.stdout).unwrap());
            assert_eq!(outcomes["c"], expected, "inspect {inspect} info {info}");
            // The host copy goes either way; only the container's copy keeps the run queued.
            assert!(!dir.exists());
        }
    }

    #[test]
    fn host_script_quotes_rooted_and_container_paths() {
        let mut rooted = run(RUN, Some("/scratch/orx"), true);
        rooted.container = Some(("abc".into(), format!("/root/.orx/runs/{RUN}")));
        let script = host_script(
            &[rooted],
            &BTreeSet::from([Some("/scratch/orx".to_string())]),
            &BTreeSet::new(),
        );
        assert!(script.contains(&format!("d='/scratch/orx/runs/{RUN}';")));
        assert!(script.contains(&format!(
            "docker exec 'abc' rm -rf -- '/root/.orx/runs/{RUN}'"
        )));
        assert!(script.contains("s='/scratch/orx/source';"));
        let batch = [run(RUN, None, false), run("other", None, false)];
        let outcomes = outcomes_by_id(&batch, &format!("FAILED {RUN} rm: Permission denied\n"));
        assert_eq!(
            outcomes[RUN],
            Outcome::Failed("rm: Permission denied".into())
        );
        assert!(matches!(outcomes["other"], Outcome::Failed(_)));
    }
}
