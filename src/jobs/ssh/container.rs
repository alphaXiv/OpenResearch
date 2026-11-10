use serde::{Deserialize, Serialize};

use super::{sh_quote, ssh_run, SshTarget};
use crate::error::{anyhow, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContainerRun {
    pub reference: String,
    pub id: String,
    pub started_at: String,
    pub run_dir: String,
    /// Fields a newer orx wrote, kept so an older supervisor's rewrite doesn't drop them.
    #[serde(flatten)]
    pub unknown: serde_json::Map<String, serde_json::Value>,
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
        unknown: Default::default(),
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

    /// The engine's run.sh for a run in this container: checks the container generation,
    /// copies the inner script and the host's staged source in, then runs it there.
    pub(crate) fn engine_run_sh(
        &self,
        exports: &str,
        script: &str,
        source: Option<&str>,
    ) -> String {
        let dir = sh_quote(&self.run_dir);
        let id = sh_quote(&self.id);
        let check = sh_quote("{{.State.StartedAt}} {{.State.Status}}");
        let expected = sh_quote(&format!("{} running", self.started_at));
        let inner = sh_quote(&inner_script(self, exports, script));
        let write = sh_quote(&format!("umask 077; mkdir -p {dir} && cat > {dir}/run.sh"));
        let mut run_sh = format!(
            "set -e\nactual=$(docker container inspect --format {check} -- {id})\n[ \"$actual\" = {expected} ] || {{ echo 'Container generation or readiness changed before launch' >&2; exit 97; }}\nprintf '%s' {inner} | docker exec -i {id} bash -c {write}\n"
        );
        if let Some(source) = source {
            let extract = sh_quote(&format!(
                "umask 077; cd {dir} && rm -rf repo.part repo && mkdir repo.part && tar -xf - -C repo.part && mv repo.part repo"
            ));
            run_sh.push_str(&format!(
                "docker exec -i {id} bash -c {extract} < {}\n",
                sh_quote(source)
            ));
        }
        run_sh.push_str(&format!(
            "exec docker exec {id} setsid --wait bash {dir}/run.sh </dev/null\n"
        ));
        run_sh
    }

    /// The engine's cancel.sh: signals the in-container process group before the host one.
    pub(crate) fn engine_cancel_sh(&self) -> String {
        let id = sh_quote(&self.id);
        let check = sh_quote("{{.State.StartedAt}} {{.State.Status}}");
        let running = sh_quote(&format!("{} running", self.started_at));
        let paused = sh_quote(&format!("{} paused", self.started_at));
        let stop = self.exec(&cancel_script(&self.run_dir));
        format!(
            "set -e\nfound=$(docker container ls --all --no-trunc --quiet --filter id={id})\n[ -n \"$found\" ] || exit 0\nactual=$(docker container inspect --format {check} -- {id})\ncase \"$actual\" in {running}|{paused}) {stop} ;; *) exit 0 ;; esac\n"
        )
    }
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

pub(super) fn inner_script(container: &ContainerRun, exports: &str, script: &str) -> String {
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
{script}
)
exit "$?"
"#
    )
}

fn cancel_script(run_dir: &str) -> String {
    let directory = sh_quote(run_dir);
    format!(
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
    )
}

#[cfg(test)]
mod tests;
