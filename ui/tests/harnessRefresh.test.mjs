import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";

const require = createRequire(import.meta.url);
const source = readFileSync(new URL("../src/components/SettingsPage.tsx", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
}).outputText;
const messages = JSON.parse(readFileSync(new URL("../messages/en.json", import.meta.url), "utf8"));
const stub = new Proxy({}, { get: (_, name) => name === "__esModule" ? false : () => null });
let states, cursor, refresh, currentScope, button;
let harness = { id: "codex", name: "Codex", installed: true, authenticated: true, agentReady: true, version: "0.154.0", models: [{ id: "gpt-6-astra" }] };
const queryFactories = new Proxy({}, { get: (_, name) => () => ({ queryKey: [name] }) });
const mocks = {
  react: { ...React, useState: (initial) => {
    const index = cursor++;
    if (!(index in states)) states[index] = initial;
    return [states[index], (value) => { states[index] = typeof value === "function" ? value(states[index]) : value; }];
  } },
  "@tanstack/react-query": { useQuery: (options) => ({ data: options.queryKey[0] === "getHarnessesQuery" ? [harness] : undefined }) },
  "../queries/settings": { getHarnessesQuery: queryFactories.getHarnessesQuery, getHarnessSetupCommandsQuery: queryFactories.getHarnessSetupCommandsQuery, refreshHarnesses: (...args) => refresh(...args) },
  "../queries/client": { workspaceScope: () => [], isCurrentScope: () => currentScope },
  "../paraglide/messages.js": { m: new Proxy({}, { get: (_, name) => (params = {}) => (messages[name] ?? name).replace(/\{(\w+)\}/g, (_, key) => params[key]) }) },
  "../api": { fmtNumber: String },
  "./ui/cn": { cn: (...names) => names.filter(Boolean).join(" ") },
  "./ui": new Proxy({}, { get: (_, name) => {
    if (name === "Button") return ({ children, onClick, disabled }) => {
      button = { onClick, disabled };
      return React.createElement("button", { disabled }, children);
    };
    if (name === "Badge") return ({ children }) => React.createElement("span", null, children);
    return stub[name];
  } }),
};
const exports = {};
new Function("require", "exports", `${compiled}\nexports.HarnessesTab = HarnessesTab;`)(
  (name) => mocks[name] ?? (name.startsWith(".") ? stub : require(name)), exports,
);
function render() {
  cursor = 0;
  return renderToStaticMarkup(React.createElement(exports.HarnessesTab, { remote: false }));
}
const settle = () => new Promise((resolve) => setImmediate(resolve));
test.beforeEach(() => {
  states = [];
  currentScope = true;
  harness = { ...harness, id: "codex" };
  refresh = async (...args) => { assert.deepEqual(args, [true, true]); return [harness]; };
});

test("an unchanged catalog still announces refresh completion and explains the CLI limit", async () => {
  const initial = render();
  assert.ok(initial.includes(messages.settings_codex_catalog_source));
  assert.ok(!initial.includes(messages.settings_harness_refresh_complete));
  button.onClick();
  assert.ok(render().includes(messages.common_checking));
  assert.equal(button.disabled, true);
  await settle();
  const html = render();
  assert.ok(html.includes('role="status"'));
  assert.ok(html.includes(messages.settings_harness_refresh_complete));
  assert.equal(button.disabled, false);
});

test("refresh failures are visible and retry clears the error before succeeding", async () => {
  refresh = async () => { throw new Error("catalog unavailable"); };
  render();
  button.onClick();
  await settle();
  assert.match(render(), /role="alert"[^>]*>Refresh failed: catalog unavailable/);
  refresh = async () => [harness];
  button.onClick();
  assert.ok(!render().includes('role="alert"'));
  await settle();
  assert.ok(render().includes(messages.settings_harness_refresh_complete));
});

test("non-Error rejections are reported instead of swallowed", async () => {
  refresh = async () => { throw "connection lost"; };
  render();
  button.onClick();
  await settle();
  assert.match(render(), /Refresh failed: connection lost/);
});

test("refresh results from a previous workspace are not shown", async () => {
  render();
  button.onClick();
  currentScope = false;
  await settle();
  assert.ok(!render().includes(messages.settings_harness_refresh_complete));
  refresh = async () => { throw new Error("old workspace failure"); };
  button.onClick();
  await settle();
  assert.ok(!render().includes("old workspace failure"));
});

test("other harnesses do not get Codex-specific catalog advice", () => {
  harness = { ...harness, id: "claude-code" };
  assert.ok(!render().includes(messages.settings_codex_catalog_source));
});
