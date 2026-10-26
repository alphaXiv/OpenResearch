# SSH (`--backend ssh`)

Use this backend when the user asks to run on their own server, or when SSH is
the configured compute default. Authentication uses SSH config, keys and the
agent; `orx` never reads private keys.

On Unix, `orx compute connect ssh --host lab` authenticates once for background
commands. Its master stays available for 24 hours of idle time and sends
keepalives every 30 seconds. Use `--persist <seconds>` to set a new master's idle
lifetime (`0` keeps it indefinitely); an existing master retains its settings.
Reconnect to recover an expired login.
On Unix with OpenSSH 8.4 or newer, successful connections automatically install
`Include openresearch_config` in `~/.ssh/config`. Agents and ordinary `ssh`,
`scp`, and `rsync` commands reuse a live connection for the same user, host,
and port of a connected alias. Configured `ProxyJump` and `ProxyCommand` routes
remain separate. Existing user `ControlPath` settings, including `Match final`,
take precedence; the managed Include appears after your settings.
Symlinked configs require adding the reported Include manually.
OpenResearch-launched agents select their instance's connection directory;
external terminals use the default configuration unless they export
`ORX_SSH_CONTROL_DIR` to the instance's directory. After upgrading from older
socket naming, reconnect once; existing masters are not terminated.
Changing a configured route requires reconnecting before ordinary commands can
reuse it. For command-line proxy overrides, use a separate configured alias or
`-S none`; native SSH does not expose those overrides to the sharing predicate.
Experiment monitors share one persistent SSH channel per resolved host and authentication
context. Windows Connect keeps the authenticated SSH process alive after its dialog closes.
OpenSSH supplies the protocol keepalives; there is no separate periodic helper
heartbeat. Helper liveness is checked when monitoring requests run.
Background monitoring never starts a fresh login on user hosts. Transport failures
back off together (5 seconds up to 5 minutes); restarting OpenResearch or adding
runs does not reset the retry schedule. Remote command errors do not reset the connection.
Monitoring outages are reported after failures persist for at least 60 seconds,
at a monitoring check, without declaring the remote job stopped.
Reconnecting resumes status and log polling.

```sh
orx exp run <expId> --backend ssh --host lab
orx exp run <expId> --backend ssh --host lab --container research
orx exp run <expId> --backend ssh --host lab --no-container
orx exp run <expId> --backend ssh --host lab --no-container --monitor-interval 60
```

`--monitor-mode auto` is the default. Status checks wait 5 seconds and log
passes wait 2 seconds after returning. `--monitor-interval <seconds>` sets both
waits to a positive whole number of seconds, for example 60 or 600. These settings
are saved with each run; application restarts do not change existing policies.
Both monitoring options apply only to direct SSH runs. For a host with a saved
container, add `--no-container`; container runs retain their existing automatic monitoring.
Use a matching updated CLI and application before enabling the new options;
older running applications and supervisors are not automatically upgraded.
With an explicit interval, each automatic log pass collects up to 1 MiB before
waiting. Legacy defaults still catch up immediately; final draining remains time-bounded. Protocol keepalives
still send lightweight traffic without collecting logs.

For a direct host run that should only be checked when needed:

```sh
orx exp run <expId> --backend ssh --host lab --no-container --monitor-mode on-demand
orx refresh <runId> --json
orx refresh <runId> --collect-logs --json
```

Keep the run ID returned by submission. Use the agent's own scheduler or wait
facility to call `refresh` at the user's requested cadence. This mode starts no
periodic status/log supervisor, including after restart. It currently supports
only direct SSH runs; containers, Slurm and managed rentals keep automatic
monitoring. A changed SSH monitoring default in Settings applies to future direct
runs only; command-line options override it.

`refresh` performs one status observation. `--collect-logs` also collects up to
1 MiB of new bytes within a 20-second log budget. Large logs need repeated explicit
refreshes; `run.backend.monitoring.logsComplete` distinguishes a finished job
from complete log collection. Remote cleanup is queued only after final log
collection. Failed refreshes exit nonzero, retain the last observed job outcome
and record `lastError` without declaring the job stopped.

`orx runs`, `orx logs` and `orx exp wait --interval` read local records. They do
not contact the remote host or refresh an on-demand run. Wait/wake completion
signals become available when a refresh observes the terminal result. End the
periodic agent schedule once the run is terminal and the needed evidence has
been collected.

Cancellation is independent of the monitoring interval. `orx exp cancel` saves
intent and starts one bounded control attempt. If the host is unreachable,
cancellation stays pending; a later refresh, repeated cancel, or application
startup retries it. On-demand mode does not create an indefinite background
retry loop. A successful cancel signal still needs observation of termination.
The remote job is never assumed stopped merely because the host is unreachable.

Machine-local SSH settings can save a container name or ID for each host, and
an absolute remote storage root (`--remote-root /data/alice/orx`) used instead
of `~/.orx` for that host's runs and cached source archives. Each run keeps the
root it was launched with.
A saved default SSH host lets launches omit `--host`.
These defaults apply only to SSH experiments; dashboards, terminals and coding
agents still connect directly to the host.

- Omitted container selection uses the host's saved target. `--container`
  overrides it; `--no-container` explicitly runs on the host. The flags conflict.

Each project's three newest SSH and Slurm runs, counted together across hosts
and including unfinished ones, stay whole. Once an older finished run's log is
collected, the running OpenResearch app removes its host dir except
`tensorboard/`, and the container's copy entirely (retried while the container
is stopped), so anything written into `repo/` goes with it. While cleaning a host it also removes orx's cached source
archives that no active run uses and that have been idle for an hour, and
checks again an hour later.
Unreachable hosts are retried later.
`orx compute configure ssh --host lab --keep-remote-runs true` keeps all of
that host's runs not yet cleaned up. Deleting a project removes its runs'
remote dirs entirely, regardless of that setting.

Keep environment preparation in the project's committed setup/run scripts and
invoke them through the fixed run command, just as with other backends. For
example, a committed `run.sh` can source Conda, activate the environment, and
then run the experiment. Those paths must exist in the selected execution
environment (host or container). There is no separate SSH setup command.

The SSH host needs Bash, tar, and Python 3.6 or newer (standard library only).
OpenResearch does not install Python automatically. Container execution also requires host Docker
access and an already running, unpaused Linux container with Bash, tar, working
`setsid --wait`, and an absolute writable `$HOME`. Execution uses the container's
configured user. There is no `sudo`, user override, Podman support, container
creation or lifecycle management. `--image` and `--flavor` are unsupported.

The committed source archive is cached on the host, then streamed into the
container's `$HOME/.orx/runs/<runId>/repo`. Host logs and exit status stay in the
host's `<root>/runs/<runId>/` (`~/.orx` unless a remote root is saved), so
container removal does not erase them before they are collected. A detached
supervisor reattaches using the saved container ID and start time; later
settings changes cannot redirect an existing run. Do not kill the supervisor.

Cancellation targets only the experiment process group, waits five seconds for
TERM, then uses KILL if needed. It leaves the container and unrelated processes
running. Paused containers must be unpaused by the user before cancellation can
finish. Stopping, removing or restarting a container fails its experiment.
Docker/SSH outages are retried rather than reported as container removal.

Host authentication and container readiness are separate checks. Preflight must
check the explicitly selected container; host readiness does not imply container
readiness.
