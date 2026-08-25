# SSH (`--backend ssh`)

Use this backend only when the user explicitly asks to run on their own machine
or server, or when it is the configured default. It runs a detached process on
a host from `~/.ssh/config` using that host's environment.

```sh
orx exp run <expId> --backend ssh --host my-gpu-box
```

- `--host` is required on every launch and must be an SSH config alias. There is
  no flavor.
- Authentication uses the user's SSH keys or agent. `orx` invokes SSH but never
  reads a private key. The host needs `bash` and `tar`.
- The committed snapshot is streamed to the host and extracted before the fixed
  run command starts.
- There is no image or timeout flag; the host environment is used as-is.
- Runs live under `~/.orx/runs/<runId>/` on the host. Cancellation terminates
  the remote process group.
- A detached `orx supervise` process polls the host; do not kill it.
