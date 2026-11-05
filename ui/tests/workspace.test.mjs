import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement, useRef, useState } from "react";
import * as demo from "../src/api.ts";
import * as live from "../src/queries/live.ts";
import { createWorkspace } from "../src/workspace.tsx";
import * as workspace from "../src/workspaceState.ts";
import * as tabs from "../src/workspaceTabs.ts";
import { m } from "../src/paraglide/messages.js";
import { navigate, render, renderApp, serve, serveApp, settle, streams } from "./render.mjs";

const clean = (value) => JSON.parse(JSON.stringify(value));
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};

/** Mounts useProjectWorkspace the way a project page does, over a backend whose project layouts `load` reads and `save` stores. */
async function host({ load, save = async () => {} }) {
  const local = createWorkspace("local");
  serve(async (path, init) => {
    const id = decodeURIComponent(path.match(/^\/api\/projects\/([^/]+)\/ui-state$/)?.[1] ?? "");
    if (!id) return;
    if (init.method !== "POST") return load(id);
    const state = JSON.parse(init.body);
    await save(id, state);
    return state;
  });
  const view = { readiness: [], scroll: {}, workspace: local, projects: local.projects };
  function Host() {
    const [world, setWorld] = useState(() => ({
      props: { projectId: "p", taskKey: "one", isTask: true, firstDemoOpen: false, location: "/projects/p/tasks/one", pane: undefined, sourceModes: {}, revision: 0 },
      state: tabs.initialRightPaneSessionState(),
    }));
    const apply = useRef((next, saved, restored) => setWorld((current) => {
      if (restored) { Object.assign(view.scroll, saved?.scroll); Object.assign(current.props.sourceModes, saved?.sourceModes); }
      return { ...current, state: next };
    })).current;
    view.setWorld = setWorld;
    view.state = world.state;
    view.result = local.projects.useProjectWorkspace({ ...world.props, state: world.state, apply, getScroll: () => view.scroll });
    view.readiness.push(view.result.ready);
    return null;
  }
  const { unmount } = await render(createElement(Host), local);
  await settle();
  return {
    ...view,
    get result() { return view.result; },
    get state() { return view.state; },
    async navigate(projectId, taskKey, pane) {
      await act(() => view.setWorld(({ props, state }) => ({
        props: { ...props, projectId, taskKey, pane, location: workspace.taskLocation(projectId, taskKey === "new" ? null : taskKey, pane) },
        state: { ...state, rightTab: pane ? tabs.paneTab(pane) : "experiments", panelOpen: Boolean(pane), selectedRunId: pane?.kind === "experiment" ? pane.runId ?? null : null },
      })));
      await settle();
    },
    unmount,
  };
}

const file = { kind: "file", path: "paper.tex", branchLabel: "experiment-branch", line: 8 };
const code = { kind: "code", experimentId: "exp", branch: "experiment-branch", view: "changes" };
function savedTask(panes = [file, code]) {
  let state = panes.reduce((current, pane) => tabs.applyPane(current, pane), tabs.initialRightPaneSessionState());
  state = tabs.applyPane(state, panes[0]);
  state.rightTab = tabs.paneTab(panes[0]); state.panelOpen = true;
  return tabs.rememberWorkspace(state, {}, {});
}

test("all tab variants retain ordering, preview, history, expansion and view metadata", () => {
  const panes = [file, code, { kind: "experiment", experimentId: "exp", view: "terminal", runId: "old" }, { kind: "plan", sessionId: "one", promptId: "plan" }, { kind: "subagent", sessionId: "one", spawnPartId: "spawn" }, { kind: "side", sessionId: "two" }, { kind: "side", sessionId: null }];
  const saved = savedTask(panes);
  saved.previewKey = tabs.rightTabKey(tabs.paneTab(file));
  saved.expanded = { files: ["src"], [tabs.rightTabKey(tabs.paneTab(code))]: ["src/utils"] };
  saved.scroll = { file: { top: 100, left: 8 } }; saved.sourceModes = { file: true }; saved.panelMax = true;
  saved.treeViewport = { x: 24, y: -18, zoom: 1.4 };
  const restored = tabs.restoreWorkspace(saved, file);
  assert.deepEqual(clean(tabs.rememberWorkspace(restored, saved.scroll, saved.sourceModes)), clean(saved));
  const selected = tabs.applyPane(restored, { ...code, view: "files" });
  assert.equal(selected.fileTabs, restored.fileTabs);
  assert.equal(selected.expTabs, restored.expTabs);
  assert.equal(selected.subagentTabs, restored.subagentTabs);
  assert.equal(selected.sideTabs, restored.sideTabs);
  assert.deepEqual([...selected.codeTabs[0].toggled], ["src/utils"]);
  const line = tabs.applyPane(restored, { ...file, line: 20 });
  assert.equal(line.fileTabs[0].branchLabel, "experiment-branch");
  assert.equal(line.fileTabs[0].line, 20);
});

test("demo defaults open the welcome pane and keep figures and literature closed", () => {
  assert.equal(tabs.defaultTaskWorkspace("ordinary", true), undefined);
  assert.equal(tabs.defaultTaskWorkspace(demo.DEMO_MAIN_SESSION_ID, false), undefined);
  assert.deepEqual(tabs.defaultTaskWorkspace(demo.DEMO_MAIN_SESSION_ID, true).active, { kind: "home", view: "experiments" });
  assert.equal(tabs.defaultTaskWorkspace(demo.DEMO_FIGURE_SESSION_ID, false), undefined);
  assert.equal(tabs.initialRightPaneSessionState(demo.DEMO_FIGURE_SESSION_ID, true).panelOpen, false);
  assert.equal(tabs.defaultTaskWorkspace(demo.DEMO_LITERATURE_SESSION_ID, false), undefined);
});

test("delayed hydration cannot save defaults or apply a previous project response", async () => {
  const a = deferred(), b = deferred(), writes = [];
  const app = await host({ load: (id) => id === "p" ? a.promise : b.promise, save: async (...args) => { writes.push(args); } });
  assert.equal(app.result.ready, false); assert.equal(writes.length, 0);
  await app.navigate("other", "two", undefined);
  b.resolve({ ...workspace.emptyProjectWorkspace(), tasks: { two: savedTask() } }); await settle();
  assert.equal(app.result.ready, true); assert.equal(app.state.fileTabs[0].path, "paper.tex");
  a.resolve({ ...workspace.emptyProjectWorkspace(), tasks: { one: savedTask([{ kind: "file", path: "wrong.txt" }]) } }); await settle();
  assert.equal(app.state.fileTabs[0].path, "paper.tex");
  assert(writes.every(([id]) => id === "other"));
  await app.unmount();
});

test("pane history keeps task readiness and saved tabs; task switches restore the target snapshot", async () => {
  const writes = [];
  const document = { ...workspace.emptyProjectWorkspace(), tasks: { one: savedTask(), two: savedTask([{ kind: "home", view: "artifacts" }, { kind: "home", view: "terminal" }]) } };
  const app = await host({ load: () => document, save: async (...args) => { writes.push(args); } });
  assert.equal(app.state.panelOpen, false);
  assert.equal(app.projects.getCachedProjectWorkspace("p").tasks.one.active.kind, "file");
  app.readiness.length = 0;
  await app.navigate("p", "one", { ...file, line: 23 });
  assert(app.readiness.every(Boolean));
  assert.equal(app.state.fileTabs[0].line, 23);
  await app.navigate("p", "two", undefined);
  assert.equal(app.state.fileTabs.length, 0); assert.equal(app.state.artifactsTabOpen, true); assert.equal(app.state.terminalTabOpen, true);
  await app.navigate("p", "one", undefined);
  assert.equal(app.state.fileTabs[0].line, 23);
  assert.equal(app.state.panelOpen, false);
  assert.equal(app.projects.getCachedProjectWorkspace("p").tasks.one.active.line, 23);
  assert(writes.length > 0);
  await app.unmount();
});

test("failed hydration stays read-only until a successful retry", async () => {
  let fail = true;
  const writes = [];
  const app = await host({ load: () => fail ? Response.json({ error: "offline" }, { status: 503 }) : { ...workspace.emptyProjectWorkspace(), tasks: { one: savedTask() } }, save: async (...args) => { writes.push(args); } });
  assert.equal(app.result.error, "offline"); assert.equal(app.result.ready, false); assert.equal(writes.length, 0);
  fail = false; await act(() => app.result.retry()); await settle();
  assert.equal(app.result.error, null); assert.equal(app.result.ready, true); assert.equal(app.state.fileTabs[0].path, "paper.tex");
  await app.unmount();
});


test("creating a task moves its workspace and retains an explicitly closed pane", async () => {
  const key = tabs.fileScrollKey("p", null, tabs.paneTab(file));
  const saved = savedTask(); saved.scroll[key] = { top: 75, left: 0 }; saved.sourceModes[key] = true;
  const app = await host({ load: () => ({ ...workspace.emptyProjectWorkspace(), tasks: { new: saved } }) });
  await app.navigate("p", "new", undefined);
  app.scroll[key] = { top: 500, left: 2 };
  app.result.capture();
  app.projects.inheritNewTaskWorkspace("p", "created");
  await app.navigate("p", "created", undefined);
  const document = app.projects.getCachedProjectWorkspace("p");
  const newKey = tabs.fileScrollKey("p", "created", tabs.paneTab(file));
  assert.equal(document.tasks.new, undefined);
  assert.equal(document.lastTaskId, "created");
  assert.equal(document.lastLocation, "/projects/p/tasks/created");
  assert.equal(app.state.panelOpen, false);
  assert.equal(document.tasks.created.scroll[newKey].top, 500);
  assert.equal(document.tasks.created.sourceModes[newKey], true);
  await app.unmount();
});

test("unmount captures scroll changes that are still awaiting their debounce", async () => {
  const writes = [];
  const app = await host({ load: () => ({ ...workspace.emptyProjectWorkspace(), tasks: { one: savedTask() } }), save: async (id, state) => { writes.push(state); } });
  const key = tabs.fileScrollKey("p", "one", tabs.paneTab(file));
  app.scroll[key] = { top: 500, left: 2 };
  await app.unmount();
  await settle();
  assert.equal(writes.at(-1).tasks.one.scroll[key].top, 500);
});

test("changing runtimes drops queued writes from the previous database", async () => {
  const first = deferred(), writes = [];
  const app = await host({ load: () => ({ ...workspace.emptyProjectWorkspace(), tasks: { one: savedTask() } }), save: async (id, state) => { writes.push(state); await first.promise; } });
  await app.navigate("p", "one", file);
  assert.equal(writes.length, 1);
  app.workspace.replaceWorkspace();
  first.resolve(); await settle();
  assert.equal(writes.length, 1);
  assert.equal(app.projects.getCachedProjectWorkspace("p"), undefined);
  await app.unmount();
});


test("selected run descriptors survive tab-close fallback history", () => {
  const overview = { kind: "experiment", experimentId: "exp", view: "overview" };
  const pinned = { ...overview, runId: "older-run" };
  let state = tabs.applyPane(tabs.initialRightPaneSessionState(), overview);
  state = tabs.applyPane(state, pinned);
  state = tabs.applyPane(state, file);
  const history = state.tabHistory.filter((tab) => tabs.rightTabKey(tab) !== tabs.rightTabKey(tabs.paneTab(file)));
  assert.deepEqual(tabs.tabPane(history.at(-1)), pinned);
  const cleared = tabs.applyPane(state, overview);
  assert.equal(tabs.tabPane(cleared.tabHistory.at(-1)).runId, undefined);
});

test("prototype names in task URLs cannot hydrate inherited object values", async () => {
  for (const key of ["constructor", "__proto__", "toString"]) {
    assert.equal(workspace.getTaskWorkspace(workspace.emptyProjectWorkspace(), key), undefined);
    const app = await host({ load: () => workspace.emptyProjectWorkspace() });
    await app.navigate("p", key, undefined);
    assert.equal(app.result.ready, true);
    assert.equal(app.state.fileTabs.length, 0);
    await app.unmount();
  }
});


test("deep-linked tabs participate in close fallback after hydration", () => {
  let state = tabs.restoreWorkspace(undefined, file);
  state = tabs.applyPane(state, code);
  const fallback = state.tabHistory.filter((tab) => tabs.rightTabKey(tab) !== tabs.rightTabKey(tabs.paneTab(code))).at(-1);
  assert.deepEqual(clean(tabs.tabPane(fallback)), file);
  const saved = savedTask([file, code]);
  assert.equal(tabs.rightTabKey(tabs.restoreWorkspace(saved, code).tabHistory.at(-1)), tabs.rightTabKey(tabs.paneTab(code)));
  assert.deepEqual(tabs.restoreWorkspace(saved, undefined).tabHistory.map(tabs.rightTabKey), saved.history);
});

test("session refresh removes missing tasks while preserving concurrent live events", async () => {
  const requests = [deferred(), deferred()];
  let calls = 0;
  serve((path) => path === "/api/chat/sessions?projectId=p" ? requests[calls++].promise.then((sessions) => ({ sessions })) : undefined);
  const { queryClient, queries } = createWorkspace("local");
  const options = queries.listChatSessionsQuery("p");
  queryClient.setQueryData(options.queryKey, [{ id: "old" }, { id: "deleted-offline" }]);
  const read = queryClient.fetchQuery({ ...options, staleTime: 0 });
  live.markLiveUpdate(queryClient, options.queryKey, "created-live");
  requests[0].resolve([{ id: "old" }]);
  for (let i = 0; i < 20; i++) await Promise.resolve();
  live.markLiveUpdate(queryClient, options.queryKey, "created-live");
  live.markLiveUpdate(queryClient, options.queryKey, "deleted-live");
  queryClient.setQueryData(options.queryKey, [{ id: "created-live" }, { id: "old" }]);
  requests[1].resolve([{ id: "old" }, { id: "deleted-live" }]);
  assert.deepEqual((await read).map((row) => row.id), ["created-live", "old"]);
  queryClient.clear();
});

/** The project chat page over a backend whose experiments and run list are these; `emit` sends a server event. */
async function projectPage({ experiments = [], runs = () => [] } = {}) {
  serveApp({
    "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project" }] }),
    "GET /api/projects/p/experiments": () => ({ experiments }),
    "GET /api/projects/p/runs": async () => ({ runs: await runs() }),
  });
  const view = await renderApp("/projects/p/tasks/new");
  return {
    ...view,
    pane: () => JSON.parse(new URLSearchParams(location.search).get("pane")),
    content: () => document.querySelector(".right-pane [data-workspace-content]")?.textContent ?? null,
    async emit(type, data) {
      await act(async () => streams[0].dispatchEvent(new MessageEvent(type, { data: JSON.stringify(data) })));
      await settle();
    },
    async closePanel() {
      await navigate("/projects/p/tasks/new");
    },
  };
}

const logs = (runId, text = "log") => ({ runId, dataBase64: btoa(text), offset: 0 });
const runningRun = (id, projectId = "p") => ({ id, projectId, experimentId: "exp", status: "running", createdAt: 1, updatedAt: 2 });
const runLogsPane = (runId) => ({ kind: "experiment", experimentId: "exp", view: "terminal", runId });

test("a newly running experiment opens its logs only on its first output", async () => {
  const app = await projectPage({ runs: () => [runningRun("historical")] });
  await app.emit("run.updated", { run: runningRun("new") });
  assert.equal(app.pane(), null);
  await app.emit("run.log", logs("new", ""));
  assert.equal(app.pane(), null);
  await app.emit("run.log", logs("new"));
  assert.deepEqual(app.pane(), runLogsPane("new"));

  await app.closePanel();
  await app.emit("run.log", logs("new", "more"));
  await app.emit("run.updated", { run: runningRun("historical") });
  await app.emit("run.log", logs("historical"));
  await app.emit("run.log", logs("unknown"));
  await app.emit("run.updated", { run: runningRun("other-project", "other") });
  await app.emit("run.log", logs("other-project"));
  assert.equal(app.pane(), null);

  await app.emit("run.log", logs("early-log"));
  await app.emit("run.updated", { run: runningRun("early-log") });
  assert.deepEqual(app.pane(), runLogsPane("early-log"));
});

test("a run baseline resolving after the project page unmounts cannot navigate", async () => {
  const baseline = deferred();
  const app = await projectPage({ runs: () => baseline.promise });
  await app.emit("run.updated", { run: runningRun("new") });
  await app.emit("run.log", logs("new"));
  assert.equal(app.pane(), null);
  await app.unmount();
  baseline.resolve([]);
  await settle();
  assert.equal(location.pathname + location.search, "/projects/p/tasks/new");
});

test("code destinations cannot silently substitute another experiment branch", async () => {
  const unavailable = m.model_picker_unavailable();
  for (const [experimentId, branch, available] of [["exp", "wrong", false], ["exp", "expected", true], ["missing", "expected", false]]) {
    const app = await projectPage({ experiments: [{ id: "exp", projectId: "p", slug: "exp", branchName: "expected", runCommand: "", agentStatus: "", createdAt: 1, updatedAt: 1, archived: false }] });
    await navigate(workspace.taskLocation("p", null, { kind: "code", experimentId, branch, view: "changes" }));
    assert.equal(app.content().includes(unavailable), !available, `${experimentId}@${branch}`);
    await app.unmount();
  }
});
