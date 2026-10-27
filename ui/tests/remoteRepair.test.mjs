import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import ts from "typescript";

const require = createRequire(import.meta.url);
const source = ts.createSourceFile("RemoteRuntime.tsx", readFileSync(new URL("../src/RemoteRuntime.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
const component = source.statements.find((node) => ts.isFunctionDeclaration(node) && node.name?.text === "RemoteSetup");
const compiled = ts.transpileModule(`export ${component.getText(source)}`, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
}).outputText;

function mount(writeText) {
  const state = [];
  const alerts = [];
  const timers = [];
  let cursor = 0;
  const bindings = {
    useState(initial) {
      const index = cursor++;
      if (!(index in state)) state[index] = initial;
      return [state[index], (value) => { state[index] = value; }];
    },
    useEffect() {},
    m: new Proxy({}, { get: (_, name) => () => String(name) }),
    ltr: (value) => value,
    needsInteractiveSsh: () => false,
    Button: "Button",
    Input: "Input",
    Spinner: "Spinner",
    CommandTerminal: "CommandTerminal",
    SshConnectTerminal: "SshConnectTerminal",
    RemoteStopDialog: "RemoteStopDialog",
    showAlert: (...args) => alerts.push(args),
    window: { setTimeout: (callback) => timers.push(callback) },
    navigator: { clipboard: { writeText } },
  };
  const exports = {};
  new Function("require", "exports", ...Object.keys(bindings), compiled)(require, exports, ...Object.values(bindings));
  const props = {
    runtime: { session: { status: "disconnected", error: "Permissions refused", host: "fixture", installPaths: null, repairCommand: "chmod go-w '/fixture/bin'" } },
    retriedInteractiveError: null,
    setRetriedInteractiveError() {},
  };
  return { render() { cursor = 0; return exports.RemoteSetup(props); }, alerts, timers };
}

function find(node, predicate) {
  if (!node || typeof node !== "object") return null;
  if (predicate(node)) return node;
  for (const child of [node.props?.children].flat(Infinity)) {
    const found = find(child, predicate);
    if (found) return found;
  }
  return null;
}

const button = (view, label) => find(view.render(), (node) => node.type === "Button" && node.props.children === label);

test("copy reports success only after the clipboard write resolves", async () => {
  let resolve;
  let command;
  const view = mount((value) => {
    command = value;
    return new Promise((done) => { resolve = done; });
  });
  const copying = button(view, "remote_repair_copy").props.onClick();
  assert.equal(command, "chmod go-w '/fixture/bin'");
  assert.ok(button(view, "remote_repair_copy"));
  resolve();
  await copying;
  assert.ok(button(view, "remote_repair_copied"));
  assert.equal(view.alerts.length, 0);
  view.timers[0]();
  assert.ok(button(view, "remote_repair_copy"));
});

test("clipboard failure reports an error without claiming copied", async () => {
  const view = mount(async () => { throw new Error("Clipboard denied"); });
  await button(view, "remote_repair_copy").props.onClick();
  assert.ok(button(view, "remote_repair_copy"));
  assert.equal(button(view, "remote_repair_copied"), null);
  assert.deepEqual(view.alerts, [["file_tree_clipboard_unavailable", "error"]]);
  assert.equal(view.timers.length, 0);
});

test("a failed terminal can be opened again without reconnecting", () => {
  const view = mount(async () => {});
  button(view, "remote_repair_run").props.onClick();
  const terminal = find(view.render(), (node) => node.type === "CommandTerminal");
  terminal.props.onError("Permission denied", true);
  assert.equal(find(view.render(), (node) => node.type === "CommandTerminal"), null);
  assert.deepEqual(view.alerts, [["Permission denied", "error"]]);
  button(view, "remote_repair_run").props.onClick();
  assert.ok(find(view.render(), (node) => node.type === "CommandTerminal"));
});
