import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
import { formatCodexModel, groupModels, resolveHarnessDefaultModel } from "../src/modelSelection.ts";

const file = ts.createSourceFile(
  "ChatPanel.tsx",
  readFileSync(new URL("../src/components/ChatPanel.tsx", import.meta.url), "utf8"),
  ts.ScriptTarget.Latest,
  true,
  ts.ScriptKind.TSX,
);

function derive(rawSelection, activeHarness, reconcile = {}) {
  const declaration = file.statements.find(
    (node) => ts.isFunctionDeclaration(node) && node.name?.text === "deriveComposerSelection",
  );
  assert.ok(declaration);
  const js = ts.transpileModule(declaration.getText(file).replace(/^export /, ""), {
    compilerOptions: { target: ts.ScriptTarget.ES2022 },
  }).outputText;
  const factory = new Function(
    "reconcileServiceTier",
    "reconcileReasoning",
    "resolveHarnessDefaultModel",
    `${js}; return deriveComposerSelection;`,
  );
  return factory(
    reconcile.serviceTier ?? ((_harness, _model, current) => current ?? null),
    reconcile.reasoning ?? ((_harness, _model, current) => current),
    resolveHarnessDefaultModel,
  )(rawSelection, activeHarness);
}

const selection = (model) => ({
  harness: "codex",
  model,
  serviceTier: null,
  permissionMode: "auto",
  reasoningLevel: "high",
});

const harness = (...models) => ({
  id: "codex",
  agentReady: true,
  models: models.map((id) => ({ id })),
});

test("an explicitly entered model id survives composer reconciliation and send", () => {
  const customId = "gpt-6-preview";
  const result = derive(selection(customId), harness("gpt-5.5", "gpt-5.6-sol"));

  assert.equal(result.model, customId);
  // Both new-session creation and turn sends consume this derived selection.
  const outgoing = { model: result.model, reasoningLevel: result.reasoningLevel };
  assert.deepEqual(outgoing, { model: customId, reasoningLevel: "high" });
});

test("catalog refreshes do not replace an explicitly entered model id", () => {
  const custom = selection("provider/new-model");
  assert.equal(derive(custom, harness("old-model")).model, custom.model);
  assert.equal(derive(custom, harness("different-model", "new-default")).model, custom.model);
});

test("an empty catalog leaves CLI default and manual IDs selectable", () => {
  assert.equal(derive(selection(null), harness()).model, null);
  assert.equal(derive(selection("provider/new-model"), harness()).model, "provider/new-model");
});

test("Codex legacy defaults resolve to the catalog's newest GPT-Sol", () => {
  const catalog = harness("gpt-6.9-sol", "gpt-6.10-sol", "gpt-6-astra");
  assert.equal(derive(selection(null), catalog).model, "gpt-6.10-sol");
  assert.equal(derive(selection("default"), catalog).model, "gpt-6.10-sol");
  assert.equal(derive(selection(null), { ...catalog, catalogPending: true }).model, null);
  assert.equal(derive(selection(null), harness("gpt-6-astra", "other")).model, "gpt-6-astra");
});

test("an unready Codex catalog does not resolve a legacy default or replace an explicit model", () => {
  const catalog = { ...harness("gpt-6.10-sol"), agentReady: false, catalogPending: false };
  assert.equal(resolveHarnessDefaultModel(catalog), null);
  assert.equal(derive(selection(null), catalog).model, null);
  assert.equal(derive(selection("default"), catalog).model, "default");
  assert.equal(derive(selection("my-model"), catalog).model, "my-model");
  assert.equal(derive(selection(null), { ...catalog, agentReady: true }).model, "gpt-6.10-sol");
});

test("default resolution uses labels and preserves other harness semantics", () => {
  assert.equal(resolveHarnessDefaultModel({ id: "codex", agentReady: true, models: [
    { id: "a", displayName: "GPT 6.9 Sol" },
    { id: "b", displayName: "gPt 6.10 sOl" },
  ] }), "b");
  for (const id of ["cursor", "antigravity", "acp:example"]) {
    assert.equal(resolveHarnessDefaultModel({ id, models: [{ id: "first" }] }), null);
    assert.equal(derive({ ...selection(null), harness: id }, { id, models: [{ id: "first" }] }).model, null);
  }
  assert.equal(resolveHarnessDefaultModel({ id: "opencode", models: [{ id: "configured" }] }), "configured");
  assert.equal(resolveHarnessDefaultModel({ id: "opencode", models: [] }), null);
});

test("catalog models still reconcile their model-specific settings", () => {
  const selected = selection("gpt-5.5");
  const result = derive(selected, harness("gpt-5.5"), {
    serviceTier: (_harness, model) => model === "gpt-5.5" ? "default" : null,
    reasoning: (_harness, model) => model === "gpt-5.5" ? "medium" : null,
  });

  assert.equal(result.model, "gpt-5.5");
  assert.equal(result.serviceTier, "default");
  assert.equal(result.reasoningLevel, "medium");
});


test("model families show newest first and keep older versions searchable", () => {
  const models = ["Haiku 4.5", "Sonnet 5", "Opus 5.5", "Fable 5.1", "Haiku 5.5", "Opus 4.8", "Mythos 6"].map((name) => ({ id: name, displayName: name }));
  const grouped = groupModels("claude-code", models);
  assert.deepEqual(grouped.current.map((m) => m.id), ["Mythos 6", "Fable 5.1", "Opus 5.5", "Sonnet 5", "Haiku 5.5"]);
  assert.deepEqual(grouped.legacy.map((m) => m.id), ["Opus 4.8", "Haiku 4.5"]);
  const codex = groupModels("codex", ["gpt-6-sol", "gpt-6-luna", "gpt-6.1-terra", "gpt-6-astra", "gpt-6.1-sol"].map((id) => ({ id })));
  assert.deepEqual(codex.current.map((m) => m.id), ["gpt-6-astra", "gpt-6.1-sol", "gpt-6.1-terra", "gpt-6-luna"]);
  assert.deepEqual(codex.legacy.map((m) => m.id), ["gpt-6-sol"]);
  assert.equal(formatCodexModel("GPT-6.1-Sol"), "GPT-6.1 Sol");
});


test("Claude defaults to the latest available Opus", () => {
  const catalog = { id: "claude-code", agentReady: true, models: [
    { id: "old", displayName: "Opus 5" }, { id: "opus", displayName: "Opus 5.5" }, { id: "fable", displayName: "Fable 6" },
  ] };
  assert.equal(resolveHarnessDefaultModel(catalog), "opus");
  assert.equal(derive({ ...selection(null), harness: "claude-code" }, catalog).model, "opus");
  assert.equal(resolveHarnessDefaultModel({ ...catalog, agentReady: false }), null);
});
