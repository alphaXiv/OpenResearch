import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";
import * as model from "../src/experimentRoutes.ts";
import { tabOpenGestureHandlers } from "../src/tabPreview.ts";

const require = createRequire(import.meta.url);
const labels = { app_routes: "Routes", routes_archived_ancestor: "Archived ancestor", routes_other_task: "From another task", routes_missing_parent: "Parent unavailable", routes_invalid_lineage: "Invalid lineage" };
const messages = { m: new Proxy({}, { get: (_, name) => () => labels[name] ?? String(name) }) };
const compile = (path, mocks) => {
  const source = readFileSync(new URL(path, import.meta.url), "utf8");
  const compiled = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX } }).outputText;
  const exports = {};
  new Function("require", "exports", compiled)((name) => mocks[name] ?? require(name), exports);
  return exports;
};
const { StatusBadge } = compile("../src/components/StatusBadge.tsx", {
  "../paraglide/messages.js": messages,
  "./ui": { StatusIndicator: ({ tone, live, children, className }) => React.createElement("span", { "data-tone": tone, "data-live": live, className }, children) },
});
const { ExperimentRoutes } = compile("../src/components/ExperimentRoutes.tsx", {
  "../paraglide/messages.js": messages,
  "../api": { runDisplayStatus: (run) => run.cancelRequested && ["starting", "running"].includes(run.status) ? "cancelling" : run.status },
  "../experimentRoutes": model,
  "../tabPreview": { tabOpenGestureHandlers },
  "./StatusBadge": { StatusBadge },
  "./WorkspaceEmptyState": { WorkspaceEmptyState: ({ title }) => React.createElement("p", null, title) },
});
const experiments = [
  { id: "root", title: "Baseline", createdAt: 1, chatSessionId: "A", archived: false },
  { id: "bridge", title: "Other task", createdAt: 2, parentExperimentId: "root", chatSessionId: "B", archived: true },
  { id: "child", title: "长名称 — a very long bilingual experiment title", createdAt: 3, parentExperimentId: "bridge", chatSessionId: "A", archived: false },
];
const render = (overrides = {}) => renderToStaticMarkup(React.createElement(ExperimentRoutes, {
  experiments, runs: [{ id: "new", experimentId: "child", status: "done", createdAt: 5 }, { id: "live", experimentId: "child", status: "running", cancelRequested: true, createdAt: 4 }],
  sessionId: "A", showArchived: false, storageKey: "test", onOpen: () => assert.fail("render cannot navigate"), ...overrides,
}));

test("renders real lineage, explicit archive context, accessible levels and status words", () => {
  const html = render();
  assert.match(html, /role="tree" aria-label="Routes"/);
  assert.equal((html.match(/role="treeitem"/g) ?? []).length, 3);
  assert.match(html, /aria-level="3"/);
  assert.equal((html.match(/tabindex="0"/g) ?? []).length, 1);
  assert.match(html, /Archived ancestor/);
  assert.match(html, /长名称 — a very long bilingual experiment title/);
  assert.match(html, /data-tone="caution" data-live="true"[^>]*>status_cancelling/);
  assert.doesNotMatch(html, /status_done/);
});

test("persisted folds omit descendants and expose collapsed state on the parent", () => {
  globalThis.localStorage = { getItem: (key) => key === "test" ? '["bridge"]' : "child" };
  try {
    const html = render();
    assert.equal((html.match(/role="treeitem"/g) ?? []).length, 2);
    assert.match(html, /aria-expanded="false" aria-selected="true"/);
    assert.doesNotMatch(html, /a very long bilingual/);
  } finally { delete globalThis.localStorage; }
});

test("missing/cyclic relationships render explicit labels and empty scopes stay empty", () => {
  const html = render({ experiments: [
    { id: "missing", title: "Imported", parentExperimentId: "gone", createdAt: 1 },
    { id: "self", title: "Legacy", parentExperimentId: "self", createdAt: 2 },
  ], sessionId: null });
  assert.match(html, /Parent unavailable/);
  assert.match(html, /Invalid lineage/);
  assert.doesNotMatch(html, /Baseline/);
  const empty = render({ experiments: [], emptyHint: "No task experiments" });
  assert.match(empty, /No task experiments/);
  assert.doesNotMatch(empty, /role="treeitem"/);
});

test("a deleted persisted selection does not select an unrelated route", () => {
  globalThis.localStorage = { getItem: (key) => key === "test" ? "[]" : "deleted" };
  try { assert.doesNotMatch(render(), /aria-selected="true"/); }
  finally { delete globalThis.localStorage; }
});

test("right toolbar exposes Routes with the shared active and accessible button treatment", () => {
  const { WorkspaceTools } = compile("../src/components/WorkspaceTools.tsx", {
    "../paraglide/messages.js": messages,
    "../api": {}, "../workspaceRuns": { activeWorkspaceRuns: () => [] },
    "../queries/settings": { getComputeSettingsQuery: () => ({}) },
    "../queries/files": {}, "../computeTargets": {}, "./GitDiff": {},
    "./StatusBadge": {}, "./BackendLogos": {},
    "@tanstack/react-query": { useQuery: () => ({}) },
    "./ModelPicker": { usePopover: () => ({ open: false }) },
    "./ui": { IconButton: ({ active, children, ...props }) => React.createElement("button", { ...props, "data-active": active }, children) },
  });
  const html = renderToStaticMarkup(React.createElement(WorkspaceTools, { expanded: false, experiments: [], runs: [], activeView: "routes" }));
  assert.match(html, /aria-label="Routes" aria-pressed="true" data-active="true"/);
});
