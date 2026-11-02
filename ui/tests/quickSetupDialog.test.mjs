import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import React from "react";
import ts from "typescript";

const source = ts.createSourceFile("SettingsPage.tsx", readFileSync(new URL("../src/components/SettingsPage.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
let renderTarget;
function visit(node) {
  if (ts.isVariableDeclaration(node) && node.name.getText(source) === "renderTarget") renderTarget = node.initializer;
  ts.forEachChild(node, visit);
}
visit(source);

function openSetup() {
  let dialog;
  let discard;
  const bindings = {
    React,
    TargetTile: "target-tile",
    QuickSetupDialog: "quick-setup",
    project: { id: "project" },
    defaultBackend: "local",
    remote: false,
    QUICK_SETUP_TARGETS: ["k8s"],
    TARGET_LABELS: { k8s: () => "Kubernetes" },
    m: { settings_k8s_discard_changes: () => "Discard changes?" },
    setSelectedTarget: () => assert.fail("Quick Setup must open a dialog"),
    onOpenEnvironment: () => {},
    showCustomDialog: (options) => { dialog = options; },
    showConfirmDialog: (options) => { discard = options; },
  };
  const code = ts.transpileModule(`return (${renderTarget.getText(source)});`, {
    compilerOptions: { jsx: ts.JsxEmit.React, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  const render = new Function(...Object.keys(bindings), code)(...Object.values(bindings));
  render({ id: "k8s" }).props.onOpen();
  return { dialog, get discard() { return discard; } };
}

test("Quick Setup closes clean edits and blocks dismissal while saving", async () => {
  const setup = openSetup();
  assert.equal(setup.dialog.onCancel(), undefined);
  setup.dialog.props.onEditState({ dirty: true, saving: true });
  await assert.rejects(setup.dialog.onCancel());
  assert.equal(setup.discard, undefined);
});

test("Quick Setup awaits discard consent and keeps dirty edits when consent is cancelled", async () => {
  const setup = openSetup();
  setup.dialog.props.onEditState({ dirty: true, saving: false });
  const cancelled = setup.dialog.onCancel();
  assert.ok(setup.discard.destructive);
  setup.discard.onCancel();
  await assert.rejects(cancelled);
  const accepted = setup.dialog.onCancel();
  setup.discard.onConfirm();
  await accepted;
});
