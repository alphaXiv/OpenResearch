import assert from "node:assert/strict";
import test from "node:test";
import { tidyModelName as tidy } from "../src/api.ts";
import { deriveComposerSelection as derive, harnessDisplayName, selectionHarness } from "../src/modelSelection.ts";
import { model } from "./models.mjs";

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
  models: models.map((id) => model(id)),
  options: { reasoningLevels: [{ id: "default", label: "Default" }, { id: "high", label: "High" }] },
});

test("a saved model the catalog doesn't list survives composer reconciliation and send", () => {
  const saved = "gpt-6-preview";
  const result = derive(selection(saved), harness("gpt-5.5", "gpt-5.6-sol"));

  assert.equal(result.model, saved);
  // Both new-session creation and turn sends consume this derived selection.
  const outgoing = { model: result.model, reasoningLevel: result.reasoningLevel };
  assert.deepEqual(outgoing, { model: saved, reasoningLevel: "high" });
});

test("catalog refreshes do not replace a saved model", () => {
  const saved = selection("provider/new-model");
  assert.equal(derive(saved, harness("old-model")).model, saved.model);
  assert.equal(derive(saved, harness("different-model", "new-default")).model, saved.model);
  assert.equal(derive(saved, harness()).model, saved.model);
});

test("a missing model is never inferred from the catalog", () => {
  assert.equal(derive(selection(null), harness("gpt-6.10-sol")).model, null);
  const claude = { id: "claude-code", agentReady: true, models: [model("opus", "Opus 5.5")] };
  assert.equal(derive({ ...selection(null), harness: "claude-code" }, claude).model, null);
});

test("catalog models still reconcile their model-specific settings", () => {
  const catalog = { ...harness(), models: [{
    id: "gpt-5.5",
    serviceTiers: [{ id: "priority", label: "Fast" }],
    reasoningLevels: [{ id: "low", label: "Low" }, { id: "medium", label: "Medium" }],
    defaultReasoningLevel: "medium",
  }] };
  const result = derive(selection("gpt-5.5"), catalog);

  assert.equal(result.model, "gpt-5.5");
  assert.equal(result.serviceTier, "default");
  assert.equal(result.reasoningLevel, "medium");
});

test("a selection finds its agent's entry, and its account's when it names one", () => {
  const own = { id: "claude-code", name: "Claude Code" };
  const work = { id: "claude-code", name: "Claude Code", profile: { id: "claude-0123456789", name: "Work" } };
  const second = { id: "claude-code", name: "Claude Code", profile: { id: "claude-abcdefghij", name: "Claude Code 2" } };
  const harnesses = [work, own, second];
  assert.equal(selectionHarness(harnesses, { harness: "claude-code", profile: null }), own);
  assert.equal(selectionHarness(harnesses, { harness: "claude-code" }), own);
  assert.equal(selectionHarness(harnesses, { harness: "claude-code", profile: "claude-0123456789" }), work);
  // A removed account matches nothing rather than the user's own sign-in.
  assert.equal(selectionHarness(harnesses, { harness: "claude-code", profile: "claude-removed000" }), undefined);
  assert.equal(selectionHarness(harnesses, null), undefined);
  assert.equal(harnessDisplayName(own), "Claude Code");
  assert.equal(harnessDisplayName(work), "Claude Code · Work");
  assert.equal(harnessDisplayName(second), "Claude Code 2");
});

test("model names read the same across catalogs", () => {
  // DeepSeek Harness names one model by its id and the rest with hyphens; OpenCode's catalog spaces them.
  assert.equal(tidy("deepseek-v4-flash"), "DeepSeek V4 Flash");
  assert.equal(tidy("DeepSeek-V4.1-Flash"), "DeepSeek V4.1 Flash");
  assert.equal(tidy("MiMo-V2.6-Pro"), "MiMo V2.6 Pro");
  assert.equal(tidy("LongCat-2.0"), "LongCat 2.0");
  assert.equal(tidy("Hy4 preview"), "Hy4 Preview");
  // Labs that write their version on keep it.
  assert.equal(tidy("GLM-5.3-Flash"), "GLM-5.3 Flash");
  assert.equal(tidy("GPT-6.1-Sol"), "GPT-6.1 Sol");
  assert.equal(tidy("GPT-OSS 120B (Medium)"), "GPT-OSS 120B (Medium)");
  // Cursor pads some names with doubled and zero-width spaces.
  assert.equal(tidy("Grok 4.7  Low Fast\u200b\u200b"), "Grok 4.7 Low Fast");
  assert.equal(tidy("Default (recommended)"), "Default (recommended)");
  // A second key's name is the user's own.
  assert.equal(tidy("Kimi-K3 · my work key"), "Kimi K3 · my work key");
});
