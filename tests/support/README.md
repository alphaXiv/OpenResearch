# Real SSH test support

`orx-test-support` is a Unix-only path dev-dependency shared by binary unit tests
and integration tests. `cargo test --locked` runs its unit test and the
`tests/sshd.rs` integration suite; no Docker, root, running SSH service, or
pre-existing SSH keys are required. Debian/Ubuntu needs `openssh-server`
(`sudo apt-get install -y openssh-server`); macOS uses `/usr/sbin/sshd`.

```rust
use orx_test_support::{Faults, SshdBuilder};

let server = SshdBuilder {
    max_sessions: 1,
    ..SshdBuilder::default()
}.start()?;
let output = server.run("mkdir -p ~/.orx; printf '%s' \"$HOME\"")?;
assert!(output.status.success());
server.proxy().set(Faults {
    black_hole: true,
    ..Faults::default()
})?;
server.proxy().reset()?;
server.proxy().clear()?;
```

Always connect with `server.ssh_args()` (`-F config alias`) or `server.spawn()`.
OpenSSH finds its client configuration through the passwd home, so changing the
local `HOME` alone does not isolate a client. The generated config pins the host
key and client identity, disables agent use, and puts ControlPath in the private
test directory. sshd sets HOME before starting the user's shell; its forced
command also sets HOME and cwd to `server.home()`. Commands use `/bin/sh`.

`spawn(options, command)` returns a `Process` guard with a PID and bounded
`wait(timeout)`. Keep ControlMasters in the foreground (`-M -N`, without `-f`).
Output is captured in anonymous temporary files so pipe buffers cannot deadlock
commands. Unwinding kills and reaps the owned process group. Server teardown
requests exit on ControlMaster sockets in its directory before killing sshd,
stopping the proxy, and deleting the directory.

The proxy blocks in `poll` on socket readiness and a control wake socket, with
deadlines only for delayed chunks. Queues bound reads and preserve backpressure.

`Faults` changes both directions of existing and future connections. `set()` and
`reset()` acknowledge after application. Latency delays received chunks;
bandwidth is per connection and direction, with chunks at most 4096 bytes.
Pause retains bytes and applies backpressure; black-hole reads and discards
bytes while keeping sockets open. Black-hole recovery does not restore discarded
SSH bytes: reset connections before reconnecting. Refusal closes the listener
without affecting established connections; clearing it reopens the same port.

`SshdBuilder::authentication_methods` defaults to `publickey`. Additional
`extra_client_keys` entries generate authorized `client_key_1`, `client_key_2`,
etc.; `None` leaves a key unencrypted and `Some(passphrase)` encrypts it.
`identity_files` selects paths relative to the harness directory (default:
`client_key`; an empty list offers no identities). Startup waits for the listener
without authenticating.

`server.askpass(passphrase)` returns a script path for `SSH_ASKPASS`; set
`SSH_ASKPASS_REQUIRE=force`, `DISPLAY=orx-test`, and override `BatchMode=no`.
Each invocation appends its prompt to `server.directory().join("askpass-prompts")`.
Answers are read from a private file, so shell syntax in passphrases is literal.

`auth_attempts()` snapshots counts keyed by `(method, outcome)` from sshd stderr.
Log delivery is asynchronous; wait for the expected events before comparing
snapshots for a quiet window. `Postponed` counts unsigned public-key probes; `Partial` counts a completed MFA factor, `Accepted` completes
authentication, and `Failed` rejects an attempt. A successful key login normally
logs both a probe and an acceptance; summing those events counts protocol steps,
not login connections. Method discovery (`none`) and key-file lookup messages
are excluded. An encrypted key under BatchMode logs one `Postponed` probe and
no signed attempt. The parser accepts optional log prefixes and submethods.
The auth_log format is checked against upstream
[8.0p1](https://github.com/openssh/openssh-portable/blob/V_8_0_P1/auth.c),
[8.9p1](https://github.com/openssh/openssh-portable/blob/V_8_9_P1/auth.c), and
[9.6p1](https://github.com/openssh/openssh-portable/blob/V_9_6_P1/auth.c).

`host_binaries()` honors `CARGO_TARGET_DIR` for its nested host build; without it,
it uses `target`. The nested build lives in its `test-hooks` subdirectory.
The opt-in seeded chaos tier is documented in [its module](../../src/engine/runtime/chaos.rs).
