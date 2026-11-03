# Remote machines

Research runs where the GPUs are: a workstation under a desk, a rented box, a
cluster's login node, all reached over SSH. The dashboard is a web server with
no login, which is safe only because nothing but the user's own machine can
reach it. orx gets the dashboard to those machines without ever listening on
a public interface, and without asking for anything SSH does not already
hold.

**orx never binds anything but loopback.** An unauthenticated dashboard on a
shared host is an open door to the account that runs it. Everything in this
essay keeps the server on `127.0.0.1` and carries the user to it over SSH.

This essay covers the dashboard on a remote machine. SSH as a place to run
jobs belongs to [experiments](experiments.md); both share the SSH machinery
described at the end.

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
browser ──► local gateway (loopback, one port per remote)
               ├─ UI         served by the laptop's own orx
               └─ /api/...   ─► SSH forward ─► remote orx (loopback) + bearer token
```

Each remote workspace gets a gateway on its own loopback port. The gateway
serves the dashboard from the laptop's binary and proxies only the API through
an SSH local forward to the remote server's loopback port. It applies the same
host, origin and fetch-metadata checks as the dashboard
([dashboard](dashboard.md)), so a web page cannot drive a remote machine
through it. It also adds a bearer token on the way out, so another account on
the remote machine that can reach its loopback port still cannot use the API.

Connecting resolves the remote `orx` once through the account's login
environment and then runs that exact path, so a different non-interactive
`PATH` cannot pick another binary. A candidate binary that is not owned by the
user or root, or that sits in a group- or world-writable place, is never
executed. orx reports it and shows the command that would fix the
permissions, and never changes a shared install itself. Installing or updating
`orx` on the remote is an explicit step the user confirms, with the binary,
database and cache paths visible and changeable.

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

- **One host per remote data directory.** A host holds the data directory's
  dashboard lock exclusively, so a second dashboard on the same database
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

## The SSH service

Every orx process that talks to remote machines, each dashboard and every
supervisor, goes through one detached SSH service per data directory instead
of opening its own connections. It listens on loopback, accepts only callers
that present the token in its owner-only state directory, and keeps one
session per host, with a small helper on the remote end for monitoring and
transfers. Repeated questions are batched: one Slurm queue poll per host
answers every run on that host.

- **The user's SSH, unmodified in behavior.** Transport is the system `ssh`
  with the user's own config, agent and keys; orx never reads a key. On
  OpenSSH versions that support it, orx shares connections with the user's
  own `ssh` through a managed file their SSH config includes, adding that one
  line and leaving the rest of the file byte for byte.
- **Prompts reach the screen.** A passphrase, password or host-key question
  from `ssh` is routed through an askpass bridge to the dashboard's connect
  dialog and answered there. An unanswered prompt times out instead of
  hanging the connection.
- **It stops when the data moves.** A data-directory move asks the service to
  shut down when idle, meaning no active runs and no connection in progress,
  and refuses to move while it is busy.

## What must stay true

- Nothing binds a non-loopback address.
- Remote API requests carry a token that is valid only while an SSH
  attachment is alive.
- The control channel is same-user only: owner-only paths, symlinks refused,
  peer user ID checked.
- orx never executes a remote binary that another user could have replaced,
  and never changes permissions on its own.
- Keys and passphrases never pass through orx's storage.
- A version mismatch is reported with a way forward, never papered over.
