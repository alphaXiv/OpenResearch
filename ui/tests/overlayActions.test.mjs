import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import ts from "typescript";

const require = createRequire(import.meta.url);

function load(name, mocks = {}) {
  const source = readFileSync(new URL(`../src/components/${name}.tsx`, import.meta.url), "utf8");
  const compiled = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
  }).outputText;
  const state = [];
  let cursor = 0;
  const exports = {};
  const react = {
    ...React,
    useState(initial) {
      const index = cursor++;
      if (!(index in state)) state[index] = typeof initial === "function" ? initial() : initial;
      return [state[index], (value) => { state[index] = typeof value === "function" ? value(state[index]) : value; }];
    },
    useRef(initial) {
      const index = cursor++;
      state[index] ??= { current: initial };
      return state[index];
    },
    useEffect() {},
  };
  new Function("require", "exports", compiled)((id) => {
    if (id === "./EscapeScope") return { EscapeScope: "escape-scope" };
    if (id in mocks) return mocks[id];
    if (id === "react") return react;
    if (id === "../paraglide/messages.js") return { m: new Proxy({}, { get: (_, key) => () => key }) };
    if (id === "../i18n") return { ltr: String, fmtNumber: String };
    if (id === "./ui") return { Button: "button", MenuItem: "button", IconButton: "button" };
    return require(id);
  }, exports);
  return {
    render(props) {
      cursor = 0;
      const tree = exports[name](props);
      return tree.type === "escape-scope" ? tree.props.children : tree;
    },
  };
}

function elements(node) {
  if (!React.isValidElement(node)) return [];
  return [node, ...React.Children.toArray(node.props.children).flatMap(elements)];
}

function action(tree, label) {
  return elements(tree).find((element) => element.props.children === label);
}

test("the project picker prioritizes the current project, then recent activity, and selects the requested project", () => {
  const selections = [];
  const picker = load("ComposerProjectPicker");
  const tree = picker.render({
    projects: [{ id: "older", name: "Older", updatedAt: 1 }, { id: "current", name: "Current", updatedAt: 2 }, { id: "newer", name: "Newer", updatedAt: 20 }],
    activity: [{ projectId: "older", lastActivityAt: 30 }],
    projectId: "current",
    projectName: "Current",
    onSelect: (id) => selections.push(id),
    onNewProject() {},
  });
  const choices = elements(tree).filter((element) => "value" in element.props && element.props.onClick);
  assert.deepEqual(choices.map((choice) => choice.props.value), ["current", "older", "newer"]);
  choices[1].props.onClick();
  assert.deepEqual(selections, ["older"]);
});

test("remote stop cancellation is blocked while stopping, including Escape", () => {
  let closed = 0;
  const dialog = load("RemoteStopDialog");
  const props = { host: "research", preview: { attachmentCount: 1 }, currentClientAttached: true, stopping: true, onClose: () => closed++, onConfirm() {} };
  let canceled = 0;
  const pending = dialog.render(props);
  const event = new Event("keydown", { cancelable: true });
  pending.props.onOpenChange(false, { event, reason: "escape-key", cancel: () => canceled++ });
  assert.equal(event.defaultPrevented, true);
  assert.equal(canceled, 1);
  assert.equal(closed, 0);
  assert.equal(action(pending, "settings_page_cancel").props.disabled, true);
  dialog.render({ ...props, stopping: false }).props.onOpenChange(false, { reason: "escape-key", cancel() { assert.fail("idle close was canceled"); } });
  assert.equal(closed, 1);
});

test("SSH edits require discard consent and preserve the optimistic-concurrency value when saved", async () => {
  const saves = [];
  const cached = [];
  let closed = 0;
  let saved = 0;
  let finishSave;
  const previousWindow = globalThis.window;
  globalThis.window = { confirm: () => false };
  try {
    const dialog = load("SshConfigDialog", {
      "@tanstack/react-query": {
        useQuery: () => ({ data: { path: "/tmp/config", content: "Host original" } }),
        useMutation: ({ mutationFn }) => ({ mutateAsync: mutationFn }),
      },
      "../queries/client": { isCurrentScope: () => true, setScopedQueryData: (...args) => cached.push(args) },
      "../queries/settings": { getSshConfigQuery: () => ({ queryKey: ["ssh"] }) },
      "../api": { saveSshConfig: (...args) => { saves.push(args); return new Promise((resolve) => { finishSave = resolve; }); } },
      "./CodeEditor": { CodeEditor: "editor" },
      "./ui": { Button: "button", IconButton: "button", showAlert() {} },
    });
    const props = { onClose: () => closed++, onSaved: () => saved++ };
    const initial = dialog.render(props);
    assert.equal(action(initial, "common_save").props.disabled, true);
    elements(initial).find((element) => element.type === "editor").props.onChange("Host edited");
    let canceled = 0;
    const edited = dialog.render(props);
    const event = new Event("keydown", { cancelable: true });
    edited.props.onOpenChange(false, { event, cancel: () => canceled++ });
    assert.equal(event.defaultPrevented, true);
    assert.equal(canceled, 1);
    assert.equal(closed, 0);
    action(edited, "common_save").props.onClick();
    assert.deepEqual(saves, [["Host edited", "Host original"]]);
    const pending = dialog.render(props);
    pending.props.onOpenChange(false, { event, cancel: () => canceled++ });
    assert.equal(canceled, 2);
    assert.equal(action(pending, "settings_page_cancel").props.disabled, true);
    finishSave();
    await new Promise(setImmediate);
    assert.equal(saved, 1);
    assert.equal(closed, 0);
    assert.deepEqual(cached, [[["ssh"], { path: "/tmp/config", content: "Host edited" }]]);
    const clean = dialog.render(props);
    clean.props.onOpenChange(false, { cancel() { assert.fail("clean close was canceled"); } });
    assert.equal(closed, 1);
  } finally {
    globalThis.window = previousWindow;
  }
});

test("SSH discard consent closes without saving", () => {
  const previousWindow = globalThis.window;
  let closed = 0;
  globalThis.window = { confirm: () => true };
  try {
    const dialog = load("SshConfigDialog", {
      "@tanstack/react-query": { useQuery: () => ({ data: { path: "/tmp/config", content: "original" } }), useMutation: () => ({}) },
      "../queries/client": {},
      "../queries/settings": { getSshConfigQuery: () => ({ queryKey: ["ssh"] }) },
      "../api": {},
      "./CodeEditor": { CodeEditor: "editor" },
    });
    const props = { onClose: () => closed++ };
    elements(dialog.render(props)).find((element) => element.type === "editor").props.onChange("edited");
    dialog.render(props).props.onOpenChange(false, { cancel() { assert.fail("consented close was canceled"); } });
    assert.equal(closed, 1);
  } finally {
    globalThis.window = previousWindow;
  }
});
