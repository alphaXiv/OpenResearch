# orx-host

`orx-host serve --root /absolute/root` speaks the types in
[`orx-host-proto`](../orx-host-proto/src/lib.rs) over stdin/stdout. Frames contain
`Message { id, body }`; replies may arrive out of order. EOF stops request work
and command sessions. Detached `wrap RUN_DIR` processes outlive the connection.
`verify-self SHA256` verifies the executable used by the bootstrap uploader.

`PutBlob` resumes a `.part` file and checks overlapping retry bytes. `HasBlob`
checks the supplied total length and SHA before publishing the final source.
`Launch` records intent exclusively, stages a source into `repo/`, and starts a
wrapper. A missing source yields `Staging`; a later feed or repeated launch
resumes it. Equal run ids require equal intents. `SlurmSubmit` passes the supplied
batch script to `sbatch` and records `job.sbatch` to prevent login-node recovery
from starting a compute-node wrapper. The script prepares the shared run directory and
executes `wrap` on the compute node.

The direct child's exit publishes `exit.json` immediately. Its `processes` list
records processes left alive; the wrapper keeps reaping them after the run ends.
`Cleanup` writes `cleanup.req` and wakes the wrapper to run the same stop sequence.
`cleanup.json` records the remaining `processes` and shared `CancelOutcome`, exposed as `Summary.cleanup` in
the feed. Natural disappearance of the last lingering process records that proof
too. Cleanup preserves the original exit receipt.

`RunState::Exited` carries the receipt, including cancellation proof. A cancelled
receipt without `CancelOutcome::Verified` remains Cancelling + Unreconciled in the engine.
Darwin has no subreaper and never verifies cancellation. Unreadable process data
observed during cancellation remains part of the final incomplete scan, except
for processes proven to predate the wrapper and its tag. A
container hook that fails or exceeds 30 seconds also prevents verification.

The `started` file contains the random run tag, allowing best-effort tagged
cleanup after wrapper death without treating an empty scan as containment proof.
The source and launch locks use fcntl across sessions. A process-wide mutex per
run directory serializes launch, cancel, and cleanup, since fcntl ownership is
shared by threads. Source uploads have their own thread mutex.

`test-hooks` in debug builds enables `ORX_TEST_CRASH_AT`: `before_launch`, `launch_published`,
`wrapper_locked`, `wrapper_identity`, `wrapper_started`, `wrapper_spawned`,
`wrapper_before_receipt`, `wrapper_receipt`. Normal test runs cover the crash
states; feature-enabled tests also abort actual wrappers at every wrapper hook.

Tests enable `test-hooks` through a dev dependency. `ORX_TEST_FAST_STOPS=1`
caps stop delays and hook budgets in debug builds; release builds ignore it.
The default-duration test checks the production grace and hook budget.
Large-page tests optimize the protocol, host, JSON and SSH proxy in the test
profile while still checking all 256 MiB, with a distinct header on each page
and four requests in flight to overlap encoding and decoding.
