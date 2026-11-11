import assert from "node:assert/strict";
import test from "node:test";
import { defaultOffer, groupModelLines, sight, variantOffer } from "../src/modelLines.ts";
import { model } from "./models.mjs";

const PLAIN = { effort: null, fast: false, thinking: false, context: null };
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
    variant("claude-opus-5-5-medium-fast", 3, { effort: "medium", fast: true }),
    variant("claude-opus-5-5-xhigh", 2, { effort: "xhigh" }),
  ]);
  const plain = defaultOffer(line.offers);
  assert.equal(plain.model.id, "claude-opus-5-5-medium");
  assert.equal(variantOffer(line.offers, plain.model.parsed.variant, { fast: true }).model.id, "claude-opus-5-5-medium-fast");
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

test("a model is new when an agent already seen starts offering it", () => {
  const codex = { key: "codex" };
  const lines = (...offers) => groupModelLines(offers.map(([tile, id, label, family, version]) => offer(tile, id, label, family, version)));
  const first = sight(null, lines([codex, "gpt-6.1-sol", "GPT-6.1 Sol", "gpt sol", [6, 1]]), 1);
  assert.deepEqual(first, { agents: ["codex"], models: { "gpt sol@6.1": 0 }, openedAt: 1 });
  assert.equal(sight(first, lines([codex, "gpt-6.1-sol", "GPT-6.1 Sol", "gpt sol", [6, 1]]), 2), first);

  const zen = { key: "opencode:opencode" };
  const next = sight(first, lines(
    [codex, "gpt-6.2-sol", "GPT-6.2 Sol", "gpt sol", [6, 2]],
    [codex, "gpt-6.1-sol", "GPT-6.1 Sol", "gpt sol", [6, 1]],
    [zen, "opencode/big-pickle", "Big Pickle", "big pickle", null],
  ), 5);
  // The old version that came with it isn't news, and neither is what a newly added agent brings.
  assert.deepEqual(next.models, { "gpt sol@6.1": 0, "gpt sol@6.2": 5, "big pickle": 0 });
  assert.deepEqual(next.agents, ["codex", "opencode:opencode"]);
});
