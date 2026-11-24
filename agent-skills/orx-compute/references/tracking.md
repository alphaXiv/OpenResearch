# Run environment and metrics

Every run gets `ORX_RUN_ID`, `ORX_EXPERIMENT`, `ORX_EXPERIMENT_TITLE`,
`ORX_PARENT_EXPERIMENT` (when it has a parent), `ORX_COMMIT` and
`TRACKIO_PROJECT`. Saved environment variables reach every backend except
`hf`, which gets only the `ORX_*` and Trackio ones.

## Trackio

To track metrics, log with Trackio; orx shows its charts beside the run:

```python
trackio.init(project=os.environ.get("TRACKIO_PROJECT", <name>),
             name=os.environ.get("ORX_RUN_ID"), group=os.environ.get("ORX_EXPERIMENT"),
             config={"hypothesis": os.environ.get("ORX_EXPERIMENT_TITLE"),
                     "parent": os.environ.get("ORX_PARENT_EXPERIMENT"),
                     "commit": os.environ.get("ORX_COMMIT"), **hyperparameters})
```

Keep that project and name, or orx cannot find the run. The client reads
`TRACKIO_SPACE_ID` or `TRACKIO_SERVER_URL` and its token itself. With neither
saved, only `local` runs show up in orx, and a localhost URL only works on the
`local` backend. Never print a token.

## Reading metrics back

`orx metrics <runId> [<runId>…]` reads back what runs logged to Trackio,
wherever it went: per metric its first, last, min and max (with steps), the
mean over the last tenth, a sparkline, and sampled `step:value` points.
`--metric loss` filters, `--points 50` adds resolution, `--json` is
machine-readable. Read it while a run is live and after it ends, and pass the
parent experiment's run too to compare curves, not just final numbers.

## TensorBoard

`local`, `tinker`, `ssh` and `slurm` runs also get `TENSORBOARD_LOGDIR`, a
per-run directory; pass it as the trainer's `logging_dir`/`log_dir`.
`orx tensorboard <runId>` prints the command that opens it (not for SSH
container runs).
