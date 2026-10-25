import assert from "node:assert/strict";
import test from "node:test";
import { opencodeProviderLabel, withProviderSuffix } from "../src/providerLabel.ts";

test("the provider is named, not its raw handle", () => {
  assert.equal(opencodeProviderLabel("openrouter/moonshotai/kimi-k3.1-pro"), "OpenRouter");
  assert.equal(opencodeProviderLabel("opencode-go/kimi-k3.1-pro"), "OpenCode Go");
  assert.equal(opencodeProviderLabel("github-copilot/gpt-5.6-sol"), "GitHub Copilot");
  // A provider we do not list keeps the handle its own catalog uses.
  assert.equal(opencodeProviderLabel("some-new-provider/model-x"), "some-new-provider");
  assert.equal(opencodeProviderLabel("default"), null);
});

test("an OpenCode model carries its provider so duplicates stay distinguishable", () => {
  // The same model name exists on Go and on OpenRouter; only the suffix tells
  // the two rows apart, and they bill and route differently.
  assert.equal(
    withProviderSuffix("Kimi K3.1 Pro", "opencode-go/kimi-k3.1-pro", "opencode"),
    "Kimi K3.1 Pro · OpenCode Go",
  );
  assert.equal(
    withProviderSuffix("Kimi K3.1 Pro", "openrouter/moonshotai/kimi-k3.1-pro", "opencode"),
    "Kimi K3.1 Pro · OpenRouter",
  );
});

test("a label that already names its provider is not repeated", () => {
  assert.equal(
    withProviderSuffix("Big Pickle · OpenCode", "opencode/big-pickle", "opencode"),
    "Big Pickle · OpenCode",
  );
});

test("every other harness keeps its label untouched", () => {
  assert.equal(withProviderSuffix("Opus 4.8 (1M)", "opus[1m]", "claude-code"), "Opus 4.8 (1M)");
  assert.equal(withProviderSuffix("GPT 5.6 Sol", "gpt-5.6-sol", "codex"), "GPT 5.6 Sol");
  // No harness known: never append a segment we cannot attribute.
  assert.equal(withProviderSuffix("Kimi K3.1 Pro", "opencode-go/kimi-k3.1-pro"), "Kimi K3.1 Pro");
});
