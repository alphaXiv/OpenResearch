import assert from "node:assert/strict";
import test from "node:test";
import { agentOffers, pickerAgents } from "../src/components/pickerAgents.tsx";
import { agentModelLines } from "../src/modelSelection.ts";
import { model } from "./models.mjs";

const harness = (id, fields = {}) => ({ id, name: id, agentReady: true, models: [], ...fields });
const harnesses = [
  harness("codex"),
  harness("codex", { profile: { id: "codex-work", name: "Work" } }),
  harness("cursor"),
  harness("antigravity", { managed: true }),
  harness("opencode", { managed: true, off: true, models: [model("anthropic/opus"), model("github-copilot/gpt"), model("opencode/free")] }),
];
const providers = [{ id: "anthropic", name: "Anthropic" }];

test("OpenCode turned off by an older server drops only the sign-ins made in OpenCode itself, unless one is picked", () => {
  const keys = (kept) => pickerAgents(harnesses, providers, [], kept).map((agent) => agent.key).sort();
  const records = ["codex", "codex-work", "cursor", "antigravity", "opencode:anthropic", "opencode:opencode"];
  assert.deepEqual(keys(), records.sort());
  assert.deepEqual(keys({ harness: "opencode", profile: null, model: "github-copilot/gpt" }), [...records, "opencode:github-copilot"].sort());
});

test("each account and OpenCode provider turns off on its own, unless it is the one picked", () => {
  const opencode = {
    ...harness("opencode", { models: [model("anthropic/opus"), model("tinker/inkling"), model("opencode/free")] }),
    foundProviders: [{ id: "tinker", sources: ["config"], removable: true }],
    offProviders: ["anthropic", "tinker"],
  };
  const agents = [harness("codex"), harness("codex", { off: true, profile: { id: "codex-work", name: "Work" } }), opencode];
  const keys = (kept) => pickerAgents(agents, providers, [], kept).map((agent) => agent.key).sort();
  assert.deepEqual(keys(), ["codex", "opencode:opencode"]);
  assert.deepEqual(keys({ harness: "codex", profile: "codex-work", model: null }), ["codex", "codex-work", "opencode:opencode"]);
  assert.deepEqual(keys({ harness: "opencode", profile: null, model: "tinker/inkling" }), ["codex", "opencode:opencode", "opencode:tinker"]);
});

test("the model feed's hosted models run under Free Models, with OpenCode's own", () => {
  const opencode = harness("opencode", {
    models: [model("opencode/free"), model("openresearch-k3v9q2/research-1"), model("tinker/inkling")],
    foundProviders: [{ id: "tinker", sources: ["config"], removable: true }],
  });
  const offered = pickerAgents([opencode], [], []).flatMap((agent) => agentOffers(agent)).map((offer) => `${offer.tile.key} ${offer.model.id}`);
  assert.deepEqual(offered, ["opencode:opencode opencode/free", "opencode:opencode openresearch-k3v9q2/research-1", "opencode:tinker tinker/inkling"]);

  const onlyHosted = harness("opencode", { models: [model("openresearch-k3v9q2/research-1")], foundProviders: [] });
  assert.deepEqual(pickerAgents([onlyHosted], [], []).map((agent) => agent.key), ["opencode:opencode"]);
});

test("a provider found in OpenCode is named and marked by what it points at", async () => {
  const { foundProviderMark } = await import("../src/components/pickerAgents.tsx");
  const { guessLogo, hostLogo } = await import("../src/components/providerIdentity.tsx");
  const name = (provider) => foundProviderMark({ sources: ["config"], removable: false, ...provider }).name;
  assert.equal(name({ id: "thinkingmachines" }), "Thinking Machines");
  assert.equal(name({ id: "tinker" }), "Tinker");
  assert.equal(name({ id: "or", baseUrl: "https://openrouter.ai/api/v1" }), "OpenRouter");
  assert.equal(name({ id: "my-router", name: "Team router" }), "Team router");
  assert.equal(name({ id: "my-router", label: { name: "Mine" } }), "Mine");

  assert.equal(guessLogo("TINKER_API_KEY"), "thinkingmachines");
  assert.equal(guessLogo("thinking-machines"), "thinkingmachines");
  assert.equal(guessLogo("azure-work"), "microsoft");
  assert.equal(guessLogo("deep seek"), "deepseek");
  assert.equal(guessLogo("my-router"), undefined);
  assert.equal(hostLogo("https://tinker.thinkingmachines.dev/services/tinker-prod/oai/api/v1"), "thinkingmachines");
  assert.equal(hostLogo("https://my-team.openai.azure.com/openai"), "microsoft");
  assert.equal(hostLogo("https://bedrock-runtime.us-east-1.amazonaws.com"), "bedrock");
  assert.equal(hostLogo("https://api.example-router.io/v1"), undefined);

  // A provider no logo knows wears a glyph on a colour of its own, the same each time.
  const icon = (id) => foundProviderMark({ id, sources: ["config"], removable: false, baseUrl: "https://api.example-router.io/v1" }).icon;
  assert.ok(icon("my-router").color);
  assert.deepEqual(icon("my-router"), icon("my-router"));
  assert.equal(foundProviderMark({ id: "thinkingmachines", sources: ["env"], removable: false }).icon.color, undefined);
});

test("an agent's model count counts the picker's lines, older versions last", () => {
  // As the server names Cursor's catalog: each effort and speed is a variant of its model.
  const named = (id, label, family, version, variantRank) =>
    model(id, undefined, { key: `${family}@${version.join(".")}`, family, series: family, version, label, variantRank });
  const models = [
    named("gpt-5.3-codex-low", "Codex 5.3", "codex", [5, 3], 3),
    named("gpt-5.3-codex-low-fast", "Codex 5.3", "codex", [5, 3], 5),
    named("gpt-5.3-codex", "Codex 5.3", "codex", [5, 3], 1),
    named("gpt-5.3-codex-fast", "Codex 5.3", "codex", [5, 3], 3),
    named("gpt-5.2-codex-high", "Codex 5.2", "codex", [5, 2], 3),
    named("claude-opus-5-5-medium", "Claude Opus 5.5", "claude opus", [5, 5], 2),
    named("claude-opus-5-5-thinking-high", "Claude Opus 5.5", "claude opus", [5, 5], 5),
  ];
  assert.deepEqual(agentModelLines("cursor", models).map((line) => line.label), ["Codex 5.3", "Claude Opus 5.5", "Codex 5.2"]);

  const alias = model("codex-latest", undefined, { key: "codex", family: "codex", series: "codex", label: "Codex", superseded: true });
  assert.equal(agentModelLines("cursor", [...models, alias]).length, 3);
});

test("only a ready agent's listed models are offered", () => {
  const offered = (harnesses) => pickerAgents(harnesses, [], []).flatMap(agentOffers).map((offer) => `${offer.tile.key} ${offer.model.id}`);
  const signedOut = harness("claude-code", { agentReady: false, models: [model("opus")] });
  const modelless = harness("acp:kimi");
  assert.deepEqual(offered([signedOut, modelless]), []);
  assert.deepEqual(offered([signedOut, modelless, harness("codex", { models: [model("gpt-6.1-sol")] })]), ["codex gpt-6.1-sol"]);
});

test("a switch notice names the model as the picker does", async () => {
  const { assistantMessage, codex, renderChat, session, userMessage } = await import("./chat.mjs");
  const { m } = await import("../src/paraglide/messages.js");
  // As the server names them: one model, whose Cursor variants spell their effort.
  const opus = { ...model("claude-opus-5-5").parsed, key: "claude opus@5.5", family: "claude opus", series: "claude opus", version: [5, 5], label: "Claude Opus 5.5" };
  const cursor = {
    ...codex, id: "cursor", name: "Cursor", defaultModel: "claude-opus-5-5-medium",
    models: [
      model("claude-opus-5-5-medium", "Opus 5.5 Medium", { ...opus, label: "Opus 5.5", variant: { ...opus.variant, effort: "medium" }, variantRank: 2 }),
      model("claude-opus-5-5-high", "Opus 5.5 High", { ...opus, label: "Opus 5.5", variant: { ...opus.variant, effort: "high" }, variantRank: 3 }),
    ],
  };
  const claude = { ...codex, id: "claude-code", name: "Claude Code", defaultModel: "claude-opus-5-5", models: [model("claude-opus-5-5", "Claude Opus 5.5", opus)] };
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

test("a model's speed tiers are one choice in the Effort menu", async () => {
  const { act } = await import("react");
  const { codex, renderChat, session } = await import("./chat.mjs");
  const { settle } = await import("./render.mjs");
  const { m } = await import("../src/paraglide/messages.js");
  // As the server names OpenCode's OpenAI catalog: each speed tier is a variant of GPT-5.
  const gpt5 = { key: "gpt@5", family: "gpt", series: "gpt", version: [5], label: "GPT-5" };
  const tier = (id, speed, variantRank) => model(id, undefined, { ...gpt5, variant: { effort: null, speed, fast: speed === "fast", thinking: false, context: null }, variantRank });
  const opencode = {
    ...codex, id: "opencode", name: "OpenCode", defaultModel: "openai/gpt-5",
    models: [tier("openai/gpt-5", null, 0), tier("openai/gpt-5-fast", "fast", 2), tier("openai/gpt-5-flex", "flex", 2)],
  };
  const { requests } = await renderChat("/projects/p/tasks/s", {
    sessions: [session("s", { harness: "opencode", model: "openai/gpt-5" })],
    harnesses: [codex, opencode],
    routes: { "GET /api/opencode/providers": () => [{ id: "openai", name: "OpenAI", kind: "key" }], "PATCH /api/chat/sessions/*": () => ({}) },
  });
  const click = async (element) => { await act(async () => element.click()); await settle(); };

  // An earlier test's chat stays on the page; this one's composer is the last.
  const effort = () => [...document.querySelectorAll('button[aria-label^="Effort"]')].at(-1);
  await click(effort());
  const speeds = [...document.querySelectorAll('[role="menuitemradio"][aria-label]')].map((item) => item.getAttribute("aria-label"));
  assert.deepEqual(speeds, [m.model_picker_speed_flex(), m.service_tier_standard(), m.model_picker_speed_fast()]);

  await click(document.querySelector(`[role="menuitemradio"][aria-label="${m.model_picker_speed_flex()}"]`));
  assert.match(effort().getAttribute("aria-label"), new RegExp(m.model_picker_speed_flex()));
  assert.ok(requests.some(({ body }) => JSON.stringify(body ?? {}).includes('"model":"openai/gpt-5-flex"')));
});
