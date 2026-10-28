import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import ts from "typescript";

const require = createRequire(import.meta.url);
const source = readFileSync(new URL("../src/components/SettingsPage.tsx", import.meta.url), "utf8");
const file = ts.createSourceFile("SettingsPage.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
const component = file.statements.find((node) => ts.isFunctionDeclaration(node) && node.name?.text === "UpdatesTab");
const compiled = ts.transpileModule(`export ${component.getText(file)}`, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
}).outputText;

function elements(node) {
  if (!React.isValidElement(node)) return [];
  return [node, ...React.Children.toArray(node.props.children).flatMap(elements)];
}

function updates(initialChannel = "stable", fail = false) {
  let status = { current: "0.2.21", channel: "app-bundle", releaseChannel: initialChannel, releaseChannelsAvailable: true, selfUpdates: true, autoUpdate: true };
  const state = [];
  const requests = [];
  let cursor = 0;
  const exports = {};
  const bindings = {
    useMutation: () => ({}), setAutoUpdateApi() {}, useRestartApp: () => ({ restarting: false }),
    useUpdateStatus: () => ({ status, error: null, apply: (next) => { status = next; } }),
    useState(initial) { const index = cursor++; if (!(index in state)) state[index] = initial; return [state[index], (next) => { state[index] = next; }]; },
    useRef(initial) { const index = cursor++; state[index] ??= { current: initial }; return state[index]; },
    setReleaseChannel: async (channel) => {
      requests.push(channel);
      if (fail) throw new Error("Channel change failed");
      return { ...status, releaseChannel: channel };
    },
    requestAnimationFrame() {}, document: {},
    m: new Proxy({}, { get: (_, key) => () => key }), ltr: String,
    CHANNEL_LABELS: { "app-bundle": () => "macOS app" }, MANUAL_UPDATE_HINT: {},
    SETTINGS_CARD_CLASS_NAME: "card", KV_CLASS_NAME: "kv", PROJECT_DEFAULT_ROW_CLASS_NAME: "row", SEGMENTED_CLASS_NAME: "segmented",
    EscapeScope: "escape-scope",
    Button: "button", Switch: "switch", InstallCliRow: "install", LoadingRow: "loading", Spinner: "spinner",
    Dialog: { Root: "dialog-root", Portal: "dialog-portal", Backdrop: "dialog-backdrop", Viewport: "dialog-viewport", Popup: "dialog-popup" },
  };
  new Function("require", "exports", ...Object.keys(bindings), compiled)(require, exports, ...Object.values(bindings));
  return {
    render() { cursor = 0; return elements(exports.UpdatesTab()); },
    requests,
    get channel() { return status.releaseChannel; },
  };
}

const radio = (tree, channel) => tree.find((element) => element.props["data-channel"] === channel);
const dialog = (tree) => tree.find((element) => element.type === "dialog-root");
const action = (tree, label) => tree.find((element) => element.type === "button" && element.props.children === label);

test("switching to canary requires confirmation for pointer and keyboard", async () => {
  const view = updates();
  radio(view.render(), "canary").props.onClick();
  assert.equal(dialog(view.render()).props.open, true);
  assert.deepEqual(view.requests, []);
  action(view.render(), "settings_page_cancel").props.onClick();
  assert.equal(dialog(view.render()).props.open, false);
  assert.equal(view.channel, "stable");

  let prevented = false;
  view.render().find((element) => element.props.role === "radiogroup").props.onKeyDown({ key: "ArrowRight", preventDefault() { prevented = true; } });
  assert.equal(prevented, true);
  assert.equal(dialog(view.render()).props.open, true);
  assert.deepEqual(view.requests, []);
  action(view.render(), "settings_canary_confirm_action").props.onClick();
  await new Promise(setImmediate);
  assert.deepEqual(view.requests, ["canary"]);
  assert.equal(view.channel, "canary");
  assert.equal(dialog(view.render()).props.open, false);
});

test("switching back to stable does not require the canary confirmation", async () => {
  const view = updates("canary");
  radio(view.render(), "stable").props.onClick();
  await new Promise(setImmediate);
  assert.deepEqual(view.requests, ["stable"]);
  assert.equal(view.channel, "stable");
  assert.equal(dialog(view.render()).props.open, false);
});

test("a failed switch keeps the selected channel and reports the error", async () => {
  const view = updates("stable", true);
  radio(view.render(), "canary").props.onClick();
  action(view.render(), "settings_canary_confirm_action").props.onClick();
  await new Promise(setImmediate);
  assert.deepEqual(view.requests, ["canary"]);
  assert.equal(view.channel, "stable");
  assert.ok(view.render().some((element) => element.props.className === "error" && element.props.children === "Channel change failed"));
});
