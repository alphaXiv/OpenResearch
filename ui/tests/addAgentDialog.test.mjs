import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { AddAgentDialog, OpenCodeSignInDialog } from "../src/components/AddAgentDialog.tsx";
import { m } from "../src/paraglide/messages.js";
import { find, render, serveApp, settle } from "./render.mjs";

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

/** Connects the endpoint the wizard's form offers first, whose model `created` says is new, then cancels at the naming step. */
async function connectThenCancel(created) {
  const removed = [];
  serveApp({
    "GET /api/harnesses": () => ({ harnesses: [opencode] }),
    "GET /api/harnesses/acp": () => [],
    "GET /api/harnesses/acp/presets": () => [],
    "GET /api/harnesses/setup/commands": () => ({}),
    "GET /api/opencode/providers": () => [],
    "GET /api/local-models": () => [{ id: "orx-local-lm", name: "LM Studio", baseUrl: "http://127.0.0.1:1234/v1", hasApiKey: false, models: { qwen: 131072 } }],
    "POST /api/local-models/discover": () => ({ models: ["qwen"] }),
    "POST /api/local-models": () => ({ model: "orx-local-lm/qwen", created }),
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
  await click(find('[role="dialog"] button', m.local_models_custom_endpoint()));
  await click(find('[role="dialog"] button', m.local_models_check()));
  await click(find('[role="dialog"] button', m.onboarding_continue()));
  assert.ok(find('[role="dialog"]', m.add_agent_name_title()));
  await click(find('[role="dialog"] button', m.settings_page_cancel()));
  await click(find('[role="dialog"] button', m.add_agent_discard_close()));
  return removed;
}

test("cancelling after connecting a model the endpoint already had keeps it", async () => {
  assert.deepEqual(await connectThenCancel(false), []);
});

test("cancelling after connecting a new model removes it", async () => {
  assert.deepEqual(await connectThenCancel(true), ["/api/local-models/orx-local-lm?model=qwen"]);
});
