import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";
import { sshObservation } from "../src/sshMonitoring.ts";

const require = createRequire(import.meta.url);
const source = readFileSync(new URL("../src/components/RunMonitoring.tsx", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX },
}).outputText;
const mocks = {
  "../api": { refreshRun: () => assert.fail("rendering cached data must not access the remote host") },
  "../sshMonitoring": { sshObservation },
  "../paraglide/messages.js": { m: new Proxy({}, { get: (_, key) => () => String(key) }) },
  "./ui": { Button: ({ children, disabled }) => React.createElement("button", { disabled }, children) },
};
const exports = {};
new Function("require", "exports", compiled)((name) => mocks[name] ?? require(name), exports);
const render = (run) => renderToStaticMarkup(React.createElement(exports.RunMonitoring, { run }));

test("on-demand UI labels cached observations and pending cancellation without refreshing on mount", () => {
  const html = render({ id: "r", updatedAt: 1, status: "running", cancelRequested: true,
    backend: { kind: "ssh_job", monitoring: { mode: "on-demand", lastObservedAt: 123 } } });
  assert.match(html, /ssh_monitor_on_demand/);
  assert.match(html, /ssh_monitor_observed/);
  assert.match(html, /ssh_monitor_cached_hint/);
  assert.match(html, /ssh_monitor_cancel_pending/);
  assert.match(html, /ssh_monitor_refresh/);
  assert.match(html, /ssh_monitor_sync_logs/);
});

test("terminal status leaves incomplete logs available for explicit collection", () => {
  const html = render({ id: "r", updatedAt: 1, status: "done", cancelRequested: false,
    backend: { kind: "ssh_job", monitoring: { mode: "on-demand", logsComplete: false } } });
  assert.match(html, /ssh_monitor_logs_pending/);
  assert.match(html, /<button disabled="">ssh_monitor_refresh/);
  assert.match(html, /<button>ssh_monitor_sync_logs/);
});

test("other backends and containers keep their existing UI", () => {
  for (const backend of [{ kind: "slurm_job" }, { kind: "ssh_job", sshContainer: { id: "c" } }]) {
    assert.equal(render({ id: "r", updatedAt: 1, status: "running", backend }), "");
  }
});
