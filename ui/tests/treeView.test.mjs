import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import ts from "typescript";
import { initialExperimentTreeState } from "../src/workspaceState.ts";

const require = createRequire(import.meta.url);
const source = readFileSync(new URL("../src/components/TreeView.tsx", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
}).outputText;

// Run the tree's hooks and effects until state settles, without mounting the canvas.
function mountTree(overrides = {}) {
  const hooks = [];
  let cursor, effects, changed;
  const slot = (init) => {
    const index = cursor++;
    if (!(index in hooks)) hooks[index] = init();
    return hooks[index];
  };
  const sameDeps = (a, b) => a && b && a.length === b.length && a.every((value, index) => Object.is(value, b[index]));
  const memo = (factory, deps) => {
    const hook = slot(() => ({}));
    if (!sameDeps(hook.deps, deps)) {
      hook.value = factory();
      hook.deps = deps;
    }
    return hook.value;
  };
  const react = {
    ...React,
    useState: (initial) => {
      const hook = slot(() => {
        const state = { value: typeof initial === "function" ? initial() : initial };
        state.set = (next) => {
          const value = typeof next === "function" ? next(state.value) : next;
          changed ||= !Object.is(value, state.value);
          state.value = value;
        };
        return state;
      });
      return [hook.value, hook.set];
    },
    useRef: (initial) => slot(() => ({ current: initial })),
    useMemo: memo,
    useCallback: (callback, deps) => memo(() => callback, deps),
    useEffect: (effect, deps) => {
      const hook = slot(() => ({}));
      if (!sameDeps(hook.deps, deps)) {
        hook.deps = deps;
        effects.push(effect);
      }
    },
  };
  const mocks = {
    react,
    "../paraglide/messages.js": { m: new Proxy({}, { get: (_, name) => () => String(name) }) },
    "../i18n": {},
    "../locale": {},
    "../api": { runDisplayStatus: (run) => run.status },
    "../tabPreview": {},
    "./BackendLogos": {},
    "./ExpHoverCard": { dismissTreeHoverCards: () => {} },
    "./ArchiveMenu": {},
    "./ModelPicker": {},
    "./ui": {},
    "./StatusBadge": {},
    "./TreeInspector": {},
  };
  const exports = {};
  new Function("require", "exports", compiled)((name) => mocks[name] ?? require(name), exports);
  const props = {
    experiments: [
      { id: "baseline", slug: "baseline", createdAt: 1, parentExperimentId: null },
      { id: "trial", slug: "trial", createdAt: 2, parentExperimentId: "baseline" },
    ],
    runs: [{ id: "run", experimentId: "trial", status: "running", createdAt: 3 }],
    archiveActions: new Map(),
    showArchived: false,
    project: {},
    agentSessionId: null,
    viewport: null,
    onOpenView: () => {},
    onOpenCode: () => {},
    onArchive: () => {},
    onShowProjectScope: () => {},
    onRestoreRegion: () => {},
    state: initialExperimentTreeState(),
    onStateChange: (update) => {
      props.state = typeof update === "function" ? update(props.state) : update;
      changed = true;
    },
    onViewportChange: (viewport) => { props.viewport = viewport; changed = true; },
    onCancel: async () => {},
    ...overrides,
  };
  return {
    props,
    render: () => {
      for (let attempt = 0; attempt < 20; attempt++) {
        cursor = 0;
        effects = [];
        changed = false;
        const tree = exports.TreeView(props);
        effects.forEach((effect) => effect());
        if (!changed) return tree;
      }
      assert.fail("TreeView effects did not settle (maximum update depth)");
    },
  };
}

const canvas = (tree) => tree.props.children.props.children.props;
const trial = (view) => view.nodes.find((node) => node.id === "trial");

test("tree mount settles with cancellation enabled, including an empty project", () => {
  const tree = mountTree();
  const view = canvas(tree.render());
  assert.equal(view.nodes.length, 2);
  assert.equal(typeof trial(view).data.onCancel, "function");
  assert.equal(canvas(tree.render()).nodes, view.nodes);
  mountTree({ experiments: [] }).render();
});

test("tree can enable, replace and disable its cancellation handler", async () => {
  const tree = mountTree({ onCancel: undefined });
  assert.equal(trial(canvas(tree.render())).data.onCancel, undefined);
  const calls = [];
  tree.props.onCancel = async (id) => calls.push(["first", id]);
  await trial(canvas(tree.render())).data.onCancel("run");
  tree.props.onCancel = async (id) => calls.push(["replacement", id]);
  await trial(canvas(tree.render())).data.onCancel("run");
  assert.deepEqual(calls, [["first", "run"], ["replacement", "run"]]);
  tree.props.onCancel = undefined;
  assert.equal(trial(canvas(tree.render())).data.onCancel, undefined);
});

test("cancellation updates settle and preserve dragged node positions", async () => {
  let finish;
  const calls = [];
  const tree = mountTree({ onCancel: (id) => {
    calls.push(id);
    return new Promise((resolve) => { finish = resolve; });
  } });
  const position = { x: 500, y: 300 };
  canvas(tree.render()).onNodesChange([{ type: "position", id: "trial", position, dragging: false }]);
  const cancelling = trial(canvas(tree.render())).data.onCancel("run");
  const pending = trial(canvas(tree.render()));
  assert.equal(pending.data.cancelling, true);
  assert.deepEqual(pending.position, position);
  assert.deepEqual(calls, ["run"]);
  finish();
  await cancelling;
  tree.props.runs = [{ ...tree.props.runs[0], status: "cancelled" }];
  const stopped = trial(canvas(tree.render()));
  assert.equal(stopped.data.liveRun, null);
  assert.equal(stopped.data.cancelling, false);
  assert.deepEqual(stopped.position, position);
});

test("failed cancellation settles, displays the error and allows retry", async () => {
  let calls = 0;
  const tree = mountTree({ onCancel: async () => {
    if (++calls === 1) throw new Error("stop failed");
  } });
  await trial(canvas(tree.render())).data.onCancel("run");
  const failed = canvas(tree.render());
  assert.equal(failed.cancelError, "stop failed");
  assert.equal(trial(failed).data.cancelling, false);
  await trial(failed).data.onCancel("run");
  const retried = canvas(tree.render());
  assert.equal(retried.cancelError, null);
  assert.equal(trial(retried).data.cancelling, true);
  assert.equal(calls, 2);
});

test("drag commits survive a remount and data refreshes during dragging", () => {
  const tree = mountTree();
  const position = { x: -400, y: 850 };
  canvas(tree.render()).onNodesChange([{ type: "position", id: "trial", position, dragging: true }]);
  assert.deepEqual(tree.props.state.positions, {});
  tree.props.runs = [{ ...tree.props.runs[0], updatedAt: 50 }];
  assert.deepEqual(trial(canvas(tree.render())).position, position);
  canvas(tree.render()).onNodesChange([{ type: "position", id: "trial", position, dragging: false }]);
  assert.deepEqual(tree.props.state.positions, { trial: position });
  const reopened = mountTree({ state: JSON.parse(JSON.stringify(tree.props.state)) });
  assert.deepEqual(trial(canvas(reopened.render())).position, position);
  canvas(reopened.render()).onResetLayout();
  assert.deepEqual(reopened.props.state.positions, {});
  assert.notDeepEqual(trial(canvas(reopened.render())).position, position);
});

test("search, status, minimap, selection, inspector tab and programmatic viewport are remembered", () => {
  const tree = mountTree();
  canvas(tree.render()).onSearchChange("trial");
  canvas(tree.render()).onStatusFilterChange("active");
  canvas(tree.render()).onToggleMiniMap();
  trial(canvas(tree.render())).data.onSelect();
  canvas(tree.render()).children.props.onTabChange("runs");
  const viewport = { x: -200, y: 140, zoom: 0.85 };
  canvas(tree.render()).onMoveEnd(null, viewport);
  const saved = JSON.parse(JSON.stringify({ state: tree.props.state, viewport: tree.props.viewport }));
  const reopened = canvas(mountTree(saved).render());
  assert.equal(reopened.search, "trial");
  assert.equal(reopened.statusFilter, "active");
  assert.equal(reopened.showMiniMap, true);
  assert.equal(trial(reopened).data.isSelected, true);
  assert.equal(reopened.children.props.tab, "runs");
  assert.deepEqual(reopened.viewport, viewport);
});
