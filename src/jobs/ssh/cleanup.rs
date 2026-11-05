//! Removing finished runs' remote dirs and unused cached source tars on SSH and Slurm hosts.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use super::{
    remote_run_dir, remote_source_dir, sh_quote, shell_path, ssh_run, validate_remote_root,
    SshTarget, HOST_PROCESS_HELPERS,
};
use crate::engine::handle::Handle;
use crate::error::{anyhow, Result};
use crate::jobs::BackendDescriptor;

/// Cached tars untouched this long are no longer being staged by any orx install.
pub(crate) const SOURCE_IDLE_MINUTES: i64 = 60;
const HOST_TIMEOUT: Duration = Duration::from_secs(120);

/// A storage root whose source cache a pass sweeps, and whether it is an engine root.
pub(crate) type SourceCache = (Option<String>, bool);

/// A run's remote dir, checked against the layout orx writes before anything removes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRun {
    pub(crate) run_id: String,
    pub(crate) host: String,
    pub(crate) slurm: bool,
    root: Option<String>,
    /// Engine runs live under `<root>/runs` beside `orx-host`'s `<root>/sources`.
    engine: bool,
    /// An engine Slurm run's job, whose end vouches for a wrapper on another node.
    slurm_job: Option<String>,
    dir: String,
    /// Docker id and in-container run dir for container runs.
    container: Option<(String, String)>,
    /// Queued by a project or experiment delete; otherwise `orx tensorboard` still reads the
    /// finished run's remote `tensorboard/`, so it stays.
    deleted: bool,
}

impl RemoteRun {
    /// `Ok(None)` when orx owns no remote dir for the run; `deleted` also removes TensorBoard logs.
    /// An engine run's container comes from its launch intent as `sshContainer`.
    pub fn from_backend(run_id: &str, backend_json: &str, deleted: bool) -> Result<Option<Self>> {
        let descriptor = BackendDescriptor::parse(backend_json)?;
        if uuid::Uuid::parse_str(run_id).is_err() {
            return Err(anyhow!("Unexpected run id {run_id}"));
        }
        let mut slurm_job = None;
        let (host, slurm, root, dir, engine) = if descriptor.kind == "engine" {
            let value: serde_json::Value = serde_json::from_str(backend_json)?;
            let handle = value
                .get("handle")
                .map(|handle| serde_json::from_value(handle.clone()))
                .transpose()?;
            let (host, root, slurm) = match handle {
                Some(Handle::Ssh { host, root }) => (host, root, false),
                Some(Handle::Slurm { host, root, job }) => {
                    if let Some(job) = &job {
                        if job.is_empty() || !job.bytes().all(|b| b.is_ascii_digit() || b == b'_') {
                            return Err(anyhow!("Run {run_id} has an unexpected Slurm job {job}"));
                        }
                    }
                    slurm_job = job;
                    (host, root, true)
                }
                _ => return Ok(None),
            };
            let root = match root.strip_prefix("~/") {
                Some(relative) => relative.to_owned(),
                None => {
                    validate_remote_root(&root)?;
                    root
                }
            };
            let dir = format!("{root}/runs/{run_id}");
            (host, slurm, Some(root), dir, true)
        } else {
            // A submission that failed before choosing a host left nothing remote.
            let Some(host) = descriptor.namespace.clone().filter(|host| !host.is_empty()) else {
                return Ok(None);
            };
            if !matches!(descriptor.kind.as_str(), "ssh_job" | "slurm_job") {
                return Ok(None);
            }
            let root = descriptor.remote_root.clone();
            if let Some(root) = &root {
                validate_remote_root(root)?;
            }
            let dir = remote_run_dir(root.as_deref(), run_id);
            // SSH records its dir; a different one was not created by this layout.
            if descriptor.kind == "ssh_job"
                && descriptor.job_id.as_deref().is_some_and(|d| d != dir)
            {
                return Err(anyhow!("Run {run_id} has an unexpected remote dir"));
            }
            (host, descriptor.kind == "slurm_job", root, dir, false)
        };
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
            slurm,
            root,
            engine,
            slurm_job,
            dir,
            container,
            deleted,
        }))
    }

    pub(crate) fn cache(&self) -> SourceCache {
        (self.root.clone(), self.engine)
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

/// One script per host: remove each run dir whose process group (an engine run's wrapper) is
/// gone, then cached sources under `roots` that no active run references and nothing has
/// touched recently. Engine sources are `orx-host` blobs no run dir still links.
fn host_script(
    runs: &[RemoteRun],
    roots: &BTreeSet<SourceCache>,
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
        // `v` is `busy`, or why the run cannot be shown to have ended.
        let liveness = if run.engine {
            match &run.slurm_job {
                Some(job) => format!(
                    "p=; v=; q=$(squeue -h -j {job} -o %T 2>&1); s=$?; \
                     if [ $s -eq 0 ] && [ -n \"$q\" ]; then v=busy; \
                     elif [ $s -ne 0 ] && ! printf '%s' \"$q\" | grep -q 'Invalid job id'; then \
                     v=\"cannot confirm Slurm job {job} ended: $(printf '%s' \"$q\" | head -n 1)\"; fi"
                ),
                None => "p=; v=; if [ -z \"$ORX_HOST_BINARY\" ]; then v='host binary unavailable'; \
                         elif ! v=$(\"$ORX_HOST_BINARY\" check-run \"$d\" 2>&1); then v=${v:-'host liveness check failed'}; \
                         elif [ \"$v\" = idle ]; then v=; \
                         elif [ \"$v\" != busy ]; then v='invalid host liveness response'; fi".into(),
            }
        } else {
            "v=; p=$(cat \"$d/pid\" 2>/dev/null)".into()
        };
        script.push_str(&format!(
            "d={dir}; {liveness}; case \"$p\" in ''|*[!0-9]*) p=;; esac; \
             if [ \"$v\" = busy ] || {{ [ -n \"$p\" ] && host_group_alive \"$p\"; }}; then echo 'BUSY {id}'; \
             elif [ -n \"$v\" ]; then printf 'FAILED {id} %s\\n' \"$v\"; \
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
    for (root, engine) in roots {
        if *engine {
            let sources = format!("{}/sources", root.as_deref().unwrap_or_default());
            script.push_str(&format!(
                "s={sources}; if [ -d \"$s\" ]; then \
                 find \"$s\" -maxdepth 1 -type f -links 1 -mmin +{SOURCE_IDLE_MINUTES} | while IFS= read -r f; do \
                 n=${{f##*/}}; case \"$n\" in *[!0-9a-f]*) continue;; esac; [ ${{#n}} -eq 64 ] || continue; \
                 case ' {keep} ' in *\" $n \"*) continue;; esac; rm -f -- \"$f\"; done; fi\n",
                sources = shell_path(&sources),
            ));
            continue;
        }
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

/// Removes `runs` (all on `target`) and that host's unreferenced cached sources under `roots`.
pub async fn clean_host(
    target: &SshTarget,
    runs: &[RemoteRun],
    roots: &BTreeSet<SourceCache>,
    keep_digests: &BTreeSet<String>,
) -> Result<Vec<Outcome>> {
    // Over stdin: a large batch would overrun Windows' command-line limit.
    let script = host_script(runs, roots, keep_digests);
    let host = &target.dest;
    let output = tokio::time::timeout(HOST_TIMEOUT, ssh_run(target, "bash -s", Some(&script)))
        .await
        .map_err(|_| anyhow!("Timed out cleaning up runs on {host}"))??;
    Ok(parse_outcomes(runs, &output))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: &str = "6f1c8f0e-6b5e-4c41-9a0e-6d3f4c1b2a10";

    fn cleanable(json: &str) -> Result<Option<RemoteRun>> {
        RemoteRun::from_backend(RUN, json, false)
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
        assert!(RemoteRun::from_backend("../x", &ssh, false).is_err());
        let unsubmitted = format!(r#"{{"kind":"ssh_job","jobId":".orx/runs/{RUN}"}}"#);
        assert!(cleanable(&unsubmitted).unwrap().is_none());
        for kind in ["openresearch_job", "k8s_job", "local_job"] {
            assert!(
                cleanable(&format!(r#"{{"kind":"{kind}","namespace":"x"}}"#))
                    .unwrap()
                    .is_none()
            );
        }

        let engine = |handle: &str| format!(r#"{{"kind":"engine","handle":{handle}}}"#);
        let home = cleanable(&engine(
            r#"{"type":"Ssh","host":"lab","root":"~/.orx/engine"}"#,
        ))
        .unwrap()
        .unwrap();
        assert_eq!(home.dir, format!(".orx/engine/runs/{RUN}"));
        assert_eq!(home.cache(), (Some(".orx/engine".into()), true));
        let slurm = cleanable(&engine(
            r#"{"type":"Slurm","host":"login","root":"/scratch/orx/engine","job":"7"}"#,
        ))
        .unwrap()
        .unwrap();
        assert!(slurm.slurm);
        assert_eq!(slurm.slurm_job.as_deref(), Some("7"));
        assert_eq!(slurm.dir, format!("/scratch/orx/engine/runs/{RUN}"));
        assert!(cleanable(&engine(r#"{"type":"Ssh","host":"lab","root":"/a b"}"#)).is_err());
        assert!(cleanable(&engine(
            r#"{"type":"Slurm","host":"login","root":"/scratch/orx/engine","job":"7;rm"}"#
        ))
        .is_err());
        assert!(cleanable(&engine(r#"{"type":"Local","dir":"/x"}"#))
            .unwrap()
            .is_none());
        assert!(cleanable(r#"{"kind":"engine"}"#).unwrap().is_none());
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
            slurm: false,
            root: root.map(Into::into),
            engine: false,
            slurm_job: None,
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
        let mut sleeper = crate::process::command("sleep")
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
                assert!(crate::process::command("touch")
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
        let out = crate::process::command("bash")
            .args([
                "-c",
                &host_script(
                    &batch,
                    &BTreeSet::from([(None, false)]),
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

    /// An engine run is busy while its wrapper lives on this node or its Slurm job is queued,
    /// and stays when neither can be confirmed; a blob goes once no run dir links it and it
    /// has sat idle.
    #[cfg(unix)]
    #[test]
    fn engine_dirs_wait_for_their_wrapper_or_slurm_job_and_unlinked_blobs_are_swept() {
        use std::os::unix::process::CommandExt as _;
        let home = crate::local::git::TemporaryDirectory::new("orx-cleanup-engine").unwrap();
        let root = home.path().join(".orx/engine");
        let sources = root.join("sources");
        std::fs::create_dir_all(&sources).unwrap();
        let mut wrapper = crate::process::command("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let node = orx_host::platform::node().unwrap();
        let (linked, unlinked, fresh, kept) = (
            "a".repeat(64),
            "b".repeat(64),
            "c".repeat(64),
            "d".repeat(64),
        );
        for name in [&linked, &unlinked, &fresh, &kept] {
            std::fs::write(sources.join(name), "x").unwrap();
            if name != &fresh {
                assert!(crate::process::command("touch")
                    .args(["-t", "202001010000"])
                    .arg(sources.join(name))
                    .status()
                    .unwrap()
                    .success());
            }
        }
        // Slurm job 41 is still running, 42 has left the queue, and 43 cannot be asked about.
        let bin = home.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let squeue = bin.join("squeue");
        std::fs::write(
            &squeue,
            "#!/bin/sh\ncase \"$3\" in 41) echo RUNNING;; 42) echo 'slurm_load_jobs error: Invalid job id specified' >&2; exit 1;; *) echo 'Unable to contact slurm controller' >&2; exit 1;; esac\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&squeue, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        for (id, pid, node) in [
            ("busy", wrapper.id(), node.as_str()),
            ("done", 999_999, node.as_str()),
            ("elsewhere", wrapper.id(), "compute-7"),
            ("queued", wrapper.id(), "compute-7"),
            ("finished", wrapper.id(), "compute-7"),
            ("unknown", wrapper.id(), "compute-7"),
        ] {
            let dir = root.join("runs").join(id);
            std::fs::create_dir_all(dir.join("repo")).unwrap();
            let mut identity = orx_host::platform::identity(wrapper.id()).unwrap();
            identity.node = node.into();
            identity.pid = pid;
            if id == "done" {
                identity.boot_id.push_str("-rebooted");
            }
            orx_host::publish(&dir.join("wrapper.json"), &identity).unwrap();
        }
        std::fs::hard_link(sources.join(&linked), root.join("runs/busy/source.tar")).unwrap();
        let engine_run = |id: &str| RemoteRun {
            engine: true,
            root: Some(".orx/engine".into()),
            dir: format!(".orx/engine/runs/{id}"),
            ..run(id, None, true)
        };
        let slurm_run = |id: &str, job: &str| RemoteRun {
            slurm: true,
            slurm_job: Some(job.into()),
            ..engine_run(id)
        };
        let batch = [
            engine_run("busy"),
            engine_run("done"),
            engine_run("elsewhere"),
            slurm_run("queued", "41"),
            slurm_run("finished", "42"),
            slurm_run("unknown", "43"),
        ];
        let out = crate::process::command("bash")
            .args([
                "-c",
                &host_script(
                    &batch,
                    &BTreeSet::from([(Some(".orx/engine".into()), true)]),
                    &BTreeSet::from([kept.clone()]),
                ),
            ])
            .env("HOME", home.path())
            .env(
                "ORX_HOST_BINARY",
                orx_test_support::host_binaries().join(format!(
                    "orx-host-{}-{}",
                    if cfg!(target_os = "macos") {
                        "darwin"
                    } else {
                        "linux"
                    },
                    std::env::consts::ARCH
                )),
            )
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .output()
            .unwrap();
        let _ = wrapper.kill();
        let _ = wrapper.wait();
        let outcomes = outcomes_by_id(&batch, &String::from_utf8(out.stdout).unwrap());
        assert_eq!(outcomes["busy"], Outcome::Busy);
        assert_eq!(outcomes["done"], Outcome::Removed);
        assert_eq!(
            outcomes["elsewhere"],
            Outcome::Failed("orx-host: wrapper belongs to another node".into())
        );
        assert_eq!(outcomes["queued"], Outcome::Busy);
        assert_eq!(outcomes["finished"], Outcome::Removed);
        assert_eq!(
            outcomes["unknown"],
            Outcome::Failed(
                "cannot confirm Slurm job 43 ended: Unable to contact slurm controller".into()
            )
        );
        for (id, kept) in [("elsewhere", true), ("queued", true), ("finished", false)] {
            assert_eq!(root.join("runs").join(id).exists(), kept, "{id}");
        }
        let mut left: Vec<_> = std::fs::read_dir(&sources)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, [linked, fresh, kept]);
    }

    #[cfg(unix)]
    #[test]
    fn engine_retention_keeps_directories_when_the_host_check_fails() {
        let home = crate::local::git::TemporaryDirectory::new("orx-cleanup-proof").unwrap();
        let dir = home.path().join("engine/runs/broken");
        std::fs::create_dir_all(dir.join("repo")).unwrap();
        std::fs::write(dir.join("wrapper.json"), "{").unwrap();
        let batch = [RemoteRun {
            engine: true,
            dir: dir.to_string_lossy().into_owned(),
            ..run("broken", None, true)
        }];
        let native = orx_test_support::host_binaries().join(format!(
            "orx-host-{}-{}",
            if cfg!(target_os = "macos") {
                "darwin"
            } else {
                "linux"
            },
            std::env::consts::ARCH
        ));
        for binary in [std::path::Path::new("/usr/bin/false"), &native] {
            let output = crate::process::command("bash")
                .args([
                    "-c",
                    &host_script(&batch, &BTreeSet::new(), &BTreeSet::new()),
                ])
                .env("ORX_HOST_BINARY", binary)
                .output()
                .unwrap();
            assert!(
                matches!(&parse_outcomes(&batch, &String::from_utf8(output.stdout).unwrap())[0], Outcome::Failed(detail) if !detail.is_empty())
            );
            assert!(dir.join("repo").is_dir());
        }
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
            let out = crate::process::command("bash")
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
            &BTreeSet::from([(Some("/scratch/orx".to_string()), false)]),
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
