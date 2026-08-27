# Tinker (`--backend tinker`)

Use Tinker when the user asks for it or it is the configured default. The
experiment controller runs as a supervised process on the machine running orx;
Tinker SDK model operations run remotely.

```sh
orx exp run <expId> --backend tinker
```

## Project environment

- Configure a non-empty `TINKER_API_KEY` in the process environment or
  `~/.openresearch/env` before launching. Never print it or commit it.
- Require Python 3.11 or newer.
- Add and lock `tinker` or `tinker-cookbook` with the package manager already
  used by the experiment. orx does not install the SDK or create a managed
  Python environment.
- Query Tinker's server capabilities before selecting a model; do not hardcode
  model availability.
- Read `ORX_RUN_ID` and attach it as `ServiceClient` user metadata so the run is
  identifiable in Tinker.

## Logs and recovery

- Print training model IDs, metrics, and every `tinker://` checkpoint path so
  they appear in orx logs.
- Periodically create resumable `save_state` checkpoints. When inference is
  needed, create separate sampler checkpoints.
- Treat the latest periodic checkpoint as the recovery boundary. Do not promise
  a final checkpoint during cancellation.

## Cancellation

- `orx exp cancel` stops the local controller, so it sends no new Tinker
  requests. Already accepted requests may finish; do not claim provider-side
  termination.

There is no flavor, host, image, manifest, or provider-specific timeout option.
The machine running orx must remain awake and connected while the controller is
active.
