# SSH (`--backend ssh`)

Use this backend when the user asks to run on their own server, or when SSH is
the configured compute default. Authentication uses SSH config, keys and the
agent; `orx` never reads private keys.

```sh
orx exp run <expId> --backend ssh --host lab
orx exp run <expId> --backend ssh --host lab --container research
orx exp run <expId> --backend ssh --host lab --no-container
```

Machine-local SSH settings can save a container name or ID for each host.
A saved default SSH host lets launches omit `--host`.
These defaults apply only to SSH experiments; dashboards, terminals and coding
agents still connect directly to the host.

- Omitted container selection uses the host's saved target. `--container`
  overrides it; `--no-container` explicitly runs on the host. The flags conflict.

Keep environment preparation in the project's committed setup/run scripts and
invoke them through the fixed run command, just as with other backends. For
example, a committed `run.sh` can source Conda, activate the environment, and
then run the experiment. Those paths must exist in the selected execution
environment (host or container). There is no separate SSH setup command.

The SSH host needs Bash and tar. Container execution also requires host Docker
access and an already running, unpaused Linux container with Bash, tar, working
`setsid --wait`, and an absolute writable `$HOME`. Execution uses the container's
configured user. There is no `sudo`, user override, Podman support, container
creation or lifecycle management. `--image` and `--flavor` are unsupported.

The committed source archive is cached on the host, then streamed into the
container's `$HOME/.orx/runs/<runId>/repo`. Host logs and exit status stay in the
host's `~/.orx/runs/<runId>/`, so container removal does not erase them. A detached
supervisor reattaches using the saved container ID and start time; later settings
changes cannot redirect an existing run. Do not kill the supervisor.

Cancellation targets only the experiment process group, waits five seconds for
TERM, then uses KILL if needed. It leaves the container and unrelated processes
running. Paused containers must be unpaused by the user before cancellation can
finish. Stopping, removing or restarting a container fails its experiment.
Docker/SSH outages are retried rather than reported as container removal.

Host authentication and container readiness are separate checks. Preflight must
check the explicitly selected container; host readiness does not imply container
readiness.
