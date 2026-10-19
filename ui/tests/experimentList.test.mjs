import assert from "node:assert/strict";
import test from "node:test";
import { experimentRows, filterExperimentRows } from "../src/experimentList.ts";

const experiment = (id, title, createdAt = 1) => ({ id, title, slug: id, branchName: `orx/${id}`, createdAt });
const run = (id, experimentId, status, createdAt) => ({ id, experimentId, status, createdAt });
const options = { search: "", filter: "all", sort: "recent", locale: "en" };
const ids = (rows, overrides = {}) => filterExperimentRows(rows, { ...options, ...overrides }).map((row) => row.experiment.id);

test("failed filter uses the latest outcome, with live retries taking precedence", () => {
  const rows = experimentRows([experiment("fixed", "Fixed"), experiment("retry", "Retry"), experiment("failed", "Failed"), experiment("idle", "Idle")], [
    run("old", "fixed", "failed", 1), run("new", "fixed", "done", 2),
    run("retrying", "retry", "starting", 3), run("failure", "retry", "failed", 2),
    run("failed", "failed", "failed", 4), run("orphan", "missing", "running", 5),
  ]);
  assert.deepEqual(ids(rows, { filter: "failed" }), ["failed"]);
  assert.deepEqual(ids(rows, { filter: "active" }), ["retry"]);
  assert.equal(rows.find((row) => row.experiment.id === "idle").latestRun, undefined);
});

test("search matches all words across the title, description, and branch, ignoring whitespace and case", () => {
  const rows = experimentRows([
    { ...experiment("lr-probe", "Learning rate"), description: "Cosine warmup schedule" },
    experiment("vocab", "Vocabulary probe"),
  ], []);
  assert.deepEqual(ids(rows, { search: "  LEARNING   orx/lr  " }), ["lr-probe"]);
  assert.deepEqual(ids(rows, { search: "cosine warmup" }), ["lr-probe"]);
  assert.deepEqual(ids(rows, { search: "missing" }), []);
  assert.deepEqual(ids(rows, { search: "probe", filter: "active" }), []);
});

test("sorting uses recent runs or creation time and natural title order without mutating inputs", () => {
  const experiments = [experiment("ten", "Trial 10", 5), experiment("two", "Trial 2", 3), experiment("one", "Trial 1", 8)];
  const runs = [run("older", "two", "done", 2), run("latest", "two", "done", 10)];
  const rows = experimentRows(experiments, runs);
  assert.deepEqual(ids(rows), ["two", "one", "ten"]);
  assert.deepEqual(ids(rows, { sort: "name" }), ["one", "two", "ten"]);
  assert.deepEqual(rows.map((row) => row.experiment.id), ["ten", "two", "one"]);
  assert.deepEqual(runs.map((item) => item.id), ["older", "latest"]);
});
