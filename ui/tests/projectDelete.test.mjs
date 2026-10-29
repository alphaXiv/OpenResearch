import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import ts from "typescript";
import { dialogHarness } from "./dialogHarness.mjs";

const require = createRequire(import.meta.url);
const source = readFileSync(new URL("../src/components/ProjectInfoCard.tsx", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
}).outputText;
let state, index, calls, removed, failure, artifactsError, alerts, dialogs, options;
const exports = {};
new Function("require", "exports", compiled)((name) => {
  if (name === "react") return { ...React, useState: (initial) => {
    const key = index++;
    if (!(key in state)) state[key] = initial;
    return [state[key], (value) => { state[key] = value; }];
  }, useRef: () => ({ current: null }), useId: () => "delete-project" };
  if (name === "../api") return { deleteProject: async (...args) => {
    calls.push(args);
    if (failure) throw new Error(failure);
    return artifactsError;
  } };
  if (name === "../paraglide/messages.js") return { m: new Proxy({}, { get: (_, key) => () => key }) };
  if (name === "./useFileManagerLabel") return { useFileManagerLabel: () => "Reveal" };
  if (name === "../dialog") return { showCustomDialog: (args) => { options = args; dialogs.showCustomDialog(args); } };
  if (name === "./ui") return { Button: "button", DialogContent: "content", DialogTitle: "title", showAlert: (...args) => { alerts.push(args); } };
  return require(name);
}, exports);

function render() {
  index = 0;
  const root = dialogs.render().at(-1);
  const content = root.props.children;
  return content.type(content.props);
}
function open() {
  index = 0;
  const tree = exports.ProjectInfoCard({ project: { id: "p1", name: "Project", artifactsDir: "/files/project" }, onRemoved: () => { removed++; } });
  elements(tree).find((node) => node.props.onClick && elements(node).some((child) => React.Children.toArray(child.props.children).includes("projects_home_delete_project_action"))).props.onClick();
  state = [];
}
function elements(node) {
  if (!React.isValidElement(node)) return [];
  return [node, ...React.Children.toArray(node.props.children).flatMap(elements)];
}
function reset() {
  state = [];
  calls = [];
  removed = 0;
  failure = artifactsError = null;
  alerts = [];
  dialogs = dialogHarness();
  open();
}
async function confirm(tree) {
  elements(tree).find((node) => elements(node).some((child) => React.Children.toArray(child.props.children).includes("projects_home_delete_project_action")) && node.type === "button").props.onClick();
  await new Promise(setImmediate);
}

test("project deletion keeps artifacts by default and cancellation deletes nothing", async () => {
  reset();
  const tree = render();
  assert.equal(elements(tree).find((node) => node.type === "input").props.checked, false);
  assert.equal(options.onCancel, "close");
  elements(tree).find((node) => node.props.children === "projects_home_cancel").props.onClick();
  await new Promise(setImmediate);
  assert.equal(dialogs.render()[0].props.open, false);
  assert.deepEqual(calls, []);
  open();
  await confirm(render());
  assert.deepEqual(calls, [["p1", false]]);
  assert.equal(removed, 1);
});

test("artifact deletion requires checkbox opt-in and errors leave the dialog open", async () => {
  reset();
  elements(render()).find((node) => node.type === "input").props.onChange({ target: { checked: true } });
  failure = "Keep artifacts to preserve the source folder.";
  await confirm(render());
  assert.deepEqual(calls, [["p1", true]]);
  assert.equal(dialogs.render()[0].props.open, true);
  assert.equal(removed, 0);
  assert.equal(elements(render()).find((node) => node.props.role === "alert").props.children, failure);
});

test("a failed artifact removal after deletion still closes the dialog and reports it", async () => {
  reset();
  artifactsError = "The project was deleted. Could not delete artifacts at /files/project: denied";
  await confirm(render());
  assert.equal(dialogs.render()[0].props.open, false);
  assert.equal(removed, 1);
  assert.deepEqual(alerts, [[artifactsError, "error"]]);
});
