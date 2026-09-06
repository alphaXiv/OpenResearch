import assert from "node:assert/strict";
import test from "node:test";

import {
  createWorkspaceWriter,
  emptyProjectWorkspace,
  parseDestination,
  parsePane,
  safeLocation,
  settingsTabs,
  taskLocation,
} from "../src/workspaceState.ts";

const panes = [
  ...["experiments", "files", "artifacts"].map((view) => ({ kind: "home", view })),
  { kind: "experiment", experimentId: "experiment", view: "overview" },
  { kind: "experiment", experimentId: "experiment", view: "terminal", runId: "run" },
  { kind: "file", path: "paper.tex" },
  { kind: "file", path: "研究/figure +100%?draft#１.tex", source: "repo", sessionId: "session α", ref: "feature/分析", line: 12 },
  { kind: "file", path: "figure.svg", source: "artifacts" },
  { kind: "file", path: "/tmp/paper.tex", source: "abs", line: Number.MAX_SAFE_INTEGER },
  { kind: "code", experimentId: "experiment", branch: "main", view: "files" },
  { kind: "code", experimentId: "experiment", branch: "feature", view: "changes" },
  { kind: "plan", sessionId: "session", promptId: "prompt" },
  { kind: "subagent", sessionId: "session", spawnPartId: "part" },
];

test("every pane variant round-trips through a concrete task URL", () => {
  for (const pane of panes) {
    assert.deepEqual(parsePane(pane), pane);
    const location = taskLocation("project α", "task % +", pane);
    assert.equal(safeLocation(location), location);
    const url = new URL(location, "http://localhost:8080");
    assert.deepEqual(parsePane(JSON.parse(url.searchParams.get("pane"))), pane);
    assert.deepEqual(parseDestination(url.pathname), {
      kind: "task", projectId: "project α", sessionId: "task % +",
    });
    assert.equal(new URL(location, "http://localhost:9999").pathname, url.pathname);
  }
  assert.equal(taskLocation("demo", null), "/projects/demo/tasks/new");
  assert.equal(taskLocation("demo", null, null), "/projects/demo/tasks/new");
  assert.deepEqual(parsePane({ kind: "file", path: "paper.tex", source: null, sessionId: null, ref: null, line: null }), { kind: "file", path: "paper.tex" });
  assert.deepEqual(parsePane({ kind: "experiment", experimentId: "exp", view: "overview", runId: null }), { kind: "experiment", experimentId: "exp", view: "overview" });
});

test("pane validation rejects malformed identifiers, views, line numbers and extra content", () => {
  for (const invalid of [
    null, undefined, [], "files", {}, { kind: "unknown" },
    { kind: "home", view: "settings" },
    { kind: "experiment", experimentId: "", view: "overview" },
    { kind: "experiment", experimentId: "exp", view: "files" },
    { kind: "experiment", experimentId: "exp", view: "terminal", runId: "" },
    { kind: "file", path: "" },
    { kind: "file", path: "file", source: "remote" },
    { kind: "file", path: "file", sessionId: "" },
    { kind: "file", path: "file", ref: "" },
    ...[0, -1, 1.5, Infinity, NaN, Number.MAX_SAFE_INTEGER + 1, "12"].map((line) => ({ kind: "file", path: "file", line })),
    { kind: "code", experimentId: "exp", branch: "", view: "files" },
    { kind: "plan", sessionId: "session", promptId: "" },
    { kind: "subagent", sessionId: "", spawnPartId: "part" },
    ...panes.map((pane) => ({ ...pane, content: "do not persist file or transcript content" })),
  ]) assert.equal(parsePane(invalid), undefined, JSON.stringify(invalid));
});

test("only recognized concrete destinations can be saved for automatic resume", () => {
  assert.deepEqual(parseDestination("/projects/demo"), { kind: "resume", projectId: "demo" });
  assert.deepEqual(parseDestination("/projects/demo/tasks/new"), { kind: "task", projectId: "demo" });
  for (const location of [
    "/projects", "/projects/demo/tasks/new", "/projects/demo/tasks/archived",
    "/projects/demo/skills", ...settingsTabs.map((tab) => `/projects/demo/settings/${tab}`),
    "/projects/demo/settings/%67it", `/projects/demo/tasks/new?%70ane=${encodeURIComponent(JSON.stringify(panes[0]))}&`,
  ]) assert.equal(safeLocation(location), location);
  for (const location of [
    null, undefined, 42, "", "/", "/projects/demo", "/projects/demo/", "/remote-launch",
    "https://example.com/projects", "javascript:alert(1)", "//example.com/projects",
    "/\\example.com/projects", "/projects/../tasks/new", "/projects/%2e%2e/tasks/new",
    "/projects/%2Fexample.com/tasks/new", "/projects/demo/tasks/%5c", "/projects/demo/tasks/%ZZ",
    "/projects/demo/tasks/%00", "/projects/demo/tasks/%7f", "/projects/demo/tasks/%C2%85",
    "/projects/demo/tasks/new\n", "/projects/demo/tasks/new#fragment",
    "/projects/demo/settings/unknown", "/projects/demo/tasks/new/extra",
    "/projects/demo/tasks/new?next=https://example.com", "/projects/demo/tasks/new?pane=not-json",
    "/projects/demo/tasks/new?pane=null", "/projects/demo/tasks/new?pane={}",
    `${taskLocation("demo", null, panes[0])}&pane=${encodeURIComponent(JSON.stringify(panes[1]))}`,
    `${taskLocation("demo", null, panes[0])}?ignored=payload`,
    taskLocation("demo", null, { kind: "home", view: "files", content: "extra" }),
  ]) assert.equal(safeLocation(location), null, String(location));
  const first = emptyProjectWorkspace();
  first.tasks.new = { tabs: [] };
  assert.deepEqual(emptyProjectWorkspace(), { version: 1, lastTaskId: null, lastLocation: null, tasks: {} });
});

test("writer coalesces layout updates and a forced flush cancels their timer", async () => {
  const calls = [];
  const writer = createWorkspaceWriter(async (value, unloading) => calls.push({ value, unloading }), assert.fail);
  writer.queue({ panelWidth: 400 }, 60_000);
  writer.queue({ panelWidth: 500 }, 60_000);
  assert.equal(calls.length, 0);
  await writer.flush();
  assert.deepEqual(calls, [{ value: { panelWidth: 500 }, unloading: false }]);
  await writer.flush();
  await writer.retry();
  assert.equal(calls.length, 1);
});

test("writer serializes saves and keeps the latest pending snapshot during unload", async () => {
  const calls = [];
  const first = Promise.withResolvers();
  const writer = createWorkspaceWriter(async (value, unloading) => {
    calls.push({ value, unloading });
    if (value === "first") await first.promise;
  }, assert.fail);
  writer.queue("first", 60_000);
  const flushing = writer.flush();
  writer.queue("intermediate", 60_000);
  writer.queue("latest", 60_000);
  await writer.flush(true);
  assert.deepEqual(calls, [{ value: "first", unloading: false }]);
  first.resolve();
  await flushing;
  assert.deepEqual(calls, [
    { value: "first", unloading: false }, { value: "latest", unloading: true },
  ]);
});

test("writer reports failed saves, retries them, and never retries over newer state", async () => {
  const calls = [];
  const errors = [];
  const error = new Error("disk unavailable");
  let failing = true;
  const writer = createWorkspaceWriter(async (value) => {
    calls.push(value);
    if (failing) throw error;
  }, (error) => errors.push(error));
  writer.queue("original", 60_000);
  await writer.flush();
  assert.deepEqual(errors, [error]);
  failing = false;
  await writer.retry();
  assert.deepEqual(calls, ["original", "original"]);
  failing = true;
  writer.queue("failed", 60_000);
  await writer.flush();
  writer.queue("newer", 60_000);
  failing = false;
  await writer.retry();
  await writer.retry();
  assert.deepEqual(calls, ["original", "original", "failed", "newer"]);
});

test("failed in-flight save does not discard a queued project snapshot", async () => {
  const first = Promise.withResolvers();
  const calls = [];
  const errors = [];
  const writer = createWorkspaceWriter(async (value) => {
    calls.push(value);
    if (value === "first") await first.promise;
  }, (error) => errors.push(error));
  writer.queue("first", 60_000);
  const flushing = writer.flush();
  writer.queue("latest", 60_000);
  const error = new Error("request failed");
  first.reject(error);
  await flushing;
  assert.deepEqual(calls, ["first", "latest"]);
  assert.deepEqual(errors, [error]);
  await writer.retry();
  assert.deepEqual(calls, ["first", "latest"]);
});

test("retry during a newer in-flight save cannot queue an obsolete failed snapshot", async () => {
  const calls = [];
  const newer = Promise.withResolvers();
  const writer = createWorkspaceWriter(async (value) => {
    calls.push(value);
    if (value === "failed") throw new Error("save failed");
    await newer.promise;
  }, () => {});
  writer.queue("failed", 60_000);
  await writer.flush();
  writer.queue("newer", 60_000);
  const flushing = writer.flush();
  await writer.retry();
  newer.resolve();
  await flushing;
  assert.deepEqual(calls, ["failed", "newer"]);
});

test("project writers remain independent and idle unload sends pending metadata immediately", async () => {
  const calls = [];
  const slow = Promise.withResolvers();
  const first = createWorkspaceWriter(async (value) => { calls.push(["first", value]); await slow.promise; }, assert.fail);
  const second = createWorkspaceWriter(async (value, unloading) => calls.push(["second", value, unloading]), assert.fail);
  first.queue("project-one", 60_000);
  const flushing = first.flush();
  second.queue("project-two", 60_000);
  await second.flush(true);
  assert.deepEqual(calls, [["first", "project-one"], ["second", "project-two", true]]);
  slow.resolve();
  await flushing;
});
