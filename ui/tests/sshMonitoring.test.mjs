import assert from "node:assert/strict";
import { test } from "node:test";
import { sshObservation } from "../src/sshMonitoring.ts";

test("legacy SSH runs keep auto mode; unavailable data never invents freshness or log completion", () => {
  assert.deepEqual(sshObservation({ backend: { kind: "ssh_job" } }), { mode: "auto" });
  const state = sshObservation({ backend: { kind: "ssh_job", monitoring: {
    mode: "on-demand", lastObservedAt: 123, lastError: "unreachable", logsComplete: false,
  } } });
  assert.equal(state.mode, "on-demand");
  assert.equal(state.lastObservedAt, 123);
  assert.equal(state.lastError, "unreachable");
  assert.equal(state.logsComplete, false);
});

test("on-demand controls are scoped to direct SSH", () => {
  for (const backend of [null, { kind: "slurm_job" }, { kind: "openresearch_job" },
    { kind: "ssh_job", sshContainer: { id: "container" } }]) {
    assert.equal(sshObservation({ backend }), null);
  }
});
