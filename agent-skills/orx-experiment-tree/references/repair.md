# Provisional until it answers: repair, don't branch

Every node exists to establish a baseline or test a hypothesis. A run that dies
on an error does **neither**, so there is nothing to protect: fix that node's
branch in place and re-run the same node. Successive runs on one node are how
you get it working.

Once a run *does* answer the node (it produced the result the node was after,
good, bad, or `nan`), the node is **frozen**. Its branch is the code that
result came from: never edit it again, branch a child instead. That holds
however the run ended, and it is permanent: a disappointing number is a
result, not a reason to repair.

Unintended behaviour is not an answer. An OOM, a timeout, a divergence from a
bug, a missing dep: those are implementation and hardware details, and the
node is still provisional (unless the node's hypothesis *is* about memory or
runtime, in which case that outcome is exactly its result).

**Repair cap:** two runs in a row that answer nothing on one node, then ask the
user. Different errors still count; a bare relaunch or a flavor/backend switch
is a repair. If the same failure hits a second node, that is one setup problem:
ask then. This is separate from the "~3 failed or regressed runs" scientific
stop.
