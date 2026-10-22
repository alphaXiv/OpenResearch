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
const SOURCE_IDLE_MINUTES: u32 = 60;
const HOST_TIMEOUT: Duration = Duration::from_secs(120);

/// A run's remote dir, checked against the layout orx writes before anything removes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRun {
    pub run_id: String,
    pub host: String,
    pub root: Option<String>,
    dir: String,
    /// Docker id and in-container run dir for container runs.
    container: Option<(String, String)>,
}

impl RemoteRun {
    /// `Ok(None)` for backends whose remote state orx does not own.
    pub fn from_descriptor(run_id: &str, descriptor: &BackendDescriptor) -> Result<Option<Self>> {
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
        let host = descriptor
            .namespace
            .clone()
            .filter(|host| !host.is_empty())
            .ok_or_else(|| anyhow!("Run {run_id} has no remote host"))?;
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
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Removed,
    /// The run's process group is still alive on the host.
    Busy,
    Failed,
}

/// One script per host: remove each run dir whose process group is gone, then cached tars
/// that no active run references and nothing has touched recently.
fn host_script(runs: &[RemoteRun], keep_digests: &BTreeSet<String>) -> String {
    let mut script = format!("{HOST_PROCESS_HELPERS}\n");
    for run in runs {
        let inner = run
            .container
            .as_ref()
            .map_or_else(String::new, |(id, dir)| {
                format!(
                    "docker exec {} rm -rf -- {} >/dev/null 2>&1; ",
                    sh_quote(id),
                    sh_quote(dir)
                )
            });
        script.push_str(&format!(
            "d={dir}; p=$(cat \"$d/pid\" 2>/dev/null); case \"$p\" in ''|*[!0-9]*) p=;; esac; \
             if [ -n \"$p\" ] && host_group_alive \"$p\"; then echo 'BUSY {id}'; \
             else {inner}rm -rf -- \"$d\" && echo 'REMOVED {id}'; fi\n",
            dir = shell_path(&run.dir),
            id = run.run_id,
        ));
    }
    let keep = keep_digests
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let roots: BTreeSet<_> = runs.iter().map(|run| run.root.as_deref()).collect();
    for root in roots {
        script.push_str(&format!(
            "s={source}; if [ -d \"$s\" ]; then \
             find \"$s\" -maxdepth 1 -type f \\( -name '*.tar' -o -name '*.tar.tmp.*' \\) -mmin +{SOURCE_IDLE_MINUTES} \
             | while IFS= read -r f; do n=${{f##*/}}; case ' {keep} ' in *\" ${{n%%.tar*}} \"*) ;; *) rm -f -- \"$f\";; esac; done; fi\n",
            source = shell_path(&remote_source_dir(root)),
        ));
    }
    script.push_str("exit 0\n");
    script
}

fn parse_outcomes(runs: &[RemoteRun], output: &str) -> BTreeMap<String, Outcome> {
    runs.iter()
        .map(|run| {
            let outcome = if output
                .lines()
                .any(|line| line == format!("REMOVED {}", run.run_id))
            {
                Outcome::Removed
            } else if output
                .lines()
                .any(|line| line == format!("BUSY {}", run.run_id))
            {
                Outcome::Busy
            } else {
                Outcome::Failed
            };
            (run.run_id.clone(), outcome)
        })
        .collect()
}

/// Removes `runs` (all on `host`) and that host's unreferenced cached tars.
pub async fn clean_host(
    host: &str,
    runs: &[RemoteRun],
    keep_digests: &BTreeSet<String>,
) -> Result<BTreeMap<String, Outcome>> {
    let script = host_script(runs, keep_digests);
    let command = format!("bash -c {}", sh_quote(&script));
    let output = tokio::time::timeout(
        HOST_TIMEOUT,
        ssh_run(&SshTarget::alias(host), &command, None),
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

    #[test]
    fn only_orx_layout_dirs_are_cleanable() {
        let ssh = descriptor(&format!(
            r#"{{"kind":"ssh_job","namespace":"lab","jobId":".orx/runs/{RUN}"}}"#
        ));
        let run = RemoteRun::from_descriptor(RUN, &ssh).unwrap().unwrap();
        assert_eq!(run.dir, format!(".orx/runs/{RUN}"));
        let rooted = descriptor(&format!(
            r#"{{"kind":"ssh_job","namespace":"lab","jobId":"/scratch/orx/runs/{RUN}","remoteRoot":"/scratch/orx"}}"#
        ));
        assert!(RemoteRun::from_descriptor(RUN, &rooted).unwrap().is_some());
        let slurm =
            descriptor(r#"{"kind":"slurm_job","namespace":"login","remoteRoot":"/scratch/orx"}"#);
        assert_eq!(
            RemoteRun::from_descriptor(RUN, &slurm)
                .unwrap()
                .unwrap()
                .dir,
            format!("/scratch/orx/runs/{RUN}")
        );
        for json in [
            r#"{"kind":"ssh_job","namespace":"lab","jobId":"/home/me"}"#.to_string(),
            r#"{"kind":"slurm_job","namespace":"login","remoteRoot":"/a b"}"#.to_string(),
            format!(r#"{{"kind":"ssh_job","jobId":".orx/runs/{RUN}"}}"#),
            format!(
                r#"{{"kind":"ssh_job","namespace":"lab","jobId":".orx/runs/{RUN}","sshContainer":{{"reference":"c","id":"i","startedAt":"t","runDir":"/root"}}}}"#
            ),
        ] {
            assert!(
                RemoteRun::from_descriptor(RUN, &descriptor(&json)).is_err(),
                "{json}"
            );
        }
        assert!(RemoteRun::from_descriptor("../x", &ssh).is_err());
        for kind in ["openresearch_job", "k8s_job", "local_job"] {
            let other = descriptor(&format!(r#"{{"kind":"{kind}","namespace":"x"}}"#));
            assert!(RemoteRun::from_descriptor(RUN, &other).unwrap().is_none());
        }
    }

    fn run(id: &str, root: Option<&str>) -> RemoteRun {
        RemoteRun {
            run_id: id.into(),
            host: "lab".into(),
            root: root.map(Into::into),
            dir: remote_run_dir(root, id),
            container: None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn host_script_removes_finished_dirs_and_stale_unreferenced_tars() {
        use std::os::unix::process::CommandExt as _;
        let home = crate::local::git::TemporaryDirectory::new("orx-cleanup-home").unwrap();
        let runs = home.path().join(".orx/runs");
        let source = home.path().join(".orx/source");
        for id in ["done", "busy"] {
            std::fs::create_dir_all(runs.join(id).join("repo")).unwrap();
        }
        let mut sleeper = std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        std::fs::write(runs.join("busy/pid"), sleeper.id().to_string()).unwrap();
        std::fs::write(runs.join("done/pid"), "999999").unwrap();
        std::fs::create_dir_all(&source).unwrap();
        for name in ["old.tar", "kept.tar", "fresh.tar", "old.tar.tmp.12"] {
            std::fs::write(source.join(name), "x").unwrap();
        }
        for name in ["old.tar", "kept.tar", "old.tar.tmp.12"] {
            assert!(std::process::Command::new("touch")
                .args(["-t", "202001010000"])
                .arg(source.join(name))
                .status()
                .unwrap()
                .success());
        }
        let batch = [run("done", None), run("busy", None), run("never", None)];
        let out = std::process::Command::new("bash")
            .args(["-c", &host_script(&batch, &BTreeSet::from(["kept".into()]))])
            .env("HOME", home.path())
            .output()
            .unwrap();
        let _ = sleeper.kill();
        let _ = sleeper.wait();
        assert!(out.status.success());
        let outcomes = parse_outcomes(&batch, &String::from_utf8(out.stdout).unwrap());
        assert_eq!(outcomes["done"], Outcome::Removed);
        assert_eq!(outcomes["busy"], Outcome::Busy);
        // An already-missing dir counts as removed.
        assert_eq!(outcomes["never"], Outcome::Removed);
        assert!(!runs.join("done").exists());
        assert!(runs.join("busy/repo").exists());
        let mut left: Vec<_> = std::fs::read_dir(&source)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["fresh.tar", "kept.tar"]);
    }

    #[test]
    fn host_script_quotes_rooted_and_container_paths() {
        let mut rooted = run(RUN, Some("/scratch/orx"));
        rooted.container = Some(("abc".into(), format!("/root/.orx/runs/{RUN}")));
        let script = host_script(&[rooted], &BTreeSet::new());
        assert!(script.contains(&format!("d='/scratch/orx/runs/{RUN}';")));
        assert!(script.contains(&format!(
            "docker exec 'abc' rm -rf -- '/root/.orx/runs/{RUN}'"
        )));
        assert!(script.contains("s='/scratch/orx/source';"));
        assert!(parse_outcomes(&[run(RUN, None)], "REMOVED other\n")[RUN] == Outcome::Failed);
    }
}
