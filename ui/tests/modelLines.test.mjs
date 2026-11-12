import assert from "node:assert/strict";
import test from "node:test";
import { defaultOffer, groupModelLines, parseModel, sight, variantOffer } from "../src/modelLines.ts";

const offer = (tile, harness, id, displayName, free) => ({ tile, model: { id, displayName, free }, parsed: parseModel(harness, { id, displayName, free }) });

test("one model reads as one line across catalogs", () => {
  // Names as Codex, OpenCode, Cursor, Antigravity and DeepSeek Harness report them.
  const lines = groupModelLines([
    offer("codex", "codex", "gpt-6.1-sol", "GPT-6.1-Sol"),
    offer("go", "opencode", "opencode-go/claude-haiku-5-5", "Claude Haiku 5.5"),
    offer("cursor", "cursor", "claude-haiku-5-5-thinking-medium", "Claude Haiku 5.5  Medium"),
    offer("claude", "claude-code", "haiku", "Haiku 5.5"),
    offer("cursor", "cursor", "claude-4.6-sonnet-medium", "Claude 4.6 Sonnet"),
    offer("antigravity", "antigravity", "claude-sonnet-4-6", "Claude Sonnet 4.6 (Thinking)"),
    offer("deepseek", "opencode", "deepseek/deepseek-flash", "DeepSeek V4.1 Flash 1017"),
    offer("dsh", "acp:dee4a440-fd64-4217-a4a7-1d4c148c0dd1", '["deepseek-official","deepseek-flash"]', "DeepSeek-V4.1-Flash"),
  ]);
  const tiles = Object.fromEntries(lines.map((line) => [line.label, [...new Set(line.offers.map((o) => o.tile))]]));
  assert.deepEqual(tiles, {
    "GPT-6.1-Sol": ["codex"],
    "Claude Haiku 5.5": ["go", "cursor", "claude"],
    "Claude 4.6 Sonnet": ["cursor", "antigravity"],
    "DeepSeek V4.1 Flash": ["deepseek", "dsh"],
  });
});

test("Cursor's effort, speed and thinking variants fold into their model", () => {
  const names = [
    ["claude-opus-5-5-low", "Claude Opus 5.5 1M Low"],
    ["claude-opus-5-5-medium", "Claude Opus 5.5 1M"],
    ["claude-opus-5-5-medium-fast", "Claude Opus 5.5 1M Fast"],
    ["claude-opus-5-5-xhigh", "Claude Opus 5.5 1M Extra High"],
    ["gpt-5.5-extra-high", "GPT-5.5 1M Extra High"],
    ["grok-4.7-low-fast", "Grok 4.7  Low Fast\u200b\u200b"],
  ];
  const offers = names.map(([id, name]) => offer("cursor", "cursor", id, name));
  const lines = groupModelLines(offers);
  assert.deepEqual(lines.map((line) => [line.label, line.offers.length]), [["Claude Opus 5.5", 4], ["GPT-5.5", 1], ["Grok 4.7", 1]]);
  // The variant Cursor names plainly runs by default.
  assert.equal(defaultOffer(lines[0].offers).model.id, "claude-opus-5-5-medium");
  assert.deepEqual(offers[4].parsed.variant, { effort: "xhigh", fast: false, thinking: false, context: "1m" });
  assert.deepEqual(offers[5].parsed.variant, { effort: "low", fast: true, thinking: false, context: null });
  // Fast keeps the effort it can.
  const fast = variantOffer(lines[0].offers, offers[1].parsed.variant, { fast: true });
  assert.equal(fast.model.id, "claude-opus-5-5-medium-fast");
  assert.equal(variantOffer(lines[0].offers, offers[1].parsed.variant, { effort: "max" }), undefined);
});

test("Claude Code's long context is a variant of the same model", () => {
  const plain = offer("claude", "claude-code", "claude-opus-5-5", "Opus 5.5");
  const long = offer("claude", "claude-code", "opus[1m]", "Opus 5.5 with 1M context");
  const [line] = groupModelLines([plain, long]);
  assert.equal(line.label, "Opus 5.5");
  assert.equal(long.parsed.variant.context, "1m");
  assert.equal(defaultOffer(line.offers), plain);
});

test("a newer version of any tier makes the older ones old, behind the newest", () => {
  const lines = groupModelLines([
    offer("codex", "codex", "gpt-5.6-sol", "GPT-5.6-Sol"),
    offer("codex", "codex", "gpt-6.1-sol", "GPT-6.1-Sol"),
    offer("codex", "codex", "gpt-6-sol", "GPT-6-Sol"),
    offer("codex", "codex", "gpt-6-luna", "GPT-6 Luna"),
    offer("go", "opencode", "opencode-go/kimi-k3", "Kimi K3"),
    offer("go", "opencode", "opencode-go/kimi-k3.1-pro", "Kimi K3.1 Pro"),
    offer("go", "opencode", "opencode-go/qwen3.8-max", "Qwen3.8 Max"),
    offer("go", "opencode", "opencode-go/qwen3.7-plus", "Qwen3.7 Plus"),
    offer("or", "opencode", "openrouter/qwen/qwen3.6-35b-a3b", "Qwen3.6 35B A3B"),
    offer("claude", "claude-code", "haiku", "Haiku 4.5"),
    offer("claude", "claude-code", "opus", "Opus 5.5"),
  ]);
  assert.deepEqual(lines.map((line) => [line.label, line.older]), [
    ["GPT-6.1-Sol", false],
    ["GPT-6-Sol", true],
    ["GPT-5.6-Sol", true],
    // Luna is a model of its own, and so is each of Claude's tiers.
    ["GPT-6 Luna", false],
    ["Kimi K3", true],
    ["Kimi K3.1 Pro", false],
    ["Qwen3.8 Max", false],
    ["Qwen3.7 Plus", true],
    ["Qwen3.6 35B A3B", true],
    ["Haiku 4.5", false],
    ["Opus 5.5", false],
  ]);
});

test("a free tier is its own model, named without Free", () => {
  const lines = groupModelLines([
    offer("go", "opencode", "opencode-go/mimo-v2.6-flash", "MiMo V2.6 Flash"),
    offer("zen", "opencode", "opencode/mimo-v2.6-flash-free", "MiMo V2.6 Flash Free", true),
    offer("or", "opencode", "openrouter/deepseek/deepseek-r1:free", "DeepSeek R1 (free)", true),
  ]);
  assert.deepEqual(lines.map((line) => [line.label, line.key]), [
    ["MiMo V2.6 Flash", "flash mimo v@2.6"],
    ["MiMo V2.6 Flash", "flash mimo v@2.6:free"],
    ["DeepSeek R1", "deepseek r@1:free"],
  ]);
});

test("a model is new when an agent already seen starts offering it", () => {
  const codex = { key: "codex" };
  const lines = (...offers) => groupModelLines(offers.map(([tile, id, name]) => ({ tile, model: { id, displayName: name }, parsed: parseModel("codex", { id, displayName: name }) })));
  const first = sight(null, lines([codex, "gpt-6.1-sol", "GPT-6.1 Sol"]), 1);
  assert.deepEqual(first, { agents: ["codex"], models: { "gpt sol@6.1": 0 }, openedAt: 1 });
  assert.equal(sight(first, lines([codex, "gpt-6.1-sol", "GPT-6.1 Sol"]), 2), first);

  const zen = { key: "opencode:opencode" };
  const next = sight(first, lines([codex, "gpt-6.2-sol", "GPT-6.2 Sol"], [codex, "gpt-6.1-sol", "GPT-6.1 Sol"], [zen, "opencode/big-pickle", "Big Pickle"]), 5);
  // The old version that came with it isn't news, and neither is what a newly added agent brings.
  assert.deepEqual(next.models, { "gpt sol@6.1": 0, "gpt sol@6.2": 5, "big pickle": 0 });
  assert.deepEqual(next.agents, ["codex", "opencode:opencode"]);
});
