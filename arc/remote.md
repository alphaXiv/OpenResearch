# Remote machines

Research runs where the GPUs are: a workstation under a desk, a rented box, a
cluster's login node, all reached over SSH. The dashboard is a web server with
no login, which is safe only because nothing but the user's own machine can
reach it. orx gets the dashboard to those machines without ever listening on
a public interface, and without asking for anything SSH does not already
hold.

**orx binds only loopback unless the user asks.** An unauthenticated dashboard
on a shared host is an open door to the account that runs it. Everything in
this essay keeps the server on `127.0.0.1` and carries the user to it over
SSH; `orx up --host` is the user's own opt-in for a trusted private network
([dashboard](dashboard.md#the-loopback-guard)).

This essay covers the dashboard on a remote machine. SSH as a place to run
jobs belongs to [experiments](experiments.md); the host side of those jobs and
the SSH transport they share are described at the end. Their on-disk and wire
formats are frozen in [FROZEN.md](../crates/orx-host-proto/FROZEN.md).

## Inside an SSH session

A plain `orx up` started on a remote box notices it is inside an SSH session
from the variables sshd sets. It cannot create a port forward itself, because
only the SSH client on the laptop can, so it prints the exact `ssh -L` command
to run there, as Jupyter and TensorBoard users already know. Everything else
in this essay is the automatic version of that command.

## A remote workspace

The dashboard on the laptop opens a remote machine as a workspace, from the
user's own SSH configuration (an alias, `user@host`, or `user@host:port`).

```text
main app ─► /_orx/workspaces/<session>/api/...
                   └─► SSH forward ─► remote orx (loopback) + bearer token
```

The main dashboard routes each remote workspace through its session's path on
its own origin. The session fixes the SSH forward and bearer token for API
requests, events, file resources and websockets. Switching machines changes
the visible workspace without navigating away from the app; connected
workspaces retain their screens and caches. The event stream follows the visible
workspace and repairs changes received while it was hidden.

The same host, origin and fetch-metadata checks protect these paths as the
local API ([dashboard](dashboard.md)). Another account on the remote machine
cannot use its API without a live attachment's bearer token. `orx up --remote`
opens the same dashboard and authenticated proxy on a loopback gateway port of
its own.

Connecting resolves the remote `orx` once through the account's login
environment and then runs that exact path, so a different non-interactive
`PATH` cannot pick another binary. A candidate binary that is not owned by the
user or root, or that sits in a group- or world-writable place, is never
executed. orx reports it and shows the command that would fix the
permissions, and never changes a shared install itself. Installing or updating
`orx` on the remote is an explicit step the user confirms, with the binary,
database and cache paths visible and changeable.

A new remote machine opens agent configuration without repeating the welcome,
alphaXiv sign-in or demo setup. Agent installations and sign-ins belong to each
machine; the account picker and paper library continue to use the laptop account.
Installing or signing in to Claude Code with a subscription first checks the
remote's public egress IP; a known datacenter address prompts once per setup,
naming the IP, and the warning permits continuing. Allowlisted agent setup commands run on the
remote host through its authenticated API.

## The persistent host

The remote server is a **persistent host**: a detached `orx up` that outlives
any SSH connection. A laptop closing its lid, losing Wi-Fi, or quitting must
not stop the agents and runs working on that machine. The laptop attaches to
the host and detaches from it, and the host keeps working in between.

```text
laptop                                remote account
orx remote-host ensure ──ssh──►  start lock ─► running? report it : start one
orx remote-host attach ──ssh──►  control socket ─► register token, ping
gateway ── forward + token ───►  host's loopback port
```

- **One host per remote data directory.** Like every `orx up`, a host holds
  the data directory's engine lock, so a second dashboard on the same database
  refuses to start, and a start lock serializes concurrent `ensure` calls.
- **A private control channel.** The laptop never talks to the host over the
  network. It runs `orx remote-host` over SSH, which speaks to the host
  through a Unix socket in an owner-only runtime directory. The host checks
  the peer's user ID on every connection and refuses a socket path that is a
  symlink or belongs to someone else. A descriptor file beside the data
  directory names the running instance, its port and its versions.
- **Attachments are live credentials.** Attaching sends a fresh random token
  over SSH. The host accepts API requests carrying it only while the
  attachment heartbeats, every few seconds; when the SSH connection dies, the
  heartbeat lapses and the token stops working. The laptop reconnects on its
  own and attaches again with a new token.
- **Instances are named.** The laptop remembers the instance it attached to.
  If that host has exited when the laptop reconnects, it does not quietly
  start a new one, which would hide that the old host's in-flight turns are
  gone. Starting a new host is the user's explicit choice.
- **Stopping shows what stops.** A stop request carries the activity the user
  reviewed: active turns, queued messages, pending approvals, running jobs and
  other attached laptops. If any of it changed in the meantime, the host
  refuses and shows the new picture.

## Two versions at once

The laptop and the remote run whatever versions they were last updated to.
Two numbers decide whether they can work together:

- the **dashboard protocol**, what the laptop's UI expects of the remote API,
  must match exactly. A newer remote asks the user to update the laptop. An
  older remote is offered an update, which first requires stopping a host
  still running the old binary.
- the **control protocol**, the shape of the control channel, must match, or
  the session reports the host as incompatible.

Only official builds offer to install a released binary. A development build
asks for a binary of the same branch instead, since no public release speaks
its protocol.

Any change to the remote API or the control channel's shape is a protocol
change: bump the number, and keep the laptop able to tell the user what to do
with a mismatched peer.

## orx-host

With the new experiment engine enabled, runs on an SSH host or Slurm cluster
are driven by `orx-host`, a small static binary (Linux 3.2 or newer on x86_64
and aarch64, and macOS) that needs
only `/bin/sh`, Bash, tar and coreutils on the host. Releases embed it, and it
is named by its SHA-256, so one root holds several versions and each is
uploaded once, in the same SSH session that first needs it.

```text
<root>   ~/.orx/engine, or <remote root>/engine; beside the 0.2.x run directories
  bin/orx-host-<sha256>
  sources/<sha256>         content-addressed source archives
  runs/<run>/              launch.json  run.sh  repo/  log  notify
                           wrapper.json  started  exit.json  cleanup.json

orx-host serve   one per SSH session; dies with it
orx-host wrap    one per run, in its own session; outlives SSH and `orx up`
```

There is no daemon to install, upgrade or restart: what persists is files and
each run's wrapper.

- **At most one start.** The wrapper takes its lock, publishes its identity
  (node, boot ID, PID, start time), then creates `started` exclusively and
  fsyncs it before spawning the payload. Its presence forbids another start,
  even if a crash prevented the spawn, and a dead wrapper never authorizes a
  rerun.
- **Receipts are whole.** Every receipt is written to a temporary file,
  fsynced and renamed, so a reader on NFS sees all of it or none. Failing to
  read, parse or inspect is uncertainty, never absence or a verified stop.
- **Identity bounds what a host can prove.** Process checks and signals run
  only on the recorded node and boot. The same node with a new boot proves the
  old execution ended; another node proves nothing about local PIDs.
- **The kill set** is the wrapper's descendants (it is their subreaper on
  Linux), every same-user process carrying the run's tag, and every process
  already seen. A scan is complete only when no candidate was unreadable.
- **Updates never touch a running wrapper.** It keeps its binary and code
  until it exits; a newer `serve` reads its receipts unchanged.

`serve` answers length-prefixed JSON frames on the SSH session's stdio, out of
order. Observation is a long poll: the engine sends its last summary of each
watched run and the host answers when any differs, so there is no journal on
the host and nothing to replay.

## Slurm authority

On a cluster, `serve` runs on the login node and the payload on a compute
node whose processes the login node cannot see, so the login node never judges
liveness. The batch script execs `orx-host wrap`, and receipts cross the shared
filesystem.

```text
1  exit.json on the shared filesystem              → the outcome
2  squeue --states=all                             → starting, running, or terminal until MinJobAge
3  sacct                                           → jobs squeue forgot
4  scheduler terminal, no receipt after 120 s      → cancelled, failed or lost
   scheduler silent or unanswered, no receipt      → unreconciled until Abandon
```

Jobs are submitted with `--no-requeue`, named by the run ID so a lost
submission is found again. Cancel writes the request the wrapper acts on;
`scancel` is used only before the wrapper has started, because it would kill
the wrapper before it wrote its receipt.

## Transport and the auth breaker

The engine talks to each host root over one system `ssh` child, multiplexing
every request on it, with the user's own config, agent and keys; orx never
reads a key. Requests queue in lanes (control, feed, logs, upload), and cheaper
lanes stop dispatching first, so bulk uploads and log pages can never take the
slots a cancel needs.

```text
Idle ─► Connecting ─► Live ─(EOF, 35 s without a reply, clock jump)─► Backoff ─► Connecting
            └─ auth failure, host key, changed route, dead master ─► NeedsAuth
Backoff: 5 s, 15 s, 60 s, 5 min, 15 min cap, jittered, persisted per root
```

**After an authentication failure, no automatic login.** Repeated logins lock
accounts and trip MFA, so the breaker is one per SSH alias, shared by all its
roots, and only the user's Connect closes it.

- **First contact is one batch-mode attempt.** Success classifies the alias as
  a key host, which then connects with no click. A network failure before
  authentication backs off like any key host. Anything that could have reached
  authentication marks the host interactive with the breaker open.
- **Interactive hosts keep an authenticated master on Unix.** Connect relays
  passphrase, password and host-key prompts to the dashboard or CLI. Key hosts
  keep a master only when supervised work needs it: the setting is off, or an
  active supervised SSH or Slurm run uses the alias. Background work attaches
  with prompts disabled.
- **The route is pinned.** `ssh -G` yields the hostname, user, port, proxy and
  identity settings; a changed route opens the breaker until Connect approves
  it.
- Every engine `ssh` runs without a terminal and with askpass disabled, because
  a jump host ignores the outer batch mode and would otherwise prompt.

**Plain ssh shares an interactive master.** On OpenSSH 8.4 and later, a live
master is published through a managed file the user's SSH config includes,
adding that include once and leaving the rest byte for byte. A `Match … exec`
check selects the socket only while its pinned route matches and the socket
exists, so `ssh`, `scp` and `rsync` to that alias reuse the authenticated
session; without a live master they authenticate as usual and never start an
engine login.

On Windows there is no shared master. Connect recovers the transports needed
by existing runs; a second MFA prompt may be required.

## What must stay true

- Nothing binds a non-loopback address unless the user passes `orx up --host`.
- Remote API requests carry a token that is valid only while an SSH
  attachment is alive.
- The control channel is same-user only: owner-only paths, symlinks refused,
  peer user ID checked.
- orx never executes a remote binary that another user could have replaced,
  and never changes permissions on its own.
- Keys and passphrases never pass through orx's storage.
- After an authentication failure, nothing logs in to that alias until the
  user connects.
- A run's payload starts at most once, and only a receipt, a scheduler verdict
  or a changed boot ends it.
- A version mismatch is reported with a way forward, never papered over.
