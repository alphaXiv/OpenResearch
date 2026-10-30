import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import { dialogHarness } from "./dialogHarness.mjs";

function component(name) {
  return ts.createSourceFile(name, readFileSync(new URL(`../src/components/${name}`, import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
}

function find(node, predicate) {
  if (predicate(node)) return node;
  return ts.forEachChild(node, (child) => find(child, predicate));
}

function evaluate(code, bindings) {
  const js = ts.transpileModule(`return (${code});`, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  return new Function(...Object.keys(bindings), js)(...Object.values(bindings));
}

test("onboarding selects the registered profile after a preset connects and refreshes", async () => {
  const dialog = component("AcpHarnessDialog.tsx");
  const run = find(dialog, (node) => ts.isVariableDeclaration(node) && node.name.getText(dialog) === "run").initializer.getText(dialog);
  const onboarding = component("Onboarding.tsx");
  const savedCallback = find(onboarding, (node) => ts.isPropertyAssignment(node) && node.name.getText(onboarding) === "onSaved").initializer.getText(onboarding);
  const selectionEffect = find(onboarding, (node) => ts.isCallExpression(node) && node.expression.getText(onboarding) === "useEffect" && node.getText(onboarding).includes("const added = ready.find")).arguments[0].getText(onboarding);
  const preset = { id: "acp:preset", name: "Kimi Code", executable: "kimi", arguments: ["acp"] };
  const addedAcpHarness = { current: null };
  let preferred = "codex", refreshes = 0, saved = 0, closed = 0;
  let harnesses = [{ id: "codex", agentReady: true }, { id: "acp:saved", agentReady: true, catalogPending: true }];
  const setPreferredHarness = (update) => { preferred = typeof update === "function" ? update(preferred) : update; };
  const onSaved = evaluate(savedCallback, { queryClient: { getQueryData: () => harnesses }, getHarnessesQuery: () => ({ queryKey: [] }), addedAcpHarness, setPreferredHarness, load: (refresh, retry) => { assert.deepEqual([refresh, retry], [true, true]); refreshes++; } });
  const harness = dialogHarness();
  const connect = async (launch, harnessId) => {
    const save = evaluate(run, {
      cancelled: { current: false }, busy: false, valid: true, launch, preset: preset.id, presets: { data: [preset] }, definition: undefined,
      setBusy: () => {}, setConnected: () => {}, setError: (error) => { if (error) assert.fail(error); },
      testAcpHarness: async () => ({ harnessId }), saveAcpHarness: async (value) => { saved++; return { ...value, id: "acp:new" }; },
      onSaved, name: launch.name, m: {},
    });
    harness.showCustomDialog({ component: () => null, props: {}, onConfirm: (submit) => submit(), onCancel: "close" });
    const opened = harness.render()[0];
    opened.props.children.props.confirm(() => save(false));
    assert.equal(harness.render()[0].props.children.props.isDisabled, true);
    assert.equal(harness.render()[0].props.onOpenChange, undefined);
    await new Promise(setImmediate);
    const closing = harness.render()[0];
    assert.equal(closing.props.open, false);
    assert.equal(harness.render().length, 1);
    closing.props.onOpenChangeComplete(false);
    assert.equal(harness.render().length, 0);
    closed++;
  };
  const refresh = (snapshot) => {
    harnesses = snapshot;
    evaluate(selectionEffect, { harnesses, addedAcpHarness, preferredAgent: null, setPreferredHarness })();
  };

  await connect({ name: preset.name, executable: preset.executable, arguments: preset.arguments }, "acp:saved");
  assert.equal(addedAcpHarness.current, "acp:saved");
  assert.equal(saved, 0);
  refresh([{ id: "codex", agentReady: true }, { id: "acp:saved", agentReady: true, catalogPending: true }]);
  assert.equal(preferred, "codex");
  refresh([{ id: "codex", agentReady: true }, { id: "acp:saved", agentReady: true, catalogPending: false }]);
  assert.equal(preferred, "acp:saved");
  assert.equal(addedAcpHarness.current, null);
  preferred = "codex";
  refresh([{ id: "codex", agentReady: true }, { id: "acp:saved", agentReady: true }]);
  assert.equal(preferred, "codex");
  await connect({ name: preset.name, executable: preset.executable, arguments: preset.arguments }, "acp:saved");
  assert.equal(preferred, "acp:saved");
  assert.equal(addedAcpHarness.current, null);

  await connect({ name: preset.name, executable: preset.executable, arguments: preset.arguments }, "acp:preset");
  assert.equal(addedAcpHarness.current, "acp:preset");
  await connect({ name: "My configuration", executable: preset.executable, arguments: preset.arguments }, "acp:saved");
  assert.equal(addedAcpHarness.current, "acp:new");
  assert.equal(saved, 1);
  assert.equal(refreshes, 4);
  assert.equal(closed, 4);
});
