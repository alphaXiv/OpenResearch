# Managed experiment rentals

This lifecycle applies only to experiments submitted to the `openresearch`
backend. Standalone instances and other compute backends keep their existing
lifecycle. Existing experiment descriptors without a rental capability use the
legacy local cleanup path.

The user's OpenResearch process stages source and the command directly over SSH.
An owner-only configuration on the experiment machine gives a detached `orx
rental-supervise` process a capability for that rental alone. It supervises the
payload, writes logs and the exit code on that machine, and contacts the billing
service directly. Laptop sleep and SSH failure do not control cleanup.

The rental heartbeat is an empty request every 30 seconds. Five minutes without
a heartbeat expires the rental, even if work is still healthy. Bootstrap is
bounded to 30 minutes; activation starts the requested execution limit (four
hours by default), plus 30 seconds for forced termination and 60 seconds for
collection. Activation retries cannot extend that limit.

After the payload exits, the remote supervisor waits for a direct acknowledgment
that the user machine saved the final log and exit code, or 60 seconds, whichever
comes first. This collection interval is billed to the user. It then requests
release. The server atomically closes billing and queues provider deletion;
provider failures retry durably. Billing does not wait for physical deletion.

Logs, commands, source, experiment IDs, outcomes, and artifacts are never sent to
the billing service by this protocol. Experiment history and downloaded logs stay
on the user machine. Collection is best effort: after deletion, data remaining
only on the experiment machine is lost. If the user machine cannot observe a
final outcome before deletion, it records “Outcome unavailable” rather than
leaving the experiment running indefinitely.

Deploy the matching service lease API and migration before releasing this client.
New rentals require that API and a remote binary supporting `rental-supervise`.
