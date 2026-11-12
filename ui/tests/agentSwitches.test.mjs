import assert from "node:assert/strict";
import test from "node:test";
import { agentOffers, pickerAgents } from "../src/components/pickerAgents.tsx";
import { agentModelLines } from "../src/modelSelection.ts";

const harness = (id, fields = {}) => ({ id, name: id, agentReady: true, models: [], ...fields });
const harnesses = [
  harness("codex"),
  harness("codex", { profile: { id: "codex-work", name: "Work" } }),
  harness("cursor"),
  harness("antigravity", { managed: true }),
  harness("opencode", { managed: true, off: true, models: [{ id: "anthropic/opus" }, { id: "github-copilot/gpt" }, { id: "opencode/free" }] }),
];
const providers = [{ id: "anthropic", name: "Anthropic" }];

test("OpenCode turned off drops only the sign-ins made in OpenCode itself, unless it is the one picked", () => {
  const keys = (kept) => pickerAgents(harnesses, providers, [], kept).map((agent) => agent.key).sort();
  const records = ["codex", "codex-work", "cursor", "antigravity", "opencode:anthropic", "opencode:opencode"];
  assert.deepEqual(keys(), records.sort());
  assert.deepEqual(keys("opencode"), [...records, "opencode:github-copilot"].sort());
});

test("an agent's model count counts the picker's lines, older versions last", () => {
  const models = [
    { id: "gpt-5.3-codex-low", displayName: "Codex 5.3 Low" },
    { id: "gpt-5.3-codex-low-fast", displayName: "Codex 5.3 Low Fast" },
    { id: "gpt-5.3-codex", displayName: "Codex 5.3" },
    { id: "gpt-5.3-codex-fast", displayName: "Codex 5.3 Fast" },
    { id: "gpt-5.2-codex-high", displayName: "Codex 5.2 High" },
    { id: "claude-opus-5-5-medium", displayName: "Claude Opus 5.5 Medium" },
    { id: "claude-opus-5-5-thinking-high", displayName: "Claude Opus 5.5 High Thinking" },
  ];
  assert.deepEqual(agentModelLines("cursor", models).map((line) => line.label), ["Codex 5.3", "Claude Opus 5.5", "Codex 5.2"]);
});

test("only a ready agent's listed models are offered", () => {
  const offered = (harnesses) => pickerAgents(harnesses, [], []).flatMap(agentOffers).map((offer) => `${offer.tile.key} ${offer.model.id}`);
  const signedOut = harness("claude-code", { agentReady: false, models: [{ id: "opus" }] });
  const modelless = harness("acp:kimi");
  assert.deepEqual(offered([signedOut, modelless]), []);
  assert.deepEqual(offered([signedOut, modelless, harness("codex", { models: [{ id: "gpt-6.1-sol" }] })]), ["codex gpt-6.1-sol"]);
});

test("a switch notice names the model as the picker does", async () => {
  const { assistantMessage, codex, renderChat, session, userMessage } = await import("./chat.mjs");
  const { m } = await import("../src/paraglide/messages.js");
  const cursor = {
    ...codex, id: "cursor", name: "Cursor", defaultModel: "claude-opus-5-5-medium",
    models: [{ id: "claude-opus-5-5-medium", displayName: "Opus 5.5 Medium" }, { id: "claude-opus-5-5-high", displayName: "Opus 5.5 High" }],
  };
  const claude = { ...codex, id: "claude-code", name: "Claude Code", defaultModel: "claude-opus-5-5", models: [{ id: "claude-opus-5-5", displayName: "Claude Opus 5.5" }] };
  const input = { fromHarness: "codex", fromModel: "gpt-6.1-sol", harness: "cursor", profile: null, name: "Cursor", model: "claude-opus-5-5-medium" };
  const transcript = [
    userMessage("u", "Hello"),
    assistantMessage("switch", [{ id: "switch-part", type: "tool", tool: "provider_switch", state: { status: "completed", input } }], "u"),
  ];
  await renderChat("/projects/p/tasks/s", {
    sessions: [session("s", { harness: "cursor", model: "claude-opus-5-5-medium" })],
    transcripts: { s: transcript },
    harnesses: [codex, cursor, claude],
  });

  const trigger = document.querySelector('[data-onboarding="model-picker"] button').textContent;
  assert.match(trigger, /Claude Opus 5\.5/);
  assert.equal(document.querySelector("[data-provider-switch]").textContent, m.chat_switched_agent({ agent: "Cursor", model: "Claude Opus 5.5" }));
});
