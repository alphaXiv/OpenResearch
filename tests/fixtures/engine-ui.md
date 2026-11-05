# Live engine dashboard fixture

From the workspace root:

```sh
env -u CLAUDE_CONFIG_DIR -u CODEX_HOME sh tests/fixtures/engine-ui.sh
```

The command builds this tree's dashboard and native host, then runs the ignored
`ui_fixture` integration test. No installs or database inserts are used. Port
4930 must be free. Open **http://localhost:4930** after `READY` appears. The
project is **Engine live fixture**. In the collaborative browser, allow port
4930 in the browser toolbar if its port guard asks; a local browser can open
localhost directly. All home, config, data, SSH keys, host logs,
and control sockets are isolated under the printed temporary directories.

| Experiment | Initial behavior |
| --- | --- |
| Steady output | Running payload emits a timestamp and training line each second; contact becomes Unreachable during the initial outage. |
| Large log with host-only middle | Finished log is 1 GiB + 1 MiB. Kept ranges are the first 64 MiB and last 256 MiB. Offset 512 MiB is fetched during setup and cached. Other middle offsets, such as 600 MiB, require the host. |
| Key rejected: NeedsAuth | `lab-login` rejects its configured key; first engine contact opens the authentication breaker. |
| Established run: NeedsAuth | `expired-key` starts a real running job, then its authorized key is revoked and its session reset. The established run becomes NeedsAuth. |
| Interactive Connect | `mfa-box` uses an encrypted key. Connect with passphrase `fixture-passphrase`; its pending payload then starts. |
| Cancel pending during outage | Cancellation is requested after `gpu-box` is blackholed. Restore the host to observe cancellation reconciliation; an unverified cancel may honestly remain Unreconciled. |
| Finished successfully | Exit 0, Done. |
| Finished with failure | Exit 7, Failed. |
| Final log collection pending | Exit 0 after writing 8 MiB. A throttled transfer holds collection pending; the outage interrupts it. Restore the host to complete collection. |

`gpu-box` begins blackholed after successful large-log collection. The fixture
asserts the project runs endpoint and host endpoint, plus a successful middle
page before the outage and a structured 503 afterwards. `lab-login` and
`mfa-box` remain separate real sshd hosts. The lab-login and mfa-box launches stay Submitting with NeedsAuth contact:
no Launch frame was sent. Their causes carry the cleaned SSH diagnostic.
The expired-key run demonstrates NeedsAuth after a handle has been established.
Unreconciled is not manufactured in this fixture; the response-loss regression
uses a real host crash after publishing the launch intent.
Collection remains pending; the hourly failed threshold is not faked.

`READY` prints the control FIFO and `ready.json` path. The latter contains run
and experiment IDs, API snapshots, and measured disk use. Send newline-terminated
commands, for example:

```sh
echo 'restore gpu-box' > /tmp/orx-ui-fixture-<id>/controls
echo 'blackhole gpu-box' > /tmp/orx-ui-fixture-<id>/controls
echo 'restart-sshd gpu-box' > /tmp/orx-ui-fixture-<id>/controls
echo quit > /tmp/orx-ui-fixture-<id>/controls
```

Controls also accept `lab-login`, `expired-key` and `mfa-box`. Blackhole resets existing TCP
sessions and discards new traffic. Restore resets sessions again so dropped
protocol bytes cannot poison recovery. Restart preserves host keys, home, port,
and proxy. Run IDs and directories change on each start.

The large-log run is launched among the newest three remote runs so normal
retention keeps its host-only middle. Launching several newer runs in the same
project can naturally evict it.

Expect about 1.4 GiB of fixture data: 1,074,790,400 host log bytes, 320 MiB kept
locally, plus the 8 MiB collection log and small runtime/source files. Build
artifacts and `ui/dist` are additional reusable workspace files. Steady logs
grow slowly until stopped. Disk use is measured with `du -sk` before readiness.

Ctrl-C, SIGTERM, or `quit` stops the app, detached payload process groups, SSH
sessions, sshd listeners and proxies, and removes temporary homes including big
logs. Cleanup runs `pgrep` against each isolated directory and fails if a process
survives. SIGKILL cannot run cleanup. The fixture stays in the foreground and is
ignored by the normal test suite.

Host GET and `host.updated` SSE objects include `root`; keep per-root details
keyed by `(alias, root)`. Retry acts on every active root for the alias and
returns the worst resulting host object: needs_auth > backoff > connecting >
live > idle. An alias with no root in use returns 404; NeedsAuth requires Connect.
