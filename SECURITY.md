# Security model

`orx` executes code on your own machine and, optionally, on remote hosts you
point it at (SSH targets, Slurm/Kubernetes/Ray clusters, cloud job APIs, and
managed OpenResearch compute). This document is the trust model for that
remote-execution boundary: what's authenticated, what isn't, why, and what to
do differently on a host other people also have accounts on (a shared GPU
server, an HPC login node, a lab workstation).

It does not cover `openresearch.sh` (accounts, organizations, sandbox
provisioning, managed-compute catalogs) — that's a separate service with its
own security model; see its own docs.

If you believe you've found a vulnerability, please report it privately
rather than opening a public issue — see "Reporting a vulnerability" below.

## The short version

- **Single-user machine (the common case): nothing to configure.** Every
  loopback service here assumes it's the only thing bound to `127.0.0.1` on a
  box only you have a shell on. That's the actual boundary — not a login
  screen.
- **Shared machine, `orx up --remote user@host` from your laptop:** already
  safe by default. The dashboard on the remote box requires a per-session
  bearer token that never leaves the SSH channel.
- **Shared machine, `orx serve` running on it directly** (the pattern a
  managed provider's API tunnels into): **unauthenticated by default.** Pass
  `--token` or set `ORX_SERVE_TOKEN` before running it on a box you don't
  exclusively control. See [`orx serve`](#orx-serve) below.
- **Shared machine, plain `orx up` run directly on it** (not through
  `--remote`): also unauthenticated, for the same reason. Prefer `orx up
  --remote` from your laptop instead — it gets you the same dashboard with
  the token gate already on.

## Loopback services and their auth posture

`orx` runs three independent local HTTP surfaces. All three bind
`127.0.0.1` only — never `0.0.0.0` — but binding loopback keeps *other
machines* out, not *other local accounts* on the same box. Whether that
matters depends on whether you're the only person with a shell there.

| Service | Default bind | Default auth | When it runs |
|---|---|---|---|
| `orx up` (plain) | `127.0.0.1:4791` | None — loopback is the boundary | Local single-user dashboard |
| `orx up --remote <host>` | Remote: `127.0.0.1:<port>` (SSH-tunneled). Laptop: `127.0.0.1:<gateway-port>` | Per-session bearer token (below) | Working against remote GPUs from a laptop browser |
| `orx serve` | `127.0.0.1:4790` | None by default; opt in with `--token`/`ORX_SERVE_TOKEN` | Exposing the local run store/log stream to another process (typically an API server on the same or a tunneled box) |

`orx up`'s own module doc states this plainly (`src/commands/up.rs:1-11`):
the normal dashboard is loopback-only with no application-level auth by
design, on the assumption that loopback already means "you." The persistent
remote-host mode adds a session bearer *on top of* loopback specifically
because that assumption stops holding once the box is shared.

### `orx up --remote <host>`

This is the flow to reach for on a shared box, and the one place in the
codebase that already has a full session-auth story:

1. Your laptop runs `orx up --remote user@host`. It SSHs in, starts `orx
   remote-host ensure` on the far end, and mints a random per-connection
   bearer token (`uuid4+uuid4`, `src/commands/up_remote.rs`).
2. That token is sent to the remote process over **stdin of the SSH child
   process** — never as a CLI argument (so it never shows up in `ps`) and
   never echoed by the terminal (`-T`, no PTY; see the doc note at
   `src/jobs/ssh.rs` on why the bearer must not hit terminal line
   discipline).
3. The remote dashboard's HTTP layer wraps every route in two middleware
   layers (`src/commands/up.rs`): `loopback_guard` (Host/Origin/
   `sec-fetch-site` checks — CSRF hardening, not the auth boundary) and
   `require_remote_auth`, which SHA-256-digests the request's `Authorization:
   Bearer <token>` and checks it against the set of currently-attached
   session tokens, or — for the one-shot callback route a freshly-attaching
   client hits before it has a session — against a separate callback secret
   compared in constant time (`RemoteAuth::matches_callback`, via
   `src/token_auth::constant_time_eq`).
4. Your laptop's local gateway process (`src/commands/up_remote.rs`) is what
   the browser actually talks to. It proxies to the remote dashboard over the
   SSH tunnel and injects the bearer token **server-side**
   (`sanitized_request_headers`) — the token never reaches browser JavaScript
   or gets written to disk in the UI.
5. Token lifecycle: minted fresh per `attach`, registered in an in-memory set
   for the life of the attach heartbeat loop, and explicitly unregistered on
   disconnect or failure (`src/commands/remote_host.rs`, `RemoteAuth::
   register`/`unregister`). There is no persistent token file and no
   expiration timer to manage — the token simply stops being valid the
   moment the session that minted it ends. Restarting the attach (e.g. after
   a laptop sleep/wake) mints a new one; old ones are gone, not merely
   expired-but-present.
6. A same-machine control channel (a Unix domain socket, not TCP) gates
   attach/status/stop operations on the remote box: connections are checked
   against `SO_PEERCRED` for a same-uid match before the token exchange even
   starts (`src/commands/remote_host.rs`), and the socket's containing
   directory is created `0700` and verified same-owner on every use
   (`ensure_private_dir`). This is what stops a different local user on the
   shared box from attaching to *your* remote-host process at all, token or
   not.
7. **Endpoint minimization is a blocklist today, not an allowlist:**
   `remote_route_forbidden` (`src/commands/up.rs`) explicitly denies a
   specific set of routes even to a holder of a valid session token (self-
   update, data-dir moves, the nested SSH terminal, openresearch login/SSH-key
   management) because they're either destructive or would leak credentials
   that have nothing to do with running this dashboard. Being a blocklist
   means a new dashboard route is remotely reachable by default unless
   someone remembers to add it to this list — flag this explicitly in review
   whenever a PR adds a new `/api/*` route to `up.rs`.

Host key handling for the SSH connection itself (`HostKeyPolicy` in
`src/jobs/ssh.rs`) is deliberately scoped by target type, not a single global
policy:

| Policy | Behavior | Applies to |
|---|---|---|
| `UserConfig` | Defers entirely to your real `~/.ssh/config`/`known_hosts` | `~/.ssh/config` aliases, and `--remote <hostname>` when the target is a hostname |
| `AcceptNew` | `StrictHostKeyChecking=accept-new` against your real `known_hosts` — genuine trust-on-first-use; a later key change is still caught and rejected | `--remote <ip>:<port>` when the target is a raw IP literal (no `known_hosts` entry would exist for one) |
| `Ephemeral` | `StrictHostKeyChecking=no`, `UserKnownHostsFile=/dev/null` — accepts any key, persists nothing | Only the managed `openresearch_job` backend, whose provider-assigned `host:port` gets recycled between different physical machines, so persisting a host key for it would eventually be a *false* trust signal |

`Ephemeral` is intentionally the exception, not the default — if you're
adding a new SSH-based backend, it should default to `UserConfig` or
`AcceptNew` unless it shares the same host-recycling property as managed
compute.

### `orx serve`

`orx serve` exposes the local SQLite run store and per-run logs read-only
over loopback HTTP/SSE (`src/commands/serve.rs`) — its own doc comment
explains the intended shape: "on an agent box the api SSH-tunnels to it and
re-streams." That description is true of the intended caller, but loopback
binding does not stop an *unintended* one: any other local account on that
box can connect to `127.0.0.1:4790` directly, bypassing the SSH tunnel
entirely, and read every run's metadata and full logs.

As of this change, `orx serve` supports the same bearer-token pattern as the
`--remote` flow:

```sh
orx serve --token "$(openssl rand -hex 32)"
# or
ORX_SERVE_TOKEN=... orx serve
```

When set, every route — including `/health` — requires a matching
`Authorization: Bearer <token>`, checked as a SHA-256 digest in constant time
(`src/token_auth`, shared with the `--remote` flow rather than a second
implementation of the same check). `/health` is gated too, for the same
reason `orx up --remote`'s own health route is: a liveness probe still
discloses the running version, which is exactly the kind of detail a
same-box attacker uses to target a known CVE.

**This flag is opt-in, not the default, and that's a real gap.** Whatever
process launches `orx serve` (today, that's `openresearch.sh`'s API tunneling
in over SSH — a separate repository, out of scope for this change per
`AGENTS.md`) has to actually pass a token for this to take effect. If you run
`orx serve` yourself on a box other people also use, pass `--token`. If
you're wiring up an automated launcher, generate a token when you provision
the box and pass it through; don't rely on the loopback bind alone.

## Credential storage and propagation

- **openresearch.sh account token** — `~/.config/openresearch/credentials.json`
  (`src/config.rs`), created with mode `0600` at open time (not written
  world-readable and chmod'd after) and its containing directory tightened to
  `0700`, since `~/.config` itself is world-traversable by convention. The
  Overleaf token/session cookie (`overleaf.json`, same directory) follows the
  identical pattern.
- **SSH keys** — never read or held by `orx` itself. Every SSH operation
  shells out to the real `ssh`/`ssh-keygen`/`ssh-add` binaries and only ever
  handles the *public* half in-process (`src/local/ssh_identity.rs`,
  `src/commands/ssh_key.rs`); pasting a private key into the CLI is rejected.
- **Compute-provider secrets** (Hugging Face, Modal, Weights & Biases,
  provider-specific tokens, etc.) reach the remote job as **environment
  variables written into the generated `run.sh`, or a platform Secret
  object** — never as a `Command` argument, so they're never visible to
  other local users on either end via `ps`. Kubernetes uses a real `Secret`
  (`src/jobs/kubernetes.rs`) referenced via `secretRef`; Modal uses an
  ephemeral Secret (`src/jobs/modal.rs`); SSH/local backends write `export
  KEY=...` lines into a `run.sh` that lives inside a `chmod 700` run
  directory (`src/jobs/ssh.rs`, `src/jobs/localbox.rs`).
- **`~/.openresearch/env`** (synced provider tokens, e.g. `HF_TOKEN`) is read
  by `synced_env_var`/`list_synced_env` and written by `orx` itself via
  `write_synced_env_var`(`s`) — all in `src/config.rs`, all now routed
  through the same owner-only-at-creation helper as `credentials.json`
  above. `openresearch.sh` (out of scope for this change per `AGENTS.md`) may
  also write to this file as part of its own sync flow; if you operate a
  shared box, confirm that side creates it `0600` too rather than assuming
  it inherits `orx`'s guarantee.
- No secret is ever passed as a literal CLI argument anywhere in this
  codebase (checked as part of this audit) — the one place that would show
  up in `ps` for any other local user on the box.

## Testing unauthorized local-user access

- `src/commands/serve.rs` and `src/token_auth.rs` carry tests exercising the
  new `orx serve` token gate end-to-end over a real loopback socket (missing
  token, wrong token, correct token, and that the gate applies to every
  route, not just `/health`), plus unit coverage of the digest/constant-time-
  compare primitives themselves.
- `src/config.rs` carries tests confirming the credential-file helpers
  actually produce `0600`/`0700` on disk (both for a freshly created path and
  for tightening a stale, looser-permissioned one from before this change) —
  the property this whole section depends on, now enforced rather than only
  asserted in a doc comment.
- `src/commands/remote_host.rs` and `src/commands/up_remote.rs` already had
  coverage for the `--remote` bearer/callback distinction and the
  `SO_PEERCRED` same-uid gate before this change.

## Reporting a vulnerability

Please don't open a public GitHub issue for a suspected vulnerability. Email
the maintainers (see the repository's contact info on GitHub) with a
description and reproduction steps; we'll acknowledge and follow up from
there.
