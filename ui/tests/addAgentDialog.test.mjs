import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { AddAgentDialog, OpenCodeSignInDialog } from "../src/components/AddAgentDialog.tsx";
import { m } from "../src/paraglide/messages.js";
import { find, render, serveApp, settle, type } from "./render.mjs";

const click = async (element) => {
  await act(async () => element.click());
  await settle();
};

/** Opens a sign-in whose status checks answer `replies` in turn, the last one from then on; returns how often it confirmed. */
async function signIn(replies) {
  serveApp({
    "GET /api/opencode/sign-in/attempt": () => {
      const reply = replies.length > 1 ? replies.shift() : replies[0];
      return reply === 503 ? Response.json({ error: "OpenResearch is restarting." }, { status: 503 }) : reply;
    },
  });
  const { workspace } = await render(null);
  const outcome = { confirmed: 0 };
  await act(async () => workspace.dialogs.showCustomDialog({
    component: OpenCodeSignInDialog,
    props: { name: "GitHub Copilot", first: { attempt: "attempt", url: "https://github.com/login/device", mode: "auto", instructions: "" }, start: () => new Promise(() => {}) },
    onConfirm: () => { outcome.confirmed++; },
    onCancel: "disable",
  }));
  return outcome;
}

// Timers mocked in a second test of this file never fire, so both cases share one.
test("a sign-in rides out failed status checks, and says why once they keep failing", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  /** Lets `ms` pass in steps, so each reply's next check is scheduled before its time comes. */
  const wait = async (ms) => {
    for (let passed = 0; passed < ms; passed += 500) {
      await act(async () => t.mock.timers.tick(500));
      await settle();
    }
  };

  const recovering = await signIn([503, 503, { status: "complete" }]);
  await wait(1500 + 3000 + 6000);
  assert.equal(recovering.confirmed, 1);

  const failing = await signIn([503]);
  await wait(1500 + 3000 + 6000 + 12000 + 24000);
  assert.equal(failing.confirmed, 0);
  assert.ok(find('[role="dialog"] [role="alert"]', "OpenResearch is restarting."));
  assert.ok(find('[role="dialog"] [role="alert"] button', m.app_retry()));
});

const opencode = {
  id: "opencode", name: "OpenCode", installed: true, installBroken: false, version: "2.1.0", authenticated: true,
  authState: "ready", agentReady: true, models: [], reasoningEfforts: [], capabilities: {},
};

const LM_STUDIO = "http://127.0.0.1:1234/v1";
const unreachable = () => Response.json({ error: "Cannot reach the local server.", code: "unreachable" }, { status: 400 });

/** Opens the wizard on `tile` over model servers `discover` answers for; returns what each probe asked. */
async function customEndpoint({ discover, connected = { model: "orx-local-lm/qwen", created: true }, removed = [], saved = [], machine, tile = m.local_models_custom_endpoint() }) {
  const probes = [];
  serveApp({
    ...(machine && { "GET /api/settings/local": () => machine }),
    "GET /api/harnesses": () => ({ harnesses: [opencode] }),
    "GET /api/harnesses/acp": () => [],
    "GET /api/harnesses/acp/presets": () => [],
    "GET /api/harnesses/setup/commands": () => ({}),
    "GET /api/opencode/providers": () => [],
    "GET /api/local-models": () => [{ id: "orx-local-lm", name: "LM Studio", baseUrl: LM_STUDIO, hasApiKey: false, models: { qwen: 131072 } }],
    "POST /api/local-models/discover": (init) => {
      const probe = JSON.parse(init.body);
      probes.push(probe);
      return discover(probe);
    },
    "POST /api/local-models": (init) => {
      saved.push(JSON.parse(init.body));
      return connected;
    },
    "DELETE /api/local-models/orx-local-lm": (_init, path) => { removed.push(path); return { ok: true }; },
  });
  const { workspace } = await render(null);
  await act(async () => workspace.dialogs.showCustomDialog({
    component: AddAgentDialog,
    props: { remote: false, onCustomAcp() {}, onEditAcp() {} },
    onConfirm() {},
    onCancel: "disable",
  }));
  await settle();
  await click(find('[role="dialog"] button', tile));
  return probes;
}
const field = (label) => find('[role="dialog"] label', label).querySelector("input");
const findModels = () => click(find('[role="dialog"] button', m.local_models_check()));

/** Connects the endpoint at LM Studio's address, whose model `created` says is new, then cancels at the naming step. */
async function connectThenCancel(created) {
  const removed = [];
  await customEndpoint({ discover: () => ({ models: ["qwen"] }), connected: { model: "orx-local-lm/qwen", created }, removed });
  await type(field(m.local_models_address()), LM_STUDIO);
  await findModels();
  await click(find('[role="dialog"] button', m.onboarding_continue()));
  assert.ok(find('[role="dialog"]', m.add_agent_name_title()));
  await click(find('[role="dialog"] button', m.settings_page_cancel()));
  await click(find('[role="dialog"] button', m.add_agent_discard_close()));
  return removed;
}

for (const [orx, reply] of [
  ["this orx", { error: "The server requires an API key. Enter its key and try again.", code: "needsKey" }],
  ["an orx from before the needsKey code", { error: "The server requires an API key. Enter its key and try again." }],
]) test(`a server that asks for a key gets a page for it, and Back returns to the address: ${orx}`, async () => {
  const needsKey = () => Response.json(reply, { status: 400 });
  const probes = await customEndpoint({ discover: ({ baseUrl, apiKey }) => baseUrl !== "https://models.example.org/v1" ? unreachable() : apiKey ? { models: ["qwen"] } : needsKey() });
  await type(field(m.local_models_address()), "https://models.example.org/v1");
  await findModels();
  assert.ok(find('[role="dialog"]', "asks for an API key"), "the key gets its own page");
  assert.equal(document.querySelector('[role="dialog"] [role="alert"]'), null, "asking for a key is no error");

  await type(field(m.add_agent_api_key()), "sk-local");
  await findModels();
  assert.deepEqual(probes.at(-1), { baseUrl: "https://models.example.org/v1", apiKey: "sk-local" });
  assert.ok(find('[role="dialog"] button', m.onboarding_continue()), "the server's models are listed");

  await click(find('[role="dialog"] button', m.onboarding_back()));
  assert.equal(field(m.local_models_address()).value, "https://models.example.org/v1");
});

test("model servers running on this computer are offered beside the address", async () => {
  await customEndpoint({ discover: ({ baseUrl }) => baseUrl === LM_STUDIO ? { models: ["qwen"] } : unreachable() });
  assert.equal(find('[role="dialog"] button', "Ollama"), null, "a server that isn't running isn't offered");
  await click(find('[role="dialog"] button', "LM Studio"));
  assert.equal(field(m.local_models_address()).value, LM_STUDIO);
  assert.ok(!find('[role="dialog"] button', m.onboarding_continue()).disabled, "its models are listed without asking again");
});

test("cancelling after connecting a model the endpoint already had keeps it", async () => {
  assert.deepEqual(await connectThenCancel(false), []);
});

test("cancelling after connecting a new model removes it", async () => {
  assert.deepEqual(await connectThenCancel(true), ["/api/local-models/orx-local-lm?model=qwen"]);
});

test("the local model guide says a server is running without models, and that it couldn't check rather than that none runs", async () => {
  const empty = () => Response.json({ error: "The server is running but has no models.", code: "noModels" }, { status: 400 });
  await customEndpoint({ tile: m.local_models_guide_name(), discover: ({ baseUrl }) => baseUrl === LM_STUDIO ? empty() : unreachable() });
  assert.ok(find('[role="dialog"]', m.local_models_guide_no_models({ name: "LM Studio" })));

  await customEndpoint({ tile: m.local_models_guide_name(), discover: () => Response.json({ error: "OpenResearch is restarting." }, { status: 503 }) });
  assert.ok(find('[role="dialog"] [role="alert"]', "OpenResearch is restarting."));
  assert.ok(!find('[role="dialog"]', m.local_models_guide_none()), "an unknown check doesn't say nothing runs");
});

for (const [ownedBy, name] of [["omlx", "oMLX"], ["rapid-mlx", "Rapid-MLX"]]) {
  test(`a server at an address two apps share is named by who owns its models: ${name}`, async () => {
    const saved = [];
    await customEndpoint({ saved, discover: ({ baseUrl }) => baseUrl === "http://127.0.0.1:8000/v1" ? { models: ["qwen"], ownedBy } : unreachable() });
    for (const other of ["oMLX", "Rapid-MLX"].filter((label) => label !== name)) assert.equal(find('[role="dialog"] button', other), null, `${other} isn't what's running`);
    await click(find('[role="dialog"] button', name));
    await click(find('[role="dialog"] button', m.onboarding_continue()));
    assert.equal(saved[0].name, name);
  });
}

test("a server at that address from another app is offered as a custom endpoint", async () => {
  const saved = [];
  await customEndpoint({ saved, discover: ({ baseUrl }) => baseUrl === "http://127.0.0.1:8000/v1" ? { models: ["qwen"], ownedBy: "vllm" } : unreachable() });
  await click(find('[role="dialog"] button', m.local_models_custom_endpoint()));
  await click(find('[role="dialog"] button', m.onboarding_continue()));
  assert.equal(saved[0].baseUrl, "http://127.0.0.1:8000/v1");
  assert.equal(saved[0].name, "Custom Endpoint");
});

test("on Apple silicon the local model guide also walks through oMLX and Rapid-MLX, without recommending them", async () => {
  await customEndpoint({ tile: m.local_models_guide_name(), machine: { os: "macos", arch: "aarch64", memBytes: 32 * 2 ** 30, gpus: [] }, discover: unreachable });
  const picker = find('[role="dialog"] button[aria-haspopup="menu"]', "LM Studio");
  assert.ok(picker, "LM Studio is still the default");
  await click(picker);
  await click(find("button", "Rapid-MLX"));
  assert.ok(find('[role="dialog"] code', "brew install rapid-mlx"));
});

test("a provider from OpenCode's list that takes a key links to where to get one", async () => {
  serveApp({
    "GET /api/harnesses": () => ({ harnesses: [opencode] }),
    "GET /api/harnesses/acp": () => [],
    "GET /api/harnesses/acp/presets": () => [],
    "GET /api/harnesses/setup/commands": () => ({}),
    "GET /api/opencode/providers": () => [],
    "GET /api/local-models": () => [],
    "GET /api/opencode/integrations": () => [{ id: "groq", name: "Groq", methods: [{ type: "key" }], connected: false }],
  });
  const { workspace } = await render(null);
  await act(async () => workspace.dialogs.showCustomDialog({
    component: AddAgentDialog,
    props: { remote: false, onCustomAcp() {}, onEditAcp() {} },
    onConfirm() {},
    onCancel: "disable",
  }));
  await settle();
  await click(find('[role="dialog"] button', m.add_agent_more()));
  await click(find('[role="dialog"] button', "Groq"));
  assert.ok(find('[role="dialog"] label', m.provider_key_label({ name: "Groq" })));
  assert.equal(find('[role="dialog"] a', m.provider_key_get()).getAttribute("href"), "https://console.groq.com/keys");
});
