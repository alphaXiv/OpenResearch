import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import ts from "typescript";

const require = createRequire(import.meta.url);
const source = readFileSync(new URL("../src/components/AgentInstructionsSettings.tsx", import.meta.url), "utf8");
const code = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
}).outputText;
const focusCode = ts.transpileModule(readFileSync(new URL("../src/components/useDialogFocus.ts", import.meta.url), "utf8"), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
}).outputText;
const settle = () => new Promise((resolve) => setImmediate(resolve));
const snapshot = (content, revision = "v1") => ({ path: "/config/openresearch/AGENTS.md", content, revision });
function nodes(tree) {
  return React.isValidElement(tree) ? [tree, ...React.Children.toArray(tree.props.children).flatMap(nodes)] : [];
}

function editor(initial = snapshot("original")) {
  const state = [];
  let cursor, effects, changed;
  const hook = (init) => { const index = cursor++; if (!(index in state)) state[index] = init(); return index; };
  const ctx = {
    data: initial, current: true, confirm: true, confirmations: [], saved: [], cached: [], closed: 0,
    save: async (content) => snapshot(content, "v2"),
  };
  const document = { body: {}, addEventListener() {}, removeEventListener() {} };
  class Element {
    focusCount = 0;
    focus() { document.activeElement = this; this.focusCount++; }
  }
  ctx.opener = new Element();
  ctx.closeButton = new Element();
  ctx.textarea = new Element();
  ctx.reloadButton = new Element();
  document.activeElement = ctx.opener;
  const dialog = {
    querySelector: (selector) => selector === "textarea" && ctx.hasEditor ? ctx.textarea : null,
    querySelectorAll: () => [ctx.closeButton, ...(ctx.hasEditor ? [ctx.textarea] : []), ctx.reloadButton],
  };
  ctx.focused = () => document.activeElement;
  const mocks = {
    react: {
      ...React,
      useState: (initial) => {
        const i = hook(() => typeof initial === "function" ? initial() : initial);
        return [state[i], (next) => { changed ||= !Object.is(state[i], next); state[i] = next; }];
      },
      useRef: (current) => state[hook(() => ({ current }))],
      useEffect: (effect, deps) => {
        const i = hook(() => undefined);
        if (!state[i] || deps.some((value, index) => !Object.is(value, state[i][index]))) {
          state[i] = deps;
          effects.push(effect);
        }
      },
    },
    "react-dom": { createPortal: (tree) => tree },
    "@tanstack/react-query": { useQuery: () => ({ data: ctx.data, isPending: !ctx.data, refetch: async () => ({ data: ctx.data }) }) },
    "@tanstack/react-router": { useBlocker: (options) => { ctx.blocker = options; } },
    "lucide-react": { X: "Icon" },
    "../api": { saveAgentInstructions: async (...args) => { ctx.saved.push(args); return ctx.save(...args); } },
    "../paraglide/messages.js": { m: new Proxy({}, { get: (_, key) => () => String(key) }) },
    "../queries/client": {
      isCurrentScope: () => ctx.current,
      setScopedQueryData: (_key, saved) => { ctx.cached.push(saved); ctx.data = saved; },
    },
    "../queries/settings": { getAgentInstructionsQuery: () => ({ queryKey: ["workspace", 1, "getAgentInstructions"] }) },
    "./CodeEditor": { CodeEditor: "Editor" },
    "./ui": { Button: "button", IconButton: "button", Spinner: "Spinner", showAlert: () => {} },
  };
  const focusExports = {};
  new Function("require", "exports", "document", "HTMLElement", focusCode)(
    (name) => mocks[name] ?? require(name), focusExports, document, Element,
  );
  mocks["./useDialogFocus"] = focusExports;
  const exports = {};
  new Function("require", "exports", "window", "document", code)(
    (name) => mocks[name] ?? require(name), exports,
    { addEventListener() {}, removeEventListener() {}, confirm: (message) => { ctx.confirmations.push(message); return ctx.confirm; } }, document,
  );
  ctx.render = () => {
    for (let attempt = 0; attempt < 10; attempt++) {
      cursor = 0; effects = []; changed = false;
      const tree = exports.AgentInstructionsDialog({ onClose: () => ctx.closed++ });
      const elements = nodes(tree);
      ctx.hasEditor = elements.some((node) => node.type === "Editor");
      elements.find((node) => node.props.role === "dialog").props.ref.current = dialog;
      effects.forEach((effect) => effect());
      if (!changed) return elements;
    }
    assert.fail("editor effects did not settle");
  };
  ctx.input = () => ctx.render().find((node) => node.type === "Editor").props;
  ctx.button = (label) => ctx.render().find((node) => node.type === "button" && node.props.children === label).props;
  return ctx;
}

test("Back and internal navigation ask before discarding a dirty instructions draft", () => {
  for (const action of ["BACK", "PUSH"]) {
    const ctx = editor();
    ctx.render();
    const navigation = {
      action,
      current: { pathname: "/projects/p/settings/settings" },
      next: { pathname: "/projects/p/tasks/chat" },
    };
    assert.equal(ctx.blocker.shouldBlockFn(navigation), false);
    assert.equal(ctx.blocker.enableBeforeUnload(), false);
    assert.deepEqual(ctx.confirmations, []);

    ctx.input().onChange("unsaved instructions");
    ctx.render();
    ctx.confirm = false;
    assert.equal(ctx.blocker.shouldBlockFn(navigation), true);
    assert.equal(ctx.blocker.enableBeforeUnload(), true);
    assert.equal(ctx.input().value, "unsaved instructions");
    assert.deepEqual(ctx.confirmations, ["settings_instructions_discard"]);

    ctx.confirm = true;
    assert.equal(ctx.blocker.shouldBlockFn(navigation), false);
  }
});

test("navigation stays blocked during saving and is released after a successful save", async () => {
  const ctx = editor();
  let finish;
  ctx.save = () => new Promise((resolve) => { finish = resolve; });
  ctx.input().onChange("draft");
  ctx.input().onSave();
  assert.equal(ctx.blocker.shouldBlockFn(), true);
  assert.equal(ctx.blocker.enableBeforeUnload(), true);
  assert.deepEqual(ctx.confirmations, []);

  finish(snapshot("draft", "v2"));
  await settle();
  ctx.render();
  assert.equal(ctx.blocker.shouldBlockFn(), false);
  assert.equal(ctx.blocker.enableBeforeUnload(), false);
});

test("initial asynchronous loading focuses the editor without stealing focus on later refreshes", () => {
  const ctx = editor(null);
  ctx.render();
  assert.equal(ctx.focused(), ctx.closeButton);

  ctx.data = snapshot("loaded instructions");
  assert.equal(ctx.input().value, "loaded instructions");
  assert.equal(ctx.focused(), ctx.textarea);

  ctx.reloadButton.focus();
  ctx.data = snapshot("refreshed instructions", "v2");
  assert.equal(ctx.input().value, "refreshed instructions");
  assert.equal(ctx.focused(), ctx.reloadButton);
});

test("a missing AGENTS.md opens empty and saves with a missing-file revision", async () => {
  const ctx = editor(snapshot("", null));
  assert.equal(ctx.input().value, "");
  assert.equal(ctx.button("common_save").disabled, true);
  ctx.input().onChange("# Rules\nUse free models.\n");
  ctx.input().onSave();
  await settle();
  assert.deepEqual(ctx.saved, [["# Rules\nUse free models.\n", null]]);
  assert.equal(ctx.button("common_save").disabled, true);
});

test("background refresh updates clean content but preserves a dirty draft", () => {
  const ctx = editor();
  ctx.data = snapshot("refreshed", "v2");
  assert.equal(ctx.input().value, "refreshed");
  ctx.input().onChange("my draft");
  ctx.data = snapshot("external edit", "v3");
  assert.equal(ctx.input().value, "my draft");
});

test("a save conflict preserves the draft and reload requires discarding it", async () => {
  const ctx = editor();
  ctx.input().onChange("my draft");
  ctx.save = async () => { throw new Error("AGENTS.md changed on disk"); };
  ctx.input().onSave();
  await settle();
  assert.equal(ctx.input().value, "my draft");
  assert.ok(ctx.render().some((node) => node.props.role === "alert" && node.props.children === "AGENTS.md changed on disk"));
  ctx.data = snapshot("external edit", "v3");
  ctx.confirm = false;
  ctx.button("settings_instructions_reload").onClick();
  await settle();
  assert.equal(ctx.input().value, "my draft");
  ctx.confirm = true;
  ctx.button("settings_instructions_reload").onClick();
  await settle();
  assert.equal(ctx.input().value, "external edit");
  assert.equal(ctx.button("common_save").disabled, true);
});

test("repeated save shortcuts share one write and freeze the editor until it settles", async () => {
  const ctx = editor();
  let finish;
  ctx.save = () => new Promise((resolve) => { finish = resolve; });
  ctx.input().onChange("draft");
  const input = ctx.input();
  input.onSave(); input.onSave();
  assert.equal(ctx.saved.length, 1);
  assert.equal(ctx.input().readOnly, true);
  finish(snapshot("draft", "v2"));
  await settle();
  assert.equal(ctx.input().readOnly, false);
  assert.equal(ctx.button("common_save").disabled, true);
});

test("a response from a retired workspace never updates the new workspace cache", async () => {
  const ctx = editor();
  let finish;
  ctx.save = () => new Promise((resolve) => { finish = resolve; });
  ctx.input().onChange("draft");
  ctx.input().onSave();
  ctx.current = false;
  finish(snapshot("draft", "v2"));
  await settle();
  assert.deepEqual(ctx.cached, []);
  ctx.input().onSave();
  assert.equal(ctx.saved.length, 1);
});
