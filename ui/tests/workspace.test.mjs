import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
import * as workspace from "../src/workspaceState.ts";

function load(name, dependencies) {
  const code = ts.transpileModule(readFileSync(new URL(`../src/${name}.ts`, import.meta.url), "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  const exports = {};
  new Function("require", "exports", "window", code)((id) => {
    if (id in dependencies) return dependencies[id];
    throw new Error(`Unexpected dependency: ${id}`);
  }, exports, { addEventListener() {}, removeEventListener() {} });
  return exports;
}
const demo = { DEMO_MAIN_SESSION_ID: "demo-main", DEMO_FIGURE_SESSION_ID: "demo-figures", DEMO_LITERATURE_SESSION_ID: "demo-literature", DEMO_OVERVIEW_ARTIFACT: "overview.md" };
const tabs = load("workspaceTabs", { "./api": demo });
const clean = (value) => JSON.parse(JSON.stringify(value));
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};

function host(api) {
  const slots = [];
  let cursor = 0, changed = true, effects = [], layouts = [];
  const same = (a, b) => a && b && a.length === b.length && a.every((value, i) => Object.is(value, b[i]));
  const effect = (queue) => (callback, deps) => {
    const index = cursor++;
    if (same(slots[index]?.deps, deps)) return;
    const previous = slots[index];
    slots[index] = { deps };
    queue.push(() => { previous?.cleanup?.(); slots[index].cleanup = callback(); });
  };
  const react = {
    useState(initial) {
      const index = cursor++;
      if (!(index in slots)) slots[index] = typeof initial === "function" ? initial() : initial;
      return [slots[index], (update) => {
        const next = typeof update === "function" ? update(slots[index]) : update;
        if (!Object.is(next, slots[index])) { slots[index] = next; changed = true; }
      }];
    },
    useRef(initial) { const index = cursor++; slots[index] ??= { current: initial }; return slots[index]; },
    useCallback(callback, deps) {
      const index = cursor++;
      if (!same(slots[index]?.deps, deps)) slots[index] = { callback, deps };
      return slots[index].callback;
    },
    useEffect(callback, deps) { effect(effects)(callback, deps); },
    useLayoutEffect(callback, deps) { effect(layouts)(callback, deps); },
    useSyncExternalStore(subscribe, getSnapshot) { react.useEffect(() => subscribe(() => { changed = true; }), [subscribe]); return getSnapshot(); },
  };
  const hook = load("useProjectWorkspace", { react, "./api": { ...api, isDemoProjectId: () => false }, "./workspaceState": workspace, "./workspaceTabs": tabs });
  const readiness = [];
  let state = tabs.initialRightPaneSessionState();
  let props = { projectId: "p", taskKey: "one", isTask: true, demoOverview: false, location: "/projects/p/tasks/one", pane: undefined, sourceModes: {}, revision: 0 };
  const scroll = {};
  const getScroll = () => scroll;
  const apply = (next, saved, restored) => {
    state = next;
    if (restored) { Object.assign(scroll, saved?.scroll); Object.assign(props.sourceModes, saved?.sourceModes); }
    changed = true;
  };
  let result;
  return {
    hook, readiness, scroll,
    get result() { return result; },
    get state() { return state; },
    navigate(projectId, taskKey, pane) {
      props = { ...props, projectId, taskKey, pane, location: workspace.taskLocation(projectId, taskKey === "new" ? null : taskKey, pane) };
      state = { ...state, rightTab: pane ? tabs.paneTab(pane) : "experiments", panelOpen: Boolean(pane), selectedRunId: pane?.kind === "experiment" ? pane.runId ?? null : null };
      changed = true;
    },
    async flush() {
      for (let round = 0; round < 30; round++) {
        if (changed) {
          changed = false;
          cursor = 0;
          result = hook.useProjectWorkspace({ ...props, state, apply, getScroll });
          readiness.push(result.ready);
          const pendingLayouts = layouts; layouts = []; pendingLayouts.forEach((run) => run());
          const pendingEffects = effects; effects = []; pendingEffects.forEach((run) => run());
        }
        await Promise.resolve();
      }
      await new Promise((resolve) => setTimeout(resolve, 2));
    },
    unmount() { slots.forEach((slot) => slot?.cleanup?.()); },
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
  const panes = [file, code, { kind: "experiment", experimentId: "exp", view: "terminal", runId: "old" }, { kind: "plan", sessionId: "one", promptId: "plan" }, { kind: "subagent", sessionId: "one", spawnPartId: "spawn" }];
  const saved = savedTask(panes);
  saved.previewKey = tabs.rightTabKey(tabs.paneTab(file));
  saved.expanded = { files: ["src"], [tabs.rightTabKey(tabs.paneTab(code))]: ["src/utils"] };
  saved.scroll = { file: { top: 100, left: 8 } }; saved.sourceModes = { file: true }; saved.panelMax = true;
  const restored = tabs.restoreWorkspace(saved, file);
  assert.deepEqual(clean(tabs.rememberWorkspace(restored, saved.scroll, saved.sourceModes)), clean(saved));
  const selected = tabs.applyPane(restored, { ...code, view: "files" });
  assert.equal(selected.fileTabs, restored.fileTabs);
  assert.equal(selected.expTabs, restored.expTabs);
  assert.equal(selected.subagentTabs, restored.subagentTabs);
  assert.deepEqual([...selected.codeTabs[0].toggled], ["src/utils"]);
  const line = tabs.applyPane(restored, { ...file, line: 20 });
  assert.equal(line.fileTabs[0].branchLabel, "experiment-branch");
  assert.equal(line.fileTabs[0].line, 20);
});

test("demo defaults preserve the welcome, figures, and literature workspaces", () => {
  assert.equal(tabs.defaultTaskWorkspace("ordinary", true), undefined);
  assert.equal(tabs.defaultTaskWorkspace(demo.DEMO_MAIN_SESSION_ID, false), undefined);
  assert.deepEqual(tabs.defaultTaskWorkspace(demo.DEMO_MAIN_SESSION_ID, true).active, { kind: "home", view: "experiments" });
  assert.equal(tabs.defaultTaskWorkspace(demo.DEMO_FIGURE_SESSION_ID, false).tabs.length, 4);
  assert.equal(tabs.defaultTaskWorkspace(demo.DEMO_LITERATURE_SESSION_ID, false).tabs[0].path, "nanochat-bottleneck-diagnosis.md");
});

test("delayed hydration cannot save defaults or apply a previous project response", async () => {
  const a = deferred(), b = deferred(), writes = [];
  const app = host({ getProjectUiState: (id) => id === "p" ? a.promise : b.promise, saveProjectUiState: async (...args) => { writes.push(args); } });
  await app.flush();
  assert.equal(app.result.ready, false); assert.equal(writes.length, 0);
  app.navigate("other", "two", undefined); await app.flush();
  b.resolve({ ...workspace.emptyProjectWorkspace(), tasks: { two: savedTask() } }); await app.flush();
  assert.equal(app.result.ready, true); assert.equal(app.state.fileTabs[0].path, "paper.tex");
  a.resolve({ ...workspace.emptyProjectWorkspace(), tasks: { one: savedTask([{ kind: "file", path: "wrong.txt" }]) } }); await app.flush();
  assert.equal(app.state.fileTabs[0].path, "paper.tex");
  assert(writes.every(([id]) => id === "other"));
  app.unmount();
});

test("pane history keeps task readiness and saved tabs; task switches restore the target snapshot", async () => {
  const writes = [];
  const document = { ...workspace.emptyProjectWorkspace(), tasks: { one: savedTask(), two: savedTask([{ kind: "home", view: "artifacts" }]) } };
  const app = host({ getProjectUiState: async () => document, saveProjectUiState: async (...args) => { writes.push(args); } });
  await app.flush();
  assert.equal(app.state.panelOpen, false);
  assert.equal(app.hook.getCachedProjectWorkspace("p").tasks.one.active.kind, "file");
  app.readiness.length = 0;
  app.navigate("p", "one", { ...file, line: 23 }); await app.flush();
  assert(app.readiness.every(Boolean));
  assert.equal(app.state.fileTabs[0].line, 23);
  app.navigate("p", "two", undefined); await app.flush();
  assert.equal(app.state.fileTabs.length, 0); assert.equal(app.state.artifactsTabOpen, true);
  app.navigate("p", "one", undefined); await app.flush();
  assert.equal(app.state.fileTabs[0].line, 23);
  assert.equal(app.state.panelOpen, false);
  assert.equal(app.hook.getCachedProjectWorkspace("p").tasks.one.active.line, 23);
  assert(writes.length > 0);
  app.unmount();
});

test("failed hydration stays read-only until a successful retry", async () => {
  let fail = true;
  const writes = [];
  const app = host({ getProjectUiState: async () => { if (fail) throw new Error("offline"); return { ...workspace.emptyProjectWorkspace(), tasks: { one: savedTask() } }; }, saveProjectUiState: async (...args) => { writes.push(args); } });
  await app.flush();
  assert.equal(app.result.error, "offline"); assert.equal(app.result.ready, false); assert.equal(writes.length, 0);
  fail = false; app.result.retry(); await app.flush();
  assert.equal(app.result.error, null); assert.equal(app.result.ready, true); assert.equal(app.state.fileTabs[0].path, "paper.tex");
  app.unmount();
});


test("creating a task moves its workspace and retains an explicitly closed pane", async () => {
  const key = tabs.fileScrollKey("p", null, tabs.paneTab(file));
  const saved = savedTask(); saved.scroll[key] = { top: 75, left: 0 }; saved.sourceModes[key] = true;
  const app = host({ getProjectUiState: async () => ({ ...workspace.emptyProjectWorkspace(), tasks: { new: saved } }), saveProjectUiState: async () => {} });
  app.navigate("p", "new", undefined); await app.flush();
  app.scroll[key] = { top: 500, left: 2 };
  app.result.capture();
  app.hook.inheritNewTaskWorkspace("p", "created");
  app.navigate("p", "created", undefined); await app.flush();
  const document = app.hook.getCachedProjectWorkspace("p");
  const newKey = tabs.fileScrollKey("p", "created", tabs.paneTab(file));
  assert.equal(document.tasks.new, undefined);
  assert.equal(document.lastTaskId, "created");
  assert.equal(document.lastLocation, "/projects/p/tasks/created");
  assert.equal(app.state.panelOpen, false);
  assert.equal(document.tasks.created.scroll[newKey].top, 500);
  assert.equal(document.tasks.created.sourceModes[newKey], true);
  app.unmount();
});

test("unmount captures scroll changes that are still awaiting their debounce", async () => {
  const writes = [];
  const app = host({ getProjectUiState: async () => ({ ...workspace.emptyProjectWorkspace(), tasks: { one: savedTask() } }), saveProjectUiState: async (id, state) => { writes.push(state); } });
  await app.flush();
  const key = tabs.fileScrollKey("p", "one", tabs.paneTab(file));
  app.scroll[key] = { top: 500, left: 2 };
  app.unmount();
  assert.equal(writes.at(-1).tasks.one.scroll[key].top, 500);
});

test("changing runtimes drops queued writes from the previous database", async () => {
  const first = deferred(), writes = [];
  const app = host({ getProjectUiState: async () => ({ ...workspace.emptyProjectWorkspace(), tasks: { one: savedTask() } }), saveProjectUiState: async (id, state) => { writes.push(state); await first.promise; } });
  await app.flush();
  app.navigate("p", "one", file); await app.flush();
  assert.equal(writes.length, 1);
  app.hook.clearProjectWorkspaceCache();
  first.resolve(); await app.flush();
  assert.equal(writes.length, 1);
  assert.equal(app.hook.getCachedProjectWorkspace("p"), undefined);
  app.unmount();
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
    const app = host({ getProjectUiState: async () => workspace.emptyProjectWorkspace(), saveProjectUiState: async () => {} });
    app.navigate("p", key, undefined);
    await app.flush();
    assert.equal(app.result.ready, true);
    assert.equal(app.state.fileTabs.length, 0);
    app.unmount();
  }
});


test("a run baseline resolving after project unmount cannot navigate", async () => {
  const source = ts.createSourceFile("App.tsx", readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let baseline, projectEffect;
  function visit(node) {
    if (ts.isVariableDeclaration(node) && node.name.getText(source) === "loadRunsBaseline") baseline = node.initializer.arguments[0];
    if (ts.isCallExpression(node) && node.expression.getText(source) === "useEffect" && node.arguments[0]?.getText(source).includes("loadRunsBaseline(projectId);")) projectEffect = node.arguments[0];
    ts.forEachChild(node, visit);
  }
  visit(source);
  const response = deferred();
  let navigations = 0;
  const context = {
    projectId: "p", projectIdRef: { current: "p" }, observedRunsProjectRef: { current: "p" },
    runsVisitRef: { current: 0 }, runsBaselineReadyRef: { current: false },
    baselineRunsRef: { current: new Map() }, pendingFirstRunningRunsRef: { current: new Map() },
    observedRunsRef: { current: new Map() }, liveRunIdsRef: { current: new Set() },
    listRuns: () => response.promise, listExperiments: async () => [], getArtifacts: async () => [], openProject: async () => {},
    setExperiments() {}, setRuns() {}, setArtifacts() {}, setRunDataReady() {}, setExperimentDataReady() {},
    openExperimentsTab: () => { navigations++; },
  };
  function evaluate(node) {
    assert(node);
    const code = ts.transpileModule(`const callback = ${node.getText(source)};`, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
    return new Function(...Object.keys(context), `${code}; return callback;`)(...Object.values(context));
  }
  context.loadRunsBaseline = evaluate(baseline);
  const cleanup = evaluate(projectEffect)();
  context.pendingFirstRunningRunsRef.current.set("run", { id: "run", status: "running", updatedAt: "now" });
  cleanup();
  response.resolve([]);
  await response.promise;
  await Promise.resolve();
  assert.equal(navigations, 0);
  assert.equal(context.runsBaselineReadyRef.current, false);
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
  const source = ts.createSourceFile("App.tsx", readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let callback;
  function visit(node) {
    if (ts.isVariableDeclaration(node) && node.name.getText(source) === "loadSessionIds") callback = node.initializer.arguments[0];
    ts.forEachChild(node, visit);
  }
  visit(source);
  assert(callback);
  const response = deferred();
  let ids = ["old", "deleted-offline"];
  const sessionLoadRef = { current: null }, rememberedSessionRef = { current: "deleted-offline" };
  const code = ts.transpileModule(`const callback = ${callback.getText(source)};`, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  const load = new Function("listChatSessions", "projectId", "sessionLoadRef", "setSessions", "rememberedSessionRef", `${code}; return callback;`)(() => response.promise, "p", sessionLoadRef, (next) => { ids = next; }, rememberedSessionRef);
  const pending = load();
  sessionLoadRef.current.set("created-live", true);
  sessionLoadRef.current.set("deleted-live", false);
  response.resolve([{ id: "old" }, { id: "deleted-live" }]);
  await pending;
  assert.deepEqual(ids, ["old", "created-live"]);
  assert.equal(rememberedSessionRef.current, null);
  assert.equal(sessionLoadRef.current, null);
});


test("code destinations cannot silently substitute another experiment branch", () => {
  const source = ts.createSourceFile("App.tsx", readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let expression;
  function visit(node) {
    if (ts.isVariableDeclaration(node) && node.name.getText(source) === "codeExperiment") expression = node.initializer;
    ts.forEachChild(node, visit);
  }
  visit(source);
  assert(expression);
  const resolve = new Function("codeTab", "experiments", `return ${expression.getText(source)};`);
  const experiment = { id: "exp", branchName: "expected" };
  assert.equal(resolve({ experimentId: "exp", branch: "wrong" }, [experiment]), null);
  assert.equal(resolve({ experimentId: "exp", branch: "expected" }, [experiment]), experiment);
  assert.equal(resolve({ experimentId: "missing", branch: "expected" }, [experiment]), null);
});
