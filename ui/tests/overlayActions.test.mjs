import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import ts from "typescript";
import { dialogHarness } from "./dialogHarness.mjs";

const require = createRequire(import.meta.url);

function load(name, mocks = {}, exportName = name) {
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
    if (id === "./ui") return { Button: "button", MenuItem: "button", IconButton: "button", Dialog: "dialog", DialogContent: "content", DialogTitle: "title", DialogFooter: "footer" };
    return require(id);
  }, exports);
  return {
    ...exports,
    render(props) {
      cursor = 0;
      const tree = exports[exportName](props);
      return tree.type === "escape-scope" ? tree.props.children : tree;
    },
  };
}

function elements(node) {
  if (!React.isValidElement(node)) return [];
  return [node, ...React.Children.toArray(node.props.children).flatMap(elements)];
}

function action(tree, label) {
  return elements(tree).find((element) => React.Children.toArray(element.props.children).includes(label));
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

test("remote stop cancellation is blocked while stopping, including Escape", async () => {
  let closed = 0;
  let finishStop;
  const dialogs = dialogHarness();
  const dialog = load("RemoteStopDialog");
  const props = { host: "research", preview: { attachmentCount: 1 }, currentClientAttached: true };
  const options = {
    component: "remote-stop",
    props,
    onCancel: () => closed++,
    onConfirm: () => new Promise((resolve) => { finishStop = resolve; }),
  };
  dialogs.showCustomDialog(options);
  let root = dialogs.render()[0];
  const idle = dialog.render({ ...props, ...root.props.children.props });
  action(idle, "remote_stop_confirm_action").props.onClick();
  root = dialogs.render()[0];
  assert.equal(root.props.onOpenChange, undefined);
  const pending = dialog.render({ ...props, ...root.props.children.props });
  assert.equal(action(pending, "settings_page_cancel").props.disabled, true);
  root.props.children.props.cancel();
  assert.equal(closed, 0);
  assert.equal(dialogs.render()[0].props.open, true);
  finishStop();
  await new Promise(setImmediate);
  assert.equal(dialogs.render()[0].props.open, false);
  dialogs.showCustomDialog(options);
  dialogs.render()[1].props.onOpenChange(false, { reason: "escape-key" });
  assert.equal(closed, 1);
});

test("SSH saves retain optimistic concurrency and block Escape while saving", async () => {
  const dialogs = dialogHarness();
  const saves = [];
  const cached = [];
  let closed = 0;
  let finishSave;
  const dialog = load("SshConfigDialog", {
    "@tanstack/react-query": {
      useQuery: () => ({ data: { path: "/tmp/config", content: "Host original" } }),
      useMutation: ({ mutationFn }) => ({ mutateAsync: mutationFn }),
    },
    "../queries/client": { isCurrentScope: () => true, setScopedQueryData: (...args) => cached.push(args) },
    "../queries/settings": { getSshConfigQuery: () => ({ queryKey: ["ssh"] }) },
    "../api": { saveSshConfig: (...args) => { saves.push(args); return new Promise((resolve) => { finishSave = resolve; }); } },
    "../dialog": dialogs,
    "./CodeEditor": { CodeEditor: "editor" },
    "./ui": { Button: "button", IconButton: "button", DialogContent: "content", DialogTitle: "title", showAlert() {} },
  });
  dialog.showSshConfigDialog(() => closed++);
  const render = () => dialog.render(dialogs.render()[0].props.children.props);
  assert.equal(action(render(), "common_save").props.disabled, true);
  elements(render()).find((element) => element.type === "editor").props.onChange("Host edited");
  action(render(), "common_save").props.onClick();
  assert.deepEqual(saves, [["Host edited", "Host original"]]);
  assert.equal(action(render(), "settings_page_cancel").props.disabled, true);
  dialogs.render()[0].props.onOpenChange(false);
  await new Promise(setImmediate);
  assert.equal(dialogs.render().length, 1);
  assert.equal(dialogs.render()[0].props.open, true);
  assert.equal(closed, 0);
  finishSave();
  await new Promise(setImmediate);
  assert.deepEqual(cached, [[["ssh"], { path: "/tmp/config", content: "Host edited" }]]);
  render();
  dialogs.render()[0].props.onOpenChange(false);
  await new Promise(setImmediate);
  assert.equal(closed, 1);
  assert.equal(dialogs.render()[0].props.open, false);
});

for (const trigger of ["Escape", "outside click", "cancel button"]) {
  test(`SSH ${trigger} waits for discard consent and retains the draft when declined`, async () => {
    const dialogs = dialogHarness();
    let closed = 0;
    const dialog = load("SshConfigDialog", {
      "@tanstack/react-query": { useQuery: () => ({ data: { path: "/tmp/config", content: "original" } }), useMutation: () => ({}) },
      "../queries/client": {},
      "../queries/settings": { getSshConfigQuery: () => ({ queryKey: ["ssh"] }) },
      "../api": {},
      "../dialog": dialogs,
      "./CodeEditor": { CodeEditor: "editor" },
    });
    dialog.showSshConfigDialog(() => closed++);
    const render = () => dialog.render(dialogs.render()[0].props.children.props);
    elements(render()).find((element) => element.type === "editor").props.onChange("edited");
    const dismiss = () => {
      const tree = render();
      if (trigger === "cancel button") action(tree, "settings_page_cancel").props.onClick();
      else dialogs.render()[0].props.onOpenChange(false, { reason: trigger === "Escape" ? "escape-key" : "outside-press" });
    };
    dismiss();
    assert.equal(dialogs.render()[0].props.open, true);
    assert.equal(dialogs.render()[0].props.children.props.isDisabled, true);
    assert.equal(dialogs.render()[1].props.children.props.description, "ssh_config_discard_changes");
    assert.equal(dialogs.render()[1].props.children.props.destructive, true);
    dialogs.render()[1].props.children.props.cancel();
    await new Promise(setImmediate);
    assert.equal(dialogs.render()[0].props.open, true);
    assert.equal(dialogs.render()[0].props.children.props.isDisabled, false);
    assert.equal(elements(render()).find((element) => element.type === "editor").props.value, "edited");
    assert.equal(closed, 0);
    dismiss();
    dialogs.render()[2].props.children.props.confirm();
    await new Promise(setImmediate);
    assert.equal(dialogs.render()[0].props.open, false);
    assert.equal(closed, 1);
  });
}

test("local model setup reads current installation state and blocks dismissal while saving", async () => {
  const dialogs = dialogHarness();
  const connected = [];
  let installed = false;
  let finishSave;
  const dialog = load("LocalModelSetup", {
    "@tanstack/react-query": {
      useQuery: () => ({ data: [{ id: "opencode", installed }] }),
      useMutation: ({ mutationFn }) => ({ mutateAsync: mutationFn }),
    },
    "../dialog": dialogs,
    "../api": {
      discoverLocalModels: async () => ({ models: ["local-test"] }),
      connectLocalModel: () => new Promise((resolve) => { finishSave = resolve; }),
    },
    "../queries/settings": {
      getHarnessesQuery: () => ({}),
      refreshHarnesses: async () => [{ id: "opencode", models: [{ id: "connected-model" }] }],
    },
    "./ModelPicker": { OptionPicker: "picker" },
    "../assets/lm-studio-logo.svg": "lm-studio.svg",
    "../assets/ollama-logo.png": "ollama.png",
    "../assets/omlx-logo.svg": "omlx.svg",
    "lucide-react": { Link2: "icon", Plus: "icon", RefreshCw: "icon", Trash2: "icon", X: "icon" },
    "./ui": { DialogContent: "content", DialogTitle: "title", Button: "button", IconButton: "button", Input: "input", Spinner: "spinner" },
  }, "LocalModelDialog");
  dialogs.showCustomDialog({
    component: "local-model",
    props: { onConnected: (model) => connected.push(model) },
    onConfirm: (connect) => connect(),
    onCancel: "close",
  });
  const render = () => dialog.render(dialogs.render()[0].props.children.props);
  assert.equal(action(render(), "local_models_check").props.disabled, true);
  installed = true;
  let tree = render();
  assert.equal(action(tree, "local_models_check").props.disabled, false);
  tree.props.render.props.onSubmit({ preventDefault() {} });
  await new Promise(setImmediate);
  tree = render();
  tree.props.render.props.onSubmit({ preventDefault() {} });
  assert.equal(dialogs.render()[0].props.onOpenChange, undefined);
  assert.equal(action(render(), "settings_page_cancel").props.disabled, true);
  dialogs.render()[0].props.children.props.cancel();
  assert.equal(dialogs.render()[0].props.open, true);
  finishSave({ model: "connected-model" });
  await new Promise(setImmediate);
  assert.deepEqual(connected, ["connected-model"]);
  assert.equal(dialogs.render()[0].props.open, false);
});
