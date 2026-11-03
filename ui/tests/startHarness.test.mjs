import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
import { resolveHarnessDefaultModel } from "../src/modelSelection.ts";
import { composerStashContent, EMPTY_COMPOSER_STASH } from "../src/composerStash.ts";

const file = ts.createSourceFile("ChatPanel.tsx", readFileSync(new URL("../src/components/ChatPanel.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
let declaration;
function visit(node) {
  if (ts.isFunctionDeclaration(node) && node.name?.text === "startHarness") declaration = node;
  ts.forEachChild(node, visit);
}
visit(file);
assert.ok(declaration);
const js = ts.transpileModule(`${declaration.getText(file)}\nreturn startHarness;`, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;

test("switching harness opens an uncreated draft with the selected harness", () => {
  const calls = [];
  const bindings = {
    resolveHarnessDefaultModel,
    reconcileServiceTier: () => null, reconcileReasoning: () => "medium",
    projectId: "project", onStartHarnessDraft: undefined,
    setSelection: (selection) => calls.push(["selection", selection]),
    onPreferredAgentChange: (selection) => { calls.push(["preference", selection]); return Promise.resolve(); },
    onActiveSessionChangeRef: { current: (id, options) => calls.push(["navigate", id, options]) },
  };
  const start = new Function(...Object.keys(bindings), js)(...Object.values(bindings));
  start({ id: "codex", agentReady: true, models: [{ id: "gpt-6.10-sol" }], options: { defaultPermissionMode: "auto" } });
  const selection = { harness: "codex", model: "gpt-6.10-sol", serviceTier: null, permissionMode: "auto", reasoningLevel: "medium" };
  assert.deepEqual(calls, [["selection", selection], ["preference", selection], ["navigate", null, { projectId: "project" }]]);
});

test("a side composer restores its draft and attachments after unmount", () => {
  let effect;
  function findEffect(node) {
    if (ts.isCallExpression(node) && node.expression.getText(file) === "useLayoutEffect"
      && node.getText(file).includes("composerStashes.get(stashKey)")) effect = node;
    ts.forEachChild(node, findEffect);
  }
  findEffect(file);
  assert.ok(effect);
  const mount = ts.transpileModule(effect.getText(file), { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  const stashes = new Map();
  const live = { current: EMPTY_COMPOSER_STASH };
  let cleanup;
  let restored;
  const bindings = {
    useLayoutEffect: (callback) => { cleanup = callback(); }, composerStashes: stashes,
    stashKey: "side:session", composerPrefill: null, composerLiveRef: live, pendingClientTurn: { current: null }, pendingScope: { current: null },
    EMPTY_COMPOSER_STASH, composerStashContent,
    mountedComposers: new Map(), deletedSessionIds: new Set(), activeId: "session", sideParentSessionId: null,
    isCurrentScope: () => true, sessionsOptions: { queryKey: [] },
    setDraft: (draft) => { restored = { ...restored, draft }; },
    setAttachments: (attachments) => { restored = { ...restored, attachments }; },
    setAnnotations: (annotations) => { restored = { ...restored, annotations }; },
    setUnconfirmedSend: (unconfirmedSend) => { restored = { ...restored, unconfirmedSend }; },
    setImageRefusedFor: () => {},
  };
  const run = () => new Function(...Object.keys(bindings), mount)(...Object.values(bindings));
  run();
  live.current = { draft: "unsent", attachments: [{ dataUrl: "data:,image" }], annotations: [], unconfirmedSend: null };
  cleanup();
  run();
  assert.deepEqual(restored, { draft: "unsent", attachments: [{ dataUrl: "data:,image" }], annotations: [], unconfirmedSend: null });
});

test("composers on machines with matching session IDs retain separate drafts", () => {
  const stashes = new Map();
  let effect;
  function visit(node) {
    if (ts.isCallExpression(node) && node.expression.getText(file) === "useLayoutEffect"
      && node.getText(file).includes("composerStashes.get(stashKey)")) effect = node;
    ts.forEachChild(node, visit);
  }
  visit(file);
  assert.ok(effect);
  const code = ts.transpileModule(effect.getText(file), { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  let generation = "machine-a";
  const mount = (key, value) => {
    let cleanup;
    const live = { current: EMPTY_COMPOSER_STASH };
    const bindings = {
      useLayoutEffect: (callback) => { cleanup = callback(); }, composerStashes: stashes,
      stashKey: `${generation}:${key}`, composerPrefill: null, composerLiveRef: live, pendingClientTurn: { current: null }, pendingScope: { current: null },
      EMPTY_COMPOSER_STASH, composerStashContent,
      mountedComposers: new Map(), deletedSessionIds: new Set(), activeId: key, sideParentSessionId: null,
      isCurrentScope: () => true, sessionsOptions: { queryKey: ["workspace", generation] },
      setDraft: () => {}, setAttachments: () => {}, setAnnotations: () => {}, setUnconfirmedSend: () => {}, setImageRefusedFor: () => {},
    };
    new Function(...Object.keys(bindings), code)(...Object.values(bindings));
    live.current = value;
    return () => cleanup();
  };
  const first = { draft: "first machine", attachments: [], annotations: [] };
  const second = { draft: "second machine", attachments: [], annotations: [] };
  const cleanup = mount("session", first);
  generation = "machine-b";
  mount("session", second)();
  cleanup();
  assert.deepEqual(stashes.get("machine-a:session"), first);
  assert.deepEqual(stashes.get("machine-b:session"), second);
});

test("a file read finishing after a workspace change cannot update or recreate a stash", () => {
  let addFiles;
  function visit(node) {
    if (ts.isFunctionDeclaration(node) && node.name?.text === "addFiles") addFiles = node;
    ts.forEachChild(node, visit);
  }
  visit(file);
  assert.ok(addFiles);
  const code = ts.transpileModule(`${addFiles.getText(file)}\nreturn addFiles;`, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  let reader;
  let generation = 1;
  let updates = 0;
  const stashes = new Map();
  const bindings = {
    FileReader: class { constructor() { reader = this; this.result = "data:,image"; } readAsDataURL() {} },
    attachments: [], noImageInputError: null, selectedModel: null,
    setAttachError: () => {}, setImageRefusedFor: () => {}, setAttachments: () => { updates++; },
    composerScopeRef: { current: { projectId: "p", activeId: "s" } }, composerMountedRef: { current: true },
    stashKey: "1:side:s", composerStashes: stashes, EMPTY_COMPOSER_STASH,
    sessionsOptions: { queryKey: ["workspace", 1] }, isCurrentScope: (scope) => scope[1] === generation,
  };
  const add = new Function(...Object.keys(bindings), code)(...Object.values(bindings));
  add([{ type: "image/png", name: "image.png", size: 1 }]);
  generation = 2;
  reader.onload();
  bindings.composerMountedRef.current = false;
  reader.onload();
  assert.equal(updates, 0);
  assert.equal(stashes.size, 0);
});
