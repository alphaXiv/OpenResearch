# SSH (`--backend ssh`)

Use this backend when the user asks to run on their own server, or when SSH is
the configured compute default. Authentication uses SSH config, keys and the
agent; `orx` never reads private keys.

With the new experiment engine enabled, the OpenResearch server keeps one SSH
connection per host for all of its runs, logs, and checks. Its first contact with a host is a single key-only attempt: a
host that accepts your keys or agent never needs Connect. If that attempt needs
a password, passphrase, or MFA, nothing retries until you run
`orx compute connect ssh --host lab` (or press Connect), which asks the questions
and keeps an orx-owned connection. On Unix, its shared master survives
OpenResearch restarts; on Windows, engine and supervised runs need separate
connections, so Connect may ask for MFA twice. A changed host route also waits
for Connect. Network failures back off from 5 seconds up to 15 minutes without
declaring any run stopped; the chat hears once
when a host has been unreachable for 10 minutes and again when it returns.
On OpenSSH 8.4 and later, plain `ssh`, `scp` and `rsync` share that interactive
connection through a managed file. orx adds its `Include openresearch_config`
to `~/.ssh/config` once, preserving the existing contents.

```sh
orx exp run <expId> --backend ssh --host lab
orx exp run <expId> --backend ssh --host lab --container research
orx exp run <expId> --backend ssh --host lab --no-container
```

Machine-local SSH settings can save a container name or ID for each host, and
an absolute remote storage root (`--remote-root /data/alice/orx`). Runs and
cached source archives live under `<root>/engine/` (`~/.orx/engine/` by
default) with the new engine, or directly under `<root>` (`~/.orx`) with
supervisors. Each run keeps the root it was launched with.
A saved default SSH host lets launches omit `--host`.
These defaults apply only to SSH experiments; dashboards, terminals and coding
agents still connect directly to the host.

- Omitted container selection uses the host's saved target. `--container`
  overrides it; `--no-container` explicitly runs on the host. The flags conflict.

OpenResearch keeps each run's log on this computer and each project's three
newest SSH and Slurm run dirs on the host. Older finished runs lose their host
dir except `tensorboard/`; `--keep-remote-runs true` keeps them all.

Keep environment preparation in the project's committed setup/run scripts and
invoke them through the fixed run command, just as with other backends. For
example, a committed `run.sh` can source Conda, activate the environment, and
then run the experiment. Those paths must exist in the selected execution
environment (host or container). There is no separate SSH setup command.

With the new engine, the SSH host needs Linux (x86_64 or aarch64, kernel 3.2
or newer) or macOS,
with Bash and tar. OpenResearch installs its small `orx-host` helper under the
storage root on first use. Container execution also requires host Docker
access and an already running, unpaused Linux container with Bash, tar, working
`setsid --wait`, and an absolute writable `$HOME`. Execution uses the container's
configured user. There is no `sudo`, user override, Podman support, container
creation or lifecycle management. `--image` and `--flavor` are unsupported.

The committed source archive is cached on the host, then streamed into the
container's `$HOME/.orx/runs/<runId>/repo`. Host logs and exit status stay in the
host's run directory (`<root>/engine/runs/<runId>/` with the new engine,
`<root>/runs/<runId>/` with supervisors), so container removal does not erase them.
A run is bound to the container ID and start time it launched with; later
settings changes cannot redirect it.

Cancellation targets only the experiment process group, waits five seconds for
TERM, then uses KILL if needed. It leaves the container and unrelated processes
running. A paused container keeps cancellation pending: the stop hook retries
with backoff until the user unpauses it or the container stops or disappears. Stopping, removing or restarting a container fails its experiment.
Docker/SSH outages are retried rather than reported as container removal.
A cancelled run with surviving processes shows Not fully stopped and their count
and host until they stop. Cancelled with an unchecked-process note names the
containment the host could not inspect.

Host authentication and container readiness are separate checks. Preflight must
check the explicitly selected container; host readiness does not imply container
readiness.

With the engine setting off, a detached `orx supervise` watches each run through
the shared SSH service. Background monitoring reuses an authenticated connection
and never starts a fresh login. The host also needs Python 3.6 or newer.
Monitoring outages appear in run status after 60 seconds; reconnect to resume
monitoring.
The new engine’s surviving-process and contact states above do not apply.
