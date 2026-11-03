import { withWorkspaceMocks } from "./workspaceMocks.mjs";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";

const require = createRequire(import.meta.url);

function loadRunDisplay() {
  const code = ts.transpileModule(readFileSync(new URL("../src/runDisplay.ts", import.meta.url), "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  const exports = {};
  new Function("exports", code)(exports);
  return exports;
}
const source = readFileSync(new URL("../src/components/DetailDrawer.tsx", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX },
}).outputText;
let trackioUrl = null;
const mocks = {
  "../paraglide/messages.js": { m: new Proxy({}, { get: (_, name) => () => String(name) }) },
  "../api": { timeAgo: () => "now", trackioRunUrl: () => trackioUrl },
  "../runDisplay": loadRunDisplay(),
  "../queries/settings": { useTrackioRuns: () => ({ data: undefined }) },
  "../tabPreview": { tabOpenGestureHandlers: () => ({}) },
  "../theme": { useResolvedTheme: () => "light" },
  "./ExperimentOverview": { ExperimentOverview: () => null },
  "./LogTerminal": { LogTerminal: ({ run }) => `LOG:${run.id}` },
  "./RunContactNotice": { RunContactNotice: ({ run }) => `NOTICE:${run.id}` },
  "./StatusBadge": { StatusBadge: () => null },
  "./ui": {
    Button: ({ children }) => React.createElement("button", null, children),
    ButtonLink: ({ children, href }) => React.createElement("a", { href }, children),
  },
  "./experimentLineageModel": { historyBackendLabel: () => "local", historyJobId: () => "—" },
  "lucide-react": { ChartLine: () => null, ChevronDown: () => null, CircleStop: () => null, ExternalLink: () => null, ScrollText: () => null },
};
const exports = {};
new Function("require", "exports", compiled)(withWorkspaceMocks((name) => mocks[name] ?? require(name)), exports);
const props = {
  experiment: { id: "experiment-a", projectId: "project-a", slug: "example" },
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

test("the header offers Trackio only once the run has logged there", () => {
  assert.doesNotMatch(render(null), />experiment_overview_visualize</);
  trackioUrl = "http://trackio.example:7860/?project=p&runs=run-new";
  try {
    const html = render(null);
    assert.match(html, />experiment_overview_visualize</);
    assert.match(html, /LOG:run-new/);
  } finally {
    trackioUrl = null;
  }
});

test("the trackio view says so when the run has not logged there", () => {
  const html = renderToStaticMarkup(React.createElement(exports.DetailDrawer, { ...props, view: "trackio", selectedRunId: null }));
  assert.doesNotMatch(html, /LOG:/);
  assert.match(html, /detail_drawer_trackio_no_data/);
});

test("the trackio view embeds the run's charts in place of its logs", () => {
  trackioUrl = "http://127.0.0.1:7860/?project=p&runs=run-new&sidebar=hidden";
  try {
    const html = renderToStaticMarkup(React.createElement(exports.DetailDrawer, { ...props, view: "trackio", selectedRunId: null }));
    assert.match(html, /<iframe[^>]*src="http:\/\/127\.0\.0\.1:7860\/\?project=p&amp;runs=run-new&amp;sidebar=hidden"/);
    assert.match(html, /experiment_overview_logs/);
    assert.doesNotMatch(html, /LOG:/);
  } finally {
    trackioUrl = null;
  }
});


test("the contact notice follows the selected run", () => {
  assert.match(render(null), /NOTICE:run-new/);
  assert.match(render("run-old"), /NOTICE:run-old/);
  assert.doesNotMatch(render("missing-run"), /NOTICE:/);
});

test("stop reads as pending while the host can't take the cancel", () => {
  const live = { id: "run-live", experimentId: "experiment-a", createdAt: 4, status: "running", cancelling: false, contact: null };
  const renderLive = (run) => renderToStaticMarkup(React.createElement(exports.DetailDrawer, { ...props, runs: [run], selectedRunId: null }));

  assert.match(renderLive(live), />common_stop</);
  assert.match(renderLive({ ...live, cancelling: true }), />status_cancelling</);
  const unreachable = { type: "unreachable", since: 1, cause: "timed out" };
  assert.match(renderLive({ ...live, cancelling: true, contact: unreachable }), />status_cancelling</);
  assert.match(renderLive({ ...live, contact: unreachable }), />common_stop</);
});
