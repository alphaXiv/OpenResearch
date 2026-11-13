import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { find, renderApp, serveApp, settle, type } from "./render.mjs";
import { model } from "./models.mjs";

// The onboarding installs OpenCode for its free models over a socket this test never answers.
globalThis.WebSocket = class extends EventTarget {
  static OPEN = 1;
  readyState = 0;
  send() {}
  close() {}
};

const harness = (id, name, extra = {}) => ({
  id, name, installed: true, installBroken: false, authenticated: true, authState: "ready", agentReady: true, models: [model("model")], defaultModel: "model", ...extra,
});
const codex = harness("codex", "Codex");
const opencodeMissing = harness("opencode", "OpenCode", { installed: false, authenticated: false, authState: "needsLogin", agentReady: false, models: [], defaultModel: undefined });
const custom = (extra) => harness("acp:custom", "My agent", extra);
const checking = { catalogPending: true, models: [], defaultModel: undefined };

const click = async (element) => {
  await act(async () => element.click());
  await settle();
};
const proceed = () => find("button", m.onboarding_continue());

/** The local walkthrough over `others`, after connecting a custom ACP agent whose check is still running. */
async function connectCustom(others) {
  const state = { harnesses: others, refreshes: 0, completions: [] };
  serveApp({
    "GET /api/settings/ui-state": () => ({ onboardingCompleted: false, preferredAgent: null }),
    "GET /api/harnesses": (init, path) => {
      if (path.includes("retry=1")) state.refreshes++;
      return { harnesses: state.harnesses };
    },
    "GET /api/project-path/status": () => ({ gitVersion: "2.50.0", resolvedPath: null, exists: null, directory: null, empty: null, initialized: null }),
    "GET /api/harnesses/acp": () => [],
    "GET /api/harnesses/acp/presets": () => [],
    "POST /api/harnesses/acp": (init) => ({ ...JSON.parse(init.body), id: "acp:custom" }),
    "POST /api/onboarding/complete": (init) => {
      state.completions.push(JSON.parse(init.body));
      return { project: { id: "demo" }, selection: JSON.parse(init.body) };
    },
  });
  await renderApp("/projects");
  await click(proceed());

  state.harnesses = [...others, custom(checking)];
  await click(find("button", m.add_agent_other_providers()));
  await click(find('[role="dialog"] button', m.add_agent_more()));
  await click(find('[role="dialog"] button', m.harness_custom_acp()));
  await type(find('[role="dialog"] label', m.acp_name()).querySelector("input"), "My agent");
  await type(find('[role="dialog"] label', m.acp_executable()).querySelector("input"), "my-agent acp");
  await click(find('[role="dialog"] button', m.common_save()));
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 0);
  assert.equal(state.refreshes, 1, "saving re-checks the agents");
  return state;
}

/** Re-checks to find `harnesses`, then finishes the walkthrough; returns the selection it completed with. */
async function finish(state, harnesses) {
  state.harnesses = harnesses;
  await click(find("button", m.onboarding_re_check()));
  await click(proceed());
  await click(find("button", m.onboarding_skip()));
  return state.completions[0];
}

test("an ACP agent connected during onboarding starts the first chat once its check finishes", async () => {
  const state = await connectCustom([codex]);
  assert.equal((await finish(state, [codex, custom()])).harness, "acp:custom");
});

test("an ACP agent still being checked leaves the first chat on an agent that's ready", async () => {
  const state = await connectCustom([codex]);
  assert.equal((await finish(state, [codex, custom(checking)])).harness, "codex");
});

test("an agent being added holds Continue rather than falling back to the free models", async () => {
  const state = await connectCustom([opencodeMissing]);
  assert.ok(proceed().disabled);
  assert.equal((await finish(state, [opencodeMissing, custom()])).harness, "acp:custom");
});
