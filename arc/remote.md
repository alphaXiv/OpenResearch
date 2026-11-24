# Remote machines

Research runs where the GPUs are, on machines reached over SSH. orx brings the dashboard and the run engine there without listening on a public interface and without asking for anything SSH does not already hold. On-disk and wire formats are frozen in [FROZEN.md](../crates/orx-host-proto/FROZEN.md); the run lifecycle belongs to [experiments](experiments.md).

**orx binds only loopback unless the user asks.** Servers stay on `127.0.0.1` and SSH carries the user to them; `orx up --host` is the user's own opt-in for a trusted private network ([dashboard](dashboard.md#the-loopback-guard)). Inside an SSH session, plain `orx up` prints the `ssh -L` command to run on the laptop (`src/remote.rs`), and the rest of this essay automates that command.

## A remote workspace

The laptop's dashboard opens a remote machine as a workspace, from a host saved in Connect to remote ([saved hosts](#saved-hosts), `src/commands/up_remote.rs`): `/_orx/workspaces/<session>/…` on the laptop's origin → the session's gateway → SSH forward → remote `orx` on its loopback. `orx up --remote` serves the same routes on a loopback gateway port of its own.

- **The laptop serves the UI.** Only `/api` reaches the remote, so one version's UI drives another version's API.
- **The session holds the credential.** The gateway adds the bearer token to API requests, events, file resources and websockets, and drops the browser's own credentials; the page never sees the token. The local API's guard protects these paths ([dashboard](dashboard.md)).
- **Accounts stay on the laptop.** The gateway answers account and paper library routes itself. Agent installs and sign-ins belong to each machine, and agent setup runs on the host from an allowlist (`src/commands/up/harness_setup.rs`).
- **The laptop lends its alphaXiv sign-in to the host's agents.** While attached, it forwards a short-lived access token on a route outside `/api`, so no page reaches it. The refresh token never leaves the laptop, because two machines refreshing one sign-in retire each other's tokens; a host's own sign-in wins over a lent one. Signing out on the laptop withdraws it; detaching does not, so a host keeps it until it expires (`src/alphaxiv.rs`).
- **A host signs itself in with a code.** alphaXiv sends a sign-in back only to a browser on the machine that started it, so `orx login` inside SSH shows a link and code to approve on any device instead.
- **The host refuses what only makes sense locally:** those account routes, file pickers, self-update, data-directory moves, the general settings command runner. A route that acts on the machine in front of the user joins the forbidden list in `src/commands/up.rs`.

**orx never runs a remote binary another user could have replaced.** It resolves `orx` once through the login environment and then runs that exact path, so a non-interactive `PATH` cannot pick another binary. A candidate must be owned by the user or root, with neither it nor its directory writable by anyone else. A rejected candidate is reported, not treated as missing, and a repairing `chmod` runs only when the user asks. Installing or updating `orx` is a step the user confirms, with its paths visible and changeable.

## Saved hosts

**orx reaches every host through its own `ssh -F` file and never edits the user's SSH configuration** (`src/jobs/ssh/hosts.rs`), which may be read-only, as a Nix-managed `~/.ssh/config` is. The generated file includes the hosts saved in orx, then `~/.ssh/config` and the system file, so plain `ssh` and orx resolve every other alias alike.

- **A host is added the way the user already reaches it:** a hostname, an alias, `user@host:port`, or a pasted `ssh …` command, which becomes the `Host` block that makes `ssh <alias>` connect the same way. Options that shape one session (forwards, a remote command) are dropped; options orx can't keep are refused by name.
- **The alias stays the key.** Engine state, the auth breaker, the route pin and compute settings all name a host by alias, so a saved host is indistinguishable from a configured one. An alias saved in orx means one thing in both lists, so a conflicting second definition is refused; a saved alias overrides the same alias in `~/.ssh/config`, for orx only.
- **Forgetting a remote never stops its host.** A saved remote stays listed until the user forgets it, which ends its session and leaves the [persistent host](#the-persistent-host) running.
- **Two lists, one file each:** `remotes` for Connect to remote, `compute` for SSH and Slurm compute, which also lists `~/.ssh/config`. A machine for agents and one only for runs are set up in different places, each pointing at the other.
- **Passwords are saved, one try at a time.** A password or key passphrase that let a login through is kept in the system credential store under the alias (`src/local/keychain.rs`); one-time codes and host-key answers never are. A saved answer is offered only within one login attempt, a failed login forgets it, Linux without `secret-tool` asks every time, and the [auth breaker](#transport-and-the-auth-breaker) still opens on failure (`src/engine/transport/askpass.rs`).

## The persistent host

**The remote server is a persistent host: a detached `orx up` that outlives any SSH connection,** because a laptop closing its lid must not stop the agents and runs on that machine (`src/commands/remote_host.rs`). Its control channel is a Unix socket, so it runs only on Unix.

Attaching, all over SSH:

1. `orx remote-host ensure` reports the running host or starts one.
2. One `ssh -L` child forwards a local port to the host and runs `orx remote-host attach`, which registers a fresh token on the control socket, so the forward and the attachment die together.
3. The laptop checks version, protocol and instance through the forward, then routes traffic with the token.

The rules around it:

- **One host per remote data directory.** It holds the engine lock ([storage](storage.md)), and a start lock serializes `ensure`. Locks and descriptor sit beside the data directory while the socket is node-local, so on a shared home a host on another node blocks a second start rather than accepting attachments.
- **A same-user control channel.** The host checks the peer's user ID on every connection and refuses a socket path that is a symlink or someone else's; the socket sits in an owner-only directory. The descriptor names the instance, its port and its versions.
- **Attachments are live credentials.** The token travels on SSH stdin, never a command line. The host keeps its digest and accepts it only while the attachment heartbeats, so it dies with the SSH connection; the laptop then reattaches with a new one. Agents and the CLI on the host hold credentials of their own ([agents](agents.md)).
- **Instances are named.** A laptop that finds its host gone does not quietly start a new one, which would hide that the old host's in-flight turns are gone. Starting a new host is the user's explicit choice.
- **Stopping shows what stops.** A stop request carries the activity the user reviewed (turns, queued messages, approvals, jobs, attached laptops). If any of it changed, the host refuses and shows the new picture.

## Two versions at once

The laptop and the remote run whatever versions they were last updated to, so **from dashboard protocol 8 on, every laptop opens every remote's workspace**, older or newer than itself. The laptop's UI is the client and the remote's API the server, and both only grow: a change that would break a shipped peer gets a new route name instead.

- **Dashboard protocol** (`src/commands/up_remote.rs`): the level of the remote API. The laptop refuses only a remote below 8, offering an update that installs beside the running host and asks it to restart with its work kept ([restarts](restarts.md)); a host too old to restart must be stopped first.
- **Control protocol** (`src/commands/remote_host.rs`): the control channel's level, which also only grows. A host below the laptop's level can't restart into an update, so the laptop asks to stop it first.
- **Host protocol** (`crates/orx-host-proto`): Hello requires the exact version and binary SHA. The engine uploads its own `orx-host`, so `serve` never skews; only wrappers and files outlive a version.

**A newer server never refuses an older client.** A field a request gains is optional, and its absence means what it meant to the version that omits it; the server takes that default instead of unwrapping, even once every current client sends it. Because an older server ignores a field it doesn't know, a change to what a request does gets a new route name rather than a new field, and the old route stays until the laptops calling it have aged out. `e2e/tests/api-contract.spec.ts` replays each call a protocol-8 dashboard makes, frozen in `tests/contracts/dashboard-api-8.json`, and fails when the current server lacks its route or refuses its body.

**A newer client asks less of an older server.** A response field added after 8 is optional in the UI's types, and the UI hides what needs it when it is missing. A new route the UI offers bumps `DASHBOARD_PROTOCOL`, and the UI shows its entry point only for a remote reporting that level (`dashboardProtocol` on the session), since an older server answers it with a bare 404; a field never needs a bump.

**An older build is an offer, not a wall.** When a remote's build is older than the laptop's, the machine switcher offers Update; the workspace stays usable while the build installs, and only the host's restart, a few seconds of reconnecting, interrupts it.

**Installing or updating a remote puts the laptop's own build there,** so the protocols match by construction. Only production builds ([desktop](desktop.md)) install: a stable laptop runs the release installer for its version, and a canary, which shares its version number with a stable release, downloads its own headless CLI from the canary release, checks it against the published SHA-256 and renames it over the binary path the stable installer would use. The probe also reads the remote's build ID (`orx version --build-id`, the version or canary tag), so a build already in place is not installed again and the dashboard can say which build a host runs. Old canaries are pruned, so a laptop whose build is gone asks the user to update it. A development build asks for a binary of the same branch, since no public release speaks its protocol.

## orx-host

**A host keeps files and one wrapper per run; there is no daemon to install, upgrade or restart.** `orx-host` (`crates/orx-host`) drives SSH, Slurm and OpenResearch box runs on Linux and macOS, needing only `/bin/sh`, Bash, tar and coreutils there. Releases embed it, named by SHA-256, so one root holds several versions and the SSH bootstrap (`src/engine/transport/ssh.rs`) uploads each once.

A root, `~/.orx/engine` or `<remote root>/engine` beside the 0.2.x run directories, holds the binaries, content-addressed source archives and one directory per run; FROZEN.md defines every file in it.

Two roles run from that binary:

- `orx-host serve`: one per SSH session; dies with it. It answers length-prefixed JSON frames on stdio, out of order. Observation is a long poll: the engine sends its last summary of each watched run and the host answers when any differs, so the host keeps no journal to replay.
- `orx-host wrap`: one per run, in its own session; outlives SSH and `orx up`.

Their rules:

- **A launch only publishes.** `serve` answers once `launch.json` and the pinned source are on disk; the wrapper unpacks the source into `repo/` before it starts the payload, under its lock and with no deadline, and a cancel stops the unpacking. A run that can't start ends with a receipt saying why. On Slurm the unpacking runs on the compute node, inside the allocation.
- **At most one start.** The wrapper creates `started` exclusively and fsyncs it before spawning the payload. Its presence forbids another start, even if a crash prevented the spawn, and a dead wrapper never authorizes a rerun.
- **Receipts are whole,** so an NFS reader sees all of one or none. Failing to read, parse or inspect is uncertainty, never absence or a verified stop.
- **Identity bounds what a host can prove.** Process checks and signals run only on the recorded node and boot. A new boot on that node proves the old execution ended; another node proves nothing about local PIDs.
- **The kill set** is the wrapper's descendants (it is their subreaper on Linux), every same-user process carrying the run's tag, and every process already seen. A scan is complete only while the wrapper lives and no candidate was unreadable. On macOS some processes, Apple's own binaries among them, show others an empty environment, so once the wrapper is gone no tag scan there is complete and a dead wrapper leaves its run unreconciled.
- **Updates never touch a running wrapper.** It keeps its code until it exits, and a newer `serve` reads its receipts unchanged.
- **Host locks are `fcntl` locks** on run-directory files, so a shared filesystem must honor them across nodes.

## Slurm authority

**The login node never judges liveness,** because `serve` runs there and the payload runs on a compute node it cannot see. The batch script execs `orx-host wrap`, and receipts cross the shared filesystem. Evidence, strongest first (`crates/orx-host/src/server.rs`, `src/engine/adapters/slurm.rs`):

1. `exit.json` on the shared filesystem: the outcome.
2. `squeue --states=all`: starting, running, or terminal until `MinJobAge`.
3. `sacct`: jobs `squeue` forgot.
4. A terminal scheduler state with no receipt after a grace period for filesystem visibility: cancelled, failed or lost, never done.
5. A silent or unanswered scheduler with no receipt: unreconciled until Abandon.

Jobs are submitted with `--no-requeue`, named by the run ID so a lost submission is found again. An engine run's cancel writes the request the wrapper acts on; `scancel` runs only before the wrapper has started, because it would kill the wrapper before it wrote its receipt.

## Transport and the auth breaker

**The engine reaches each host root over one system `ssh` child, multiplexing every request on it** (`src/engine/transport/`), with orx's [SSH file](#saved-hosts) over the user's own config, agent and keys; orx never reads a key. Requests queue in lanes (control, feed, logs, upload), and cheaper lanes stop dispatching first, so bulk uploads and log pages can never take the slots a cancel needs.

- Idle → Connecting → Live.
- Connecting → Backoff on a network failure.
- Connecting → NeedsAuth on an authentication or host-key failure, a changed route or a dead master.
- Live → Backoff on EOF, a missed reply deadline or a clock jump.
- Backoff → Connecting when the jittered retry, persisted per root, comes due.
- Any state → NeedsAuth when the alias's breaker opens; only Connect clears it.

An OpenResearch box is a key-only host orx rents and forgets once nothing needs it; it has no Connect, so it retries on a tighter schedule and gives a fresh box time to accept its key before the breaker opens (`src/engine/transport/mod.rs`).

**After an authentication failure, no automatic login.** Repeated logins lock accounts and trip MFA, so the breaker is one per SSH alias, shared by its roots and kept in the store. Only the user's Connect closes it, except that on Unix a saved password logs an ended master back in once; a retry resets a key host's backoff and nothing more.

- **First contact is one batch-mode attempt.** Success classifies the alias as a key host, which then connects with no click. A network failure before authentication backs off like any key host. Anything that could have reached authentication marks the host interactive with the breaker open, and a first attempt interrupted by a crash opens the breaker too.
- **Interactive hosts keep an authenticated master on Unix.** Connect relays passphrase, password and host-key prompts to the dashboard or CLI, answering a password from the credential store first ([saved hosts](#saved-hosts)). Key hosts keep a master only when the run engine is off or an active SSH or Slurm run uses the alias.
- **The route is pinned.** A fingerprint of `ssh -G` covers destination, identity, proxy and host-key settings, and a change opens the breaker until Connect approves it.
- **Background `ssh` never prompts.** It runs without a terminal and with askpass disabled, because a jump host ignores the outer batch mode and would otherwise prompt.

Workspace tunnels and probes take a separate path (`src/jobs/ssh.rs`): batch mode over the ControlMaster shared with compute settings, ending at an authentication failure instead of retrying.

**Plain ssh can share an authenticated master.** On OpenSSH 8.4 and later, a live master is published through a managed file, `~/.ssh/openresearch_config`, which the user's SSH config may include; orx never adds that line (`src/jobs/ssh/sharing.rs`). A `Match … exec` check selects the socket only while its pinned route matches and the socket exists; without a live master, `ssh` authenticates as usual and never starts an engine login.

**On Windows the SSH service holds the login,** because Win32-OpenSSH cannot multiplex. Connect starts each root's `ssh` inside the detached `orx ssh-service` (`src/jobs/ssh/service/hold.rs`), reached on loopback with the token in its owner-only directory, and `orx up` talks to orx-host through it. A restarted or updated server reattaches with no prompt while the control protocol matches; the older orx-host then serves, and wraps new runs, until the login ends. It ends when the user signs out of Windows, Connect replaces it, its route changes, or no server has attached for a long while. Plain `ssh` does not share it, and Connect still logs in the transports existing 0.2.x runs need, so a second MFA prompt may be required.

A seeded chaos tier (`src/engine/runtime/chaos.rs`) holds the engine, transport and `orx-host` to these rules under faults over real SSH; its module comment says how to run and replay it.

## Advisory run signals

**Run signals are folded where the log lives.** A native `serve` scans each watched run's log, Slurm jobs' included, for declared `::orx::` records and detected progress, and keeps the fold as a checkpoint in the host root, so the workspace pulls a bounded snapshot instead of the whole log and a finished run keeps its signals after its log is gone. Local runs use the same scanner in process. A connection advertises snapshots by a `signals` field in its summary; against an older `serve` the workspace reads the log through the frozen byte-page API and folds it itself. Answers carry the host's clock, so the workspace knows how current they are. Watchers and stop rules always run on the workspace, and signals never change a run's lifecycle ([FROZEN.md](../crates/orx-host-proto/FROZEN.md)).
