import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";
import * as bashCommand from "../src/bashCommand.ts";
import * as chatRecovery from "../src/chatRecovery.ts";
import * as chatRendering from "../src/chatRendering.ts";
import * as orxCommand from "../src/orxCommand.ts";

const require = createRequire(import.meta.url);
const source = readFileSync(new URL("../src/components/ChatPanel.tsx", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
}).outputText;
// Every other app module is a stub: its exports render nothing and return nothing.
const stub = new Proxy({}, { get: (_, name) => (name === "__esModule" ? false : () => null) });
const mocks = {
  "../paraglide/messages.js": { m: new Proxy({}, { get: (_, name) => () => String(name) }) },
  "../paraglide/runtime.js": { getLocale: () => "en" },
  "../i18n": { ltr: String, autoDir: String },
  "../bashCommand": bashCommand,
  "../chatRecovery": chatRecovery,
  "../chatRendering": chatRendering,
  "../orxCommand": orxCommand,
  // The real router package warns about a circular require under CommonJS.
  "@tanstack/react-router": { useNavigate: () => () => {} },
  // A plain button, so a recovery action is visible in the markup.
  "./ui": new Proxy({}, {
    get: (_, name) => (name === "Button" ? ({ children }) => React.createElement("button", null, children) : stub[name]),
  }),
};
const exports = {};
new Function("require", "exports", compiled)(
  (name) => mocks[name] ?? (name.startsWith(".") ? stub : require(name)),
  exports,
);
const render = (parts) => renderToStaticMarkup(React.createElement(React.Fragment, null, exports.renderParts(parts, {})));
const renderMessage = (parts) => renderToStaticMarkup(React.createElement(exports.Message, {
  message: { id: "msg-1", role: "assistant", parts, createdAt: 0, completedAt: 1000 },
  activePermissionId: null,
  forkDisabled: false,
  branchDisabled: false,
  onFork: () => {},
  onSelectFork: () => {},
  onRecover: () => {},
}));

const bash = { id: "tool-1", type: "tool", tool: "Bash", state: { status: "completed", input: { command: "echo ready" } } };
const apiError = (state) => ({
  id: "api-error-1",
  type: "tool",
  tool: "api_error",
  state: { status: "error", error: "API Error: Connection to the API was lost (ENOTFOUND).", ...state },
});

const detail = "API Error: Connection to the API was lost (ENOTFOUND).";
const recovery = {
  id: "turn-recovery",
  type: "tool",
  tool: "error",
  state: { status: "error", error: detail, input: { turnId: "turn-1", errorKind: "claude_terminal", recoveryAction: "retry" } },
};

// The visible label of every rendered tool row, in order.
const rowLabels = (html) => [...html.matchAll(/class="tool-line[^"]*">([^<]*)</g)].map((match) => match[1]);
// The label, details and action of every turn status row, in order.
const statusRows = (html) => [...html.matchAll(/<details class="turn-usage-limit[^"]*"><summary[^>]*>.*?<span>([^<]*)<\/span>.*?<\/summary><pre[^>]*>([^<]*)<\/pre>(?:<button>([^<]*)<\/button>)?<\/details>/g)]
  .map(([, label, details, action]) => ({ label, details, action }));

test("a classified API error renders outside the preceding tool group with the localized label", () => {
  const html = render([bash, apiError({ title: "Temporary API error" })]);

  assert.deepEqual(rowLabels(html), ["activity_ran_command", "chat_panel_temporary_api_error"]);
  assert.doesNotMatch(html, /chat_panel_used_tools/);
});

test("a full message reports a transient API failure once, on the recovery row with its retry action", () => {
  const html = renderMessage([bash, apiError({ title: "Temporary API error" }), recovery]);

  assert.deepEqual(rowLabels(html), ["activity_ran_command"]);
  assert.deepEqual(statusRows(html), [{ label: "chat_panel_temporary_api_error", details: detail, action: "app_retry" }]);
});

test("a full message keeps the generic recovery label when its last error is not a transient API failure", () => {
  const html = renderMessage([bash, { ...recovery, id: "err-1", state: { status: "error", error: detail } }, recovery]);

  assert.deepEqual(rowLabels(html), ["activity_ran_command"]);
  assert.deepEqual(statusRows(html), [{ label: "chat_turn_incomplete", details: detail, action: "app_retry" }]);
});
