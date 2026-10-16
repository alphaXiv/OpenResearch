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
const stub = new Proxy({}, { get: (_, name) => (name === "__esModule" ? false : () => null) });
const mocks = {
  "../paraglide/messages.js": { m: new Proxy({}, { get: (_, name) => () => String(name) }) },
  "../paraglide/runtime.js": { getLocale: () => "en" },
  "../i18n": { ltr: String, autoDir: String },
  "../bashCommand": bashCommand,
  "../chatRecovery": chatRecovery,
  "../chatRendering": chatRendering,
  "../orxCommand": orxCommand,
  "@tanstack/react-router": { useNavigate: () => () => {} },
  "./ui": new Proxy({}, {
    get: (_, name) => (name === "Button" ? ({ children }) => React.createElement("button", null, children) : stub[name]),
  }),
};
const exports = {};
new Function("require", "exports", compiled)(
  (name) => mocks[name] ?? (name.startsWith(".") ? stub : require(name)),
  exports,
);
const renderMessage = (parts) => renderToStaticMarkup(React.createElement(exports.Message, {
  message: { id: "msg-1", role: "assistant", parts, createdAt: 0, completedAt: 1000 },
  activePermissionId: null,
  forkDisabled: false,
  branchDisabled: false,
  onFork: () => {},
  onSelectFork: () => {},
  onRecover: () => {},
}));

const detail = "API Error: Connection dropped (ECONNRESET)";
const bash = { id: "tool-1", type: "tool", tool: "Bash", state: { status: "completed", input: { command: "echo ready" } } };
const apiError = { id: "api-error-1", type: "tool", tool: "api_error", state: { status: "error", error: detail } };
const recovery = {
  id: "turn-recovery", type: "tool", tool: "error",
  state: { status: "error", error: detail, input: { turnId: "turn-1", errorKind: "claude_terminal", recoveryAction: "continue" } },
};

test("a full message presents one nonfatal transport status with details and recovery", () => {
  const html = renderMessage([bash, apiError, recovery]);
  assert.equal(html.match(/chat_panel_temporary_api_error/g)?.length, 1);
  assert.equal(html.match(/Connection dropped/g)?.length, 1);
  assert.match(html, /chat_continue/);
  assert.doesNotMatch(html, /chat_turn_incomplete|text-accent-red/);
});

test("a transport status stays visible outside a preceding collapsed tool group", () => {
  const html = renderMessage([bash, bash, apiError]);
  assert.equal(html.match(/chat_panel_temporary_api_error/g)?.length, 1);
  assert.match(html, /Connection dropped/);
  assert.doesNotMatch(html, /text-accent-red/);
});

test("nontransport failures retain the generic recovery label", () => {
  const generic = { ...apiError, tool: "error" };
  const html = renderMessage([bash, generic, recovery]);
  assert.match(html, /chat_turn_incomplete/);
  assert.match(html, /text-accent-red/);
  assert.doesNotMatch(html, /chat_panel_temporary_api_error/);
});

test("unrelated errors are not absorbed into recovery", () => {
  const html = renderMessage([apiError, { ...recovery, state: { ...recovery.state, error: "Other failure" } }]);
  assert.match(html, /chat_panel_temporary_api_error/);
  assert.match(html, /chat_turn_incomplete/);
  assert.match(html, /Other failure/);
});
