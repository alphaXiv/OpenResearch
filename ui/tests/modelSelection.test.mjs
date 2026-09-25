import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

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
    `${js}; return deriveComposerSelection;`,
  );
  return factory(
    reconcile.serviceTier ?? ((_harness, _model, current) => current ?? null),
    reconcile.reasoning ?? ((_harness, _model, current) => current),
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
