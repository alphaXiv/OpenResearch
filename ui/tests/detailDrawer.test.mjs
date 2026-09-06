import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";

const require = createRequire(import.meta.url);
const source = readFileSync(new URL("../src/components/DetailDrawer.tsx", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX },
}).outputText;
const mocks = {
  "../paraglide/messages.js": { m: new Proxy({}, { get: (_, name) => () => String(name) }) },
  "../api": { runDisplayStatus: () => "complete", timeAgo: () => "now" },
  "./ExperimentOverview": { ExperimentOverview: () => null },
  "./LogTerminal": { LogTerminal: ({ runId }) => `LOG:${runId}` },
  "./StatusBadge": { StatusBadge: () => null },
  "./ui": { Button: ({ children }) => React.createElement("button", null, children) },
  "lucide-react": { ChevronDown: () => null, CircleStop: () => null },
};
const exports = {};
new Function("require", "exports", compiled)((name) => mocks[name] ?? require(name), exports);
const props = {
  experiment: { id: "experiment-a", slug: "example" },
  project: {},
  view: "terminal",
  runs: [
    { id: "run-old", experimentId: "experiment-a", createdAt: 1, status: "completed" },
    { id: "run-new", experimentId: "experiment-a", createdAt: 2, status: "completed" },
    { id: "foreign-run", experimentId: "experiment-b", createdAt: 3, status: "completed" },
  ],
  onSelectRun: () => assert.fail("render must not navigate"),
};
const render = (selectedRunId) => renderToStaticMarkup(React.createElement(exports.DetailDrawer, { ...props, selectedRunId }));

test("terminal uses newest only when the URL omits a run", () => {
  assert.match(render(null), /LOG:run-new/);
  const explicit = render("run-old");
  assert.match(explicit, /LOG:run-old/);
  assert.doesNotMatch(explicit, /LOG:run-new/);
});

test("missing and foreign explicit runs render unavailable without substituting another run", () => {
  for (const id of ["missing-run", "foreign-run"]) {
    const html = render(id);
    assert.doesNotMatch(html, /LOG:/);
    assert.match(html, /model_picker_unavailable/);
  }
});
