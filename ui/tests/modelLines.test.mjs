import assert from "node:assert/strict";
import test from "node:test";
import { defaultOffer, groupModelLines, newLines, variantOffer } from "../src/modelLines.ts";
import { model } from "./models.mjs";

const PLAIN = { effort: null, speed: null, fast: false, thinking: false, context: null };
/** An offer of a model as the server names it: `key` is `family@version`. */
const offer = (tile, id, label, family, version, fields = {}) => ({
  tile,
  model: model(id, label, { key: version ? `${family}@${version.join(".")}` : family, family, series: family, version, label, ...fields }),
});

test("offers of one key read as one line, named by the catalog that spells it most fully", () => {
  const lines = groupModelLines([
    offer("codex", "gpt-6.1-sol", "GPT-6.1-Sol", "gpt sol", [6, 1]),
    offer("go", "opencode-go/claude-haiku-5-5", "Claude Haiku 5.5", "claude haiku", [5, 5]),
    offer("claude", "haiku", "Haiku 5.5", "claude haiku", [5, 5]),
  ]);
  assert.deepEqual(lines.map((line) => [line.label, line.offers.map((o) => o.tile)]), [
    ["GPT-6.1-Sol", ["codex"]],
    ["Claude Haiku 5.5", ["go", "claude"]],
  ]);
});

test("a tile runs its plainest variant, and switching one keeps the rest the catalog allows", () => {
  const variant = (id, rank, fields) => offer("cursor", id, "Claude Opus 5.5", "claude opus", [5, 5], { variant: { ...PLAIN, context: "1m", ...fields }, variantRank: rank });
  const [line] = groupModelLines([
    variant("claude-opus-5-5-low", 2, { effort: "low" }),
    variant("claude-opus-5-5-medium", 1, { effort: "medium" }),
    variant("claude-opus-5-5-medium-fast", 3, { effort: "medium", speed: "fast", fast: true }),
    variant("claude-opus-5-5-xhigh", 2, { effort: "xhigh" }),
  ]);
  const plain = defaultOffer(line.offers);
  assert.equal(plain.model.id, "claude-opus-5-5-medium");
  assert.equal(variantOffer(line.offers, plain.model.parsed.variant, { speed: "fast" }).model.id, "claude-opus-5-5-medium-fast");
  assert.equal(variantOffer(line.offers, plain.model.parsed.variant, { effort: "max" }), undefined);
});

test("a newer version in a series makes the older ones old, behind the newest", () => {
  const lines = groupModelLines([
    offer("codex", "gpt-5.6-sol", "GPT-5.6-Sol", "gpt sol", [5, 6]),
    offer("codex", "gpt-6.1-sol", "GPT-6.1-Sol", "gpt sol", [6, 1]),
    offer("codex", "gpt-6-luna", "GPT-6 Luna", "gpt luna", [6]),
    offer("go", "opencode-go/kimi-k3", "Kimi K3", "k kimi", [3]),
    offer("go", "opencode-go/kimi-k3.1-pro", "Kimi K3.1 Pro", "k kimi pro", [3, 1], { series: "k kimi" }),
    offer("zen", "opencode/big-pickle", "Big Pickle", "big pickle", null),
  ]);
  assert.deepEqual(lines.map((line) => [line.label, line.older]), [
    ["GPT-6.1-Sol", false],
    ["GPT-5.6-Sol", true],
    ["GPT-6 Luna", false],
    ["Kimi K3", true],
    ["Kimi K3.1 Pro", false],
    ["Big Pickle", false],
  ]);
});

test("a model is new for a week after models.dev dates its release, whichever day an agent starts offering it", () => {
  const lines = groupModelLines([
    offer("opencode", "anthropic/claude-fable-5-2", "Claude Fable 5.2", "claude fable", [5, 2]),
    offer("opencode", "anthropic/claude-fable-5-1", "Claude Fable 5.1", "claude fable", [5, 1]),
    offer("claude", "haiku", "Haiku 5.5", "claude haiku", [5, 5]),
    offer("opencode", "openai/gpt-6.1-terra", "GPT-6.1 Terra", "gpt terra", [6, 1]),
    offer("opencode", "opencode/deepseek-v4.1-flash-free", "DeepSeek V4.1 Flash", "deepseek flash", [4, 1], { key: "deepseek flash@4.1:free" }),
    offer("codex", "gpt-6.1-sol", "GPT-6.1-Sol", "gpt sol", [6, 1]),
  ]);
  const releases = {
    "claude fable@5.2": "2026-11-09",
    "claude fable@5.1": "2026-11-10",
    "claude haiku@5.5": "2026-10-07",
    "gpt terra@6.1": "2026-11-12",
    "deepseek flash@4.1": "2026-11-11",
    "gpt sol@6.1": "2026-11-17",
  };
  const on = (day, hour) => new Date(2026, 10, day, hour).getTime();
  const named = (now) => newLines(lines, releases, now).map((line) => line.label);

  assert.deepEqual(named(on(15, 23)), ["GPT-6.1 Terra", "DeepSeek V4.1 Flash", "Claude Fable 5.2"]);
  assert.deepEqual(named(on(16, 0)), ["GPT-6.1 Terra", "DeepSeek V4.1 Flash"]);
  assert.deepEqual(named(on(17, 0)), ["GPT-6.1-Sol", "GPT-6.1 Terra", "DeepSeek V4.1 Flash"]);
});
