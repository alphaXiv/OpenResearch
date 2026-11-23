# Result and attention messages

## A result arrives

Read `orx runs` and reconcile every newly terminal run, including
runs that finished while you were analyzing an earlier result. Read the file
located by `orx logs <runId>` and compare metrics before deciding whether to
repair, refill a slot, promote a result, or stop. A failed run is not a new
node: repair and relaunch the same experiment as specified by
`orx-experiment-tree`.

## Attention

Attention messages arrive when monitoring or cancellation needs action.
Inspect `orx exp status <expId>` and tell the user what needs fixing. Cancel
only a run that is broken or no longer wanted.

## Cancellation

**Cancellation pending** means the cancel has been accepted but has not reached
the machine. It will be sent when the machine is reachable again or the launch
is confirmed. **Cancelling…** means the machine is stopping the run.

To stop tracking an unreachable run, use `orx exp abandon <runId>`; double-check
that the actual compute is shut down. The cancel stays pending after Abandon.
