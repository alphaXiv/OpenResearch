import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { find, renderApp, serveApp, settle, type } from "./render.mjs";

const preset = { id: "acp:preset", name: "Kimi Code", executable: "kimi", arguments: ["acp"], setupUrl: "https://example.com", needsConnectionAdapter: false };
const harness = (id, name, extra = {}) => ({
  id, name, installed: true, installBroken: false, authenticated: true, authState: "ready", agentReady: true, models: [], ...extra,
});
const codex = harness("codex", "Codex");
const saved = (extra) => harness("acp:saved", "Saved ACP", extra);

const click = async (element) => {
  await act(async () => element.click());
  await settle();
};

test("onboarding selects the registered profile after a preset connects and refreshes", async () => {
  let harnesses = [codex, saved({ catalogPending: true })];
  let tested;
  const refreshes = [];
  const saves = [];
  serveApp({
    "GET /api/settings/ui-state": () => ({ onboardingCompleted: false, preferredAgent: null }),
    "GET /api/harnesses": (init, path) => {
      if (path.includes("retry=1")) refreshes.push(path);
      return { harnesses };
    },
    "GET /api/project-path/status": () => ({ gitVersion: "2.50.0", resolvedPath: null, exists: null, directory: null, empty: null, initialized: null }),
    "GET /api/harnesses/acp/presets": () => [preset],
    "POST /api/harnesses/acp/test": () => ({ status: "ok", harnessId: tested }),
    "POST /api/harnesses/acp": (init) => {
      saves.push(JSON.parse(init.body));
      return { ...JSON.parse(init.body), id: "acp:new" };
    },
  });
  await renderApp("/projects");
  await click(find("button", m.onboarding_continue()));

  const selected = () => document.querySelector('button[aria-pressed="true"]')?.textContent ?? null;
  const dialogs = () => document.querySelectorAll('[role="dialog"]').length;
  const recheck = async (snapshot) => {
    harnesses = snapshot;
    await click(find("button", m.onboarding_re_check()));
  };
  /** Connects the preset through the ACP dialog, optionally renamed; the server's test answers `harnessId`. */
  const connect = async (harnessId, name = preset.name) => {
    tested = harnessId;
    const before = refreshes.length;
    await click(find("button", m.acp_add()));
    if (name !== preset.name) await type(find('[role="dialog"] label', m.acp_name()).querySelector("input"), name);
    await click(find('[role="dialog"] button', m.settings_connect()));
    assert.equal(dialogs(), 0);
    assert.equal(refreshes.length, before + 1);
  };

  assert.match(selected(), /Codex/);
  await connect("acp:saved");
  assert.match(selected(), /Codex/, "a profile still loading its catalog is not selected yet");
  await recheck([codex, saved({ catalogPending: false })]);
  assert.match(selected(), /Saved ACP/);

  await click(find("button", "Codex"));
  await recheck([codex, saved()]);
  assert.match(selected(), /Codex/, "a later refresh does not override the user's choice");

  await connect("acp:saved");
  assert.match(selected(), /Saved ACP/, "an already-ready profile is selected at once");

  await click(find("button", "Codex"));
  await connect("acp:preset");
  await recheck([codex, saved(), harness("acp:preset", preset.name)]);
  assert.match(selected(), /Kimi Code/, "the preset's own profile is selected once it appears");

  await connect("acp:saved", "My configuration");
  assert.deepEqual(saves, [{ name: "My configuration", executable: preset.executable, arguments: preset.arguments }]);
  await recheck([codex, saved(), harness("acp:preset", preset.name), harness("acp:new", "My configuration")]);
  assert.match(selected(), /My configuration/, "an edited preset registers and selects a new profile");
});
