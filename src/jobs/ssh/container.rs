use serde::{Deserialize, Serialize};

use super::{sh_quote, ssh_run, JobState, SshTarget};
use crate::error::{anyhow, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContainerRun {
    pub reference: String,
    pub id: String,
    pub started_at: String,
    pub run_dir: String,
}

#[derive(Deserialize)]
struct Inspection {
    id: String,
    status: String,
    started_at: String,
}

pub fn validate_reference(reference: &str) -> Result<()> {
    if reference.is_empty()
        || !reference.starts_with(|c: char| c.is_ascii_alphanumeric())
        || !reference
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
    {
        return Err(anyhow!(
            "Container must be a Docker name or ID (letters, numbers, '.', '_' or '-')."
        ));
    }
    Ok(())
}

async fn inspection(target: &SshTarget, reference: &str) -> Result<Option<Inspection>> {
    let template = r#"{"id":{{json .Id}},"status":{{json .State.Status}},"started_at":{{json .State.StartedAt}}}"#;
    match ssh_run(
        target,
        &format!(
            "docker container inspect --format {} -- {}",
            sh_quote(template),
            sh_quote(reference)
        ),
        None,
    )
    .await
    {
        Ok(output) => Ok(Some(serde_json::from_str(output.trim())?)),
        Err(error) => {
            // A successful listing distinguishes absence from an unreachable Docker daemon.
            let list = ssh_run(
                target,
                "docker container ls --all --no-trunc --format '{{.ID}} {{.Names}}'",
                None,
            )
            .await?;
            if list.lines().any(|line| {
                line.split_whitespace().any(|value| {
                    value == reference || (value.len() == 64 && value.starts_with(reference))
                })
            }) {
                Err(error)
            } else {
                Ok(None)
            }
        }
    }
}

pub(super) async fn same_target(
    target: &SshTarget,
    reference: &str,
    container: &ContainerRun,
) -> Result<bool> {
    Ok(inspection(target, reference)
        .await?
        .is_some_and(|found| found.id == container.id && found.started_at == container.started_at))
}

pub async fn resolve(target: &SshTarget, reference: &str) -> Result<ContainerRun> {
    validate_reference(reference)?;
    let found = inspection(target, reference)
        .await?
        .ok_or_else(|| anyhow!("Container '{reference}' does not exist on {}.", target.dest))?;
    if found.status != "running" {
        return Err(anyhow!(
            "Container '{reference}' is {}. Start or unpause it before running an experiment.",
            found.status
        ));
    }
    let probe = r#"set -e
[ "$(uname -s)" = Linux ] || { echo 'Container must run Linux' >&2; exit 1; }
command -v tar >/dev/null
if setsid --wait bash -c 'exit 42'; then rc=0; else rc=$?; fi
[ "$rc" = 42 ] || { echo 'Container needs working setsid --wait' >&2; exit 1; }
case "$HOME" in /*) ;; *) echo 'Container needs an absolute HOME' >&2; exit 1;; esac
[ -d "$HOME" ] && [ -w "$HOME" ] || { echo 'Container HOME is not writable' >&2; exit 1; }
printf '%s' "$HOME"
"#;
    let home = ssh_run(
        target,
        &format!(
            "docker exec {} bash -c {}",
            sh_quote(&found.id),
            sh_quote(probe)
        ),
        None,
    )
    .await?;
    if !home.starts_with('/') || home.contains(['\n', '\r', '\0']) {
        return Err(anyhow!("Container returned an invalid home directory."));
    }
    Ok(ContainerRun {
        reference: reference.into(),
        id: found.id,
        started_at: found.started_at,
        run_dir: format!("{}/.orx/runs", home.trim_end_matches('/')),
    })
}

impl ContainerRun {
    pub fn for_run(&self, run_id: &str) -> Self {
        Self {
            run_dir: format!("{}/{run_id}", self.run_dir),
            ..self.clone()
        }
    }

    pub fn exec(&self, script: &str) -> String {
        format!(
            "docker exec -i {} bash -c {}",
            sh_quote(&self.id),
            sh_quote(script)
        )
    }

    pub async fn require_running(&self, target: &SshTarget) -> Result<()> {
        if let Some(reason) = unavailable(target, self).await? {
            return Err(anyhow!("{reason}"));
        }
        Ok(())
    }
}

async fn unavailable(target: &SshTarget, container: &ContainerRun) -> Result<Option<String>> {
    let Some(found) = inspection(target, &container.id).await? else {
        return Ok(Some(format!(
            "Container '{}' was removed; host logs are preserved.",
            container.reference
        )));
    };
    if found.id != container.id || found.started_at != container.started_at {
        return Ok(Some(format!(
            "Container '{}' restarted; the experiment cannot resume.",
            container.reference
        )));
    }
    Ok((found.status != "running")
        .then(|| format!("Container '{}' is {}.", container.reference, found.status)))
}

// Linux process start time prevents a recycled PID from identifying another experiment.
// Minimal containers need no procps; ignore zombies when checking the process group.
pub(super) const PROCESS_HELPERS: &str = r#"
start_time() {
    local stat
    IFS= read -r stat < "/proc/$1/stat" 2>/dev/null || return 1
    stat=${stat##*) }
    local fields
    read -ra fields <<< "$stat"
    printf '%s' "${fields[19]}"
}
group_alive() {
    local file stat fields
    for file in /proc/[0-9]*/stat; do
        IFS= read -r stat < "$file" 2>/dev/null || continue
        stat=${stat##*) }
        read -ra fields <<< "$stat"
        if [ "${fields[2]}" = "$p" ] && [ "${fields[0]}" != Z ] && [ "${fields[0]}" != X ]; then return 0; fi
    done
    return 1
}
"#;

pub(super) fn inner_script(
    container: &ContainerRun,
    exports: &str,
    setup: Option<&str>,
    script: &str,
) -> String {
    let dir = sh_quote(&container.run_dir);
    format!(
        r#"#!/usr/bin/env bash
{PROCESS_HELPERS}
umask 077
cd {dir} || exit 97
printf '%s %s\n' "$$" "$(start_time $$)" > identity.tmp
mv identity.tmp identity
[ ! -f cancel ] || exit 143
(
set -eo pipefail
{exports}
cd {dir}/repo
{setup}
cd {dir}
{script}
)
exit "$?"
"#,
        setup = setup.unwrap_or("")
    )
}

pub(super) async fn upload_script(
    target: &SshTarget,
    container: &ContainerRun,
    script: &str,
) -> Result<()> {
    let dir = sh_quote(&container.run_dir);
    ssh_run(
        target,
        &container.exec(&format!(
            "umask 077; cat > {dir}/run.sh && chmod 600 {dir}/run.sh"
        )),
        Some(script),
    )
    .await?;
    Ok(())
}

pub(super) fn host_script(dir: &str, container: &ContainerRun) -> String {
    let inner = sh_quote(&format!("{}/run.sh", container.run_dir));
    let check = sh_quote("{{.State.StartedAt}} {{.State.Status}}");
    let expected = sh_quote(&format!("{} running", container.started_at));
    let id = sh_quote(&container.id);
    format!("#!/usr/bin/env bash\ncd \"$HOME/{dir}\" || exit 97\nprintf '%s\\n' \"$$\" > pid\n(\nactual=$(docker container inspect --format {check} -- {id}) || exit $?\n[ \"$actual\" = {expected} ] || {{ echo 'Container generation or readiness changed before launch' >&2; exit 97; }}\ndocker exec {id} setsid --wait bash {inner} </dev/null\n) >> log 2>&1\nprintf '%s\\n' \"$?\" > exit_code\n")
}

fn failed(message: String) -> JobState {
    JobState {
        stage: "ERROR".into(),
        message: Some(message),
    }
}

async fn host_state(target: &SshTarget, dir: &str) -> Result<String> {
    ssh_run(target, &format!(r#"d="$HOME/{dir}"; if [ -f "$d/exit_code" ]; then echo "EXIT $(cat "$d/exit_code")"; elif [ -f "$d/pid" ] && kill -0 "$(cat "$d/pid")" 2>/dev/null; then echo RUNNING; else echo DEAD; fi; if [ -f "$d/launch_time" ]; then echo "$(( $(date +%s) - $(cat "$d/launch_time") ))"; else echo 0; fi"#), None).await
}

pub(super) async fn inspect(
    target: &SshTarget,
    dir: &str,
    container: &ContainerRun,
) -> Result<JobState> {
    let host = host_state(target, dir).await?;
    let mut lines = host.lines();
    let state = lines.next().unwrap_or_default();
    if state.starts_with("EXIT ") {
        return Ok(super::parse_job_state(state));
    }
    let age: i64 = lines.next().unwrap_or("0").parse()?;
    if let Some(reason) = unavailable(target, container).await? {
        if reason.ends_with(" is paused.") {
            return Ok(JobState {
                stage: "RUNNING".into(),
                message: Some(format!(
                    "{reason} Unpause it to continue or finish cancellation."
                )),
            });
        }
        return Ok(failed(reason));
    }
    let directory = sh_quote(&container.run_dir);
    let probe = format!(
        r#"{PROCESS_HELPERS}
cd {directory} || exit 1
if [ ! -f identity ]; then echo PENDING; exit 0; fi
read -r p started < identity
current=$(start_time "$p")
if [ -n "$current" ] && [ "$current" != "$started" ]; then echo REUSED
elif group_alive; then echo RUNNING
else echo DEAD; fi
"#
    );
    let inner = ssh_run(target, &container.exec(&probe), None).await?;
    match inner.trim() {
        "RUNNING" if state == "RUNNING" => Ok(JobState {
            stage: "RUNNING".into(),
            message: None,
        }),
        "PENDING" if age < 30 => Ok(JobState {
            stage: "RUNNING".into(),
            message: Some("Waiting for the container launcher.".into()),
        }),
        "RUNNING" | "PENDING" => {
            if inner.trim() == "RUNNING" {
                // The wrapper can start between the first host read and the inner probe.
                let current = host_state(target, dir).await?;
                let state = current.lines().next().unwrap_or_default();
                if state == "RUNNING" || state.starts_with("EXIT ") {
                    return Ok(super::parse_job_state(state));
                }
            }
            cancel(target, container).await?;
            super::cancel_job(target, dir, None).await?;
            Ok(failed("SSH launcher disappeared or did not start within 30 seconds; the container experiment was stopped.".into()))
        }
        "REUSED" => Ok(failed(
            "The container experiment's process identity changed.".into(),
        )),
        "DEAD" => {
            // The host wrapper may still be writing the final exit code.
            let final_state = super::inspect_host_job(target, dir).await?;
            if final_state.stage != "RUNNING" {
                return Ok(final_state);
            }
            Ok(JobState {
                stage: "RUNNING".into(),
                message: Some("Waiting for the launcher to record the exit status.".into()),
            })
        }
        other => Err(anyhow!("Unexpected container process state: {other}")),
    }
}

pub(super) async fn cancel(target: &SshTarget, container: &ContainerRun) -> Result<()> {
    if let Some(reason) = unavailable(target, container).await? {
        if reason.ends_with(" is paused.") {
            return Err(anyhow!("{reason} Unpause it to complete cancellation."));
        }
        return Ok(());
    }
    let directory = sh_quote(&container.run_dir);
    let script = format!(
        r#"{PROCESS_HELPERS}
cd {directory} || exit 1
umask 077
: > cancel
[ -f identity ] || exit 0
read -r p started < identity
case "$p" in ''|*[!0-9]*) exit 1;; esac
[ "$p" -gt 1 ] || exit 1
current=$(start_time "$p")
if [ -n "$current" ] && [ "$current" != "$started" ]; then echo 'Process identity changed; refusing to signal it' >&2; exit 1; fi
if ! group_alive; then exit 0; fi
kill -TERM -- -"$p" 2>/dev/null || true
for ((i=0; i<50; i++)); do group_alive || exit 0; sleep 0.1; done
kill -KILL -- -"$p" 2>/dev/null || true
for ((i=0; i<10; i++)); do group_alive || exit 0; sleep 0.1; done
echo 'Experiment process group is still alive' >&2
exit 1
"#
    );
    ssh_run(target, &container.exec(&script), None).await?;
    Ok(())
}

#[cfg(test)]
mod tests;
