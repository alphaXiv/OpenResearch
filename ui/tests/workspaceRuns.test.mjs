import assert from "node:assert/strict";
import test from "node:test";
import { activeWorkspaceRuns } from "../src/workspaceRuns.ts";

test("active workspace runs stay scoped to an existing chat and retain the run destination", () => {
  const experiments = [
    { id: "mine", chatSessionId: "chat" },
    { id: "other", chatSessionId: "other-chat" },
    { id: "cli", chatSessionId: null },
  ];
  const runs = [
    { id: "starting", experimentId: "mine", status: "starting" },
    { id: "running", experimentId: "mine", status: "running" },
    { id: "done", experimentId: "mine", status: "done" },
    { id: "other", experimentId: "other", status: "running" },
    { id: "cli", experimentId: "cli", status: "running" },
    { id: "missing", experimentId: "missing", status: "running" },
  ];
  assert.deepEqual(activeWorkspaceRuns(experiments, runs, null), []);
  assert.deepEqual(activeWorkspaceRuns(experiments, runs, "chat").map(({ experiment, run }) => [experiment.id, run.id]), [["mine", "starting"], ["mine", "running"]]);
});
