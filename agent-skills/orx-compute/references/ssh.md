# SSH (`--backend ssh`)

Use this backend when the user asks to run on their own server, or when SSH is
the configured compute default. Authentication uses SSH config, keys and the
agent; `orx` never reads private keys.

```sh
orx exp run <expId> --backend ssh --host lab
orx exp run <expId> --backend ssh --host lab --container research
orx exp run <expId> --backend ssh --host lab --no-container
```

Machine-local SSH settings can save a container name or ID and an optional
setup command for each host. A saved default SSH host lets launches omit `--host`.
These defaults apply only to SSH experiments; dashboards, terminals and coding
agents still connect directly to the host.

- Omitted container selection uses the host's saved target. `--container`
  overrides it; `--no-container` explicitly runs on the host. The flags conflict.
- Setup belongs to the saved target. Changing the container for one launch does
  not inherit that target's setup. Selecting the same target keeps its setup.
- Setup runs once after synced environment exports, inside the staged repository.
  Its shell environment persists into the experiment (including Conda activation).
  A failed setup fails the run; its output appears in the normal log. Control
  operations and Test never execute setup. The fixed run command starts from the
  staged repository even when setup changes directory.

Example setup:

```bash
source /opt/conda/etc/profile.d/conda.sh
conda activate research
```

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
