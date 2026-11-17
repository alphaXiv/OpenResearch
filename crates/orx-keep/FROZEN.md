# Frozen keeper surfaces

A keeper outlives the server that started it, and every later server version
attaches to it. Everything here is therefore a contract with every keeper still
running anywhere: it may only grow. Add optional fields and new frame kinds;
never rename, retype or repurpose one. A keeper ignores frame kinds, control
ops and signals it does not know; a server ignores frame kinds it does not know.
`PROTOCOL` changes only for a break, and a server must then still speak every
protocol a running keeper may speak, or fall back to stopping that keeper's
child the way a user would. The `Spec` alone passes only between a server and
the keeper it starts from its own build, so it may change with them; it is
listed here because the files and frames carry its meaning onward.

## Starting

`orx keep DIR` (Windows, and Unix without an `orx-host`) or `orx-host keep DIR`
(Unix) reads one JSON `Spec` from stdin until end of input, then prints one
line on stdout: `ready`, or `error: <message>` and exits nonzero. On Unix the
started process forks the keeper and exits at once; the keeper leads a new
session and ignores `SIGHUP`, `SIGINT` and `SIGPIPE`, and its child does not
inherit that. On Windows the keeper puts itself in a Job Object that kills its
members when the keeper exits.

| `Spec` field | Contract |
|---|---|
| `protocol` | `1`. |
| `token` | Secret every attachment must present. |
| `program`, `args`, `cwd` | What to run, where; UTF-8. |
| `env` | `[name, value \| null]` changes over the keeper's inherited environment; `null` removes. `envClear` starts from empty. |
| `stdio` | `{"kind":"pipes","stderr":path\|null}`: stdin and stdout pipes, stderr appended to the file or discarded. `{"kind":"merged"}`: a stdin pipe and one pipe for stdout and stderr, in the order the child writes them. `{"kind":"pty","cols","rows"}`: one pseudoterminal; on Unix the child leads its session with the terminal as its controlling terminal. |
| `retain` | `{"kind":"unacked","limit"}`: keep every output byte not yet acknowledged; at `limit` the keeper stops reading and the child waits. `{"kind":"tail","bytes"}`: keep the newest bytes and never make the child wait. |
| `orphanSeconds` | With no attachment for this long, the keeper ends the child; after the child's exit, it waits this long for an attachment to deliver it to. |

## Directory

The server creates `DIR`, owner-only, and owns `owner.json` (its record of whose
child this is; the keeper never reads it) and the directory's removal. The
keeper writes the rest, each published whole by write, fsync and rename.

| File | Contract |
|---|---|
| `keeper.lock` | Locked exclusively for the keeper's whole life. A failed shared lock attempt (`WouldBlock`) is the only liveness proof; a missing or unlocked file means no keeper. |
| `endpoint.json` | `{protocol, port, token, keeperPid, childPid}`, published once the keeper listens on `127.0.0.1:port` and the child started. |
| `exit.json` | `{code, signal, finishedAt, reason, unfinishedOutput}` once the child exited and its output ended, or two seconds passed. `code` and `signal` are null when unknown; `signal` is Unix only. `reason` is null for an exit of the child's own, `stopped` after a stop, `orphaned` after the orphan timeout. `unfinishedOutput`, present only when true, says the output had not ended: something the child started still held it, and output after the exit may be missing. |
| `keeper.log` | The keeper's stderr, opened by the server. |

## Frames

A TCP connection carries frames: a little-endian `u32` length of the rest, one
kind byte, then the body. Lengths of 0 or above 16 MiB are invalid. The first
frame from the server must be `HELLO` within 5 seconds.

| Kind | Direction | Body |
|---|---|---|
| `0x01 HELLO` | server → keeper | JSON `{protocol, token, from}`. A valid hello makes this connection the attachment and closes any previous one. |
| `0x02 INPUT` | server → keeper | Bytes written whole to the child's stdin or terminal before the next frame is read. |
| `0x03 ACK` | server → keeper | `u64` offset. With `unacked` retention, output before it is released. |
| `0x04 CONTROL` | server → keeper | JSON with `op`: `resize {cols, rows}`, `signal {signal}` (`interrupt`, `terminate`, `kill`), `closeStdin`, `stop`. |
| `0x81 WELCOME` | keeper → server | JSON `{protocol, childPid, start, next}`: output frames follow from `start`, the later of `from` and the oldest retained byte. |
| `0x82 OUTPUT` | keeper → server | `u64` offset of the first byte, then the bytes. Offsets count every byte the child wrote. With `unacked` retention frames are contiguous; with `tail` a frame may start later than the last ended. |
| `0x83 EXIT` | keeper → server | The `exit.json` object, after the last output frame. |
| `0x84 REFUSED` | keeper → server | JSON `{error}`, then the connection closes. |

## Semantics

- **Signals** go to the child's process group on Unix (`SIGINT`, `SIGTERM`,
  `SIGKILL`). On Windows `interrupt` is Ctrl-Break to the child's process group
  for pipes and `\x03` typed into a terminal; `terminate` and `kill` end the
  tree.
- **Stop** ends the tree: on Unix every process descending from the child,
  each stopped as it is found and then killed, including ones in sessions of
  their own, then `SIGKILL` to the child's process group, all preceded by
  `SIGHUP` and 200 ms for a terminal; on Windows every member of the keeper's
  job but the keeper. A child that already exited is not looked for again.
  The keeper exits once the exit was sent to the attachment that asked, or
  right away if none is attached. `signal kill` reaches only the child's
  process group, so a command it started in a session of its own survives.
- **A dead keeper takes its child with it.** Pipes close (and Linux children
  carry `PR_SET_PDEATHSIG`), a terminal hangs up, a Windows job closes.
- **A terminal on Windows** answers a cursor-position query (`ESC[6n`) with
  `ESC[1;1R` when no server is attached to answer it, or when a new attachment
  replaces one that had not, since ConPTY holds the child's output until
  someone does. Every query before a `WELCOME`'s `next` has been answered; one
  after it is that attachment's to answer.

## Tests

`tests/keeper.rs` drives the keeper over these frames on every platform. A
change to any surface here keeps those tests passing unchanged and adds one
for the extension.
