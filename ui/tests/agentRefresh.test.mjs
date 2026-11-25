import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { AgentList } from "../src/components/SettingsPage.tsx";
import { m } from "../src/paraglide/messages.js";
import { model } from "./models.mjs";
import { find, render, serveApp, settle } from "./render.mjs";

const codex = {
  id: "codex", name: "Codex", installed: true, authenticated: true, authState: "ready", agentReady: true,
  version: "0.164.0", models: [model("gpt-6.1-sol", "GPT-6.1 Sol")], options: {},
};

const click = async (element) => {
  await act(async () => element.click());
  await settle();
};

async function agents(refresh, { catalog = () => ({ harnesses: [codex] }), remote = false } = {}) {
  const requests = [];
  serveApp({
    "GET /api/harnesses": (_init, path) => {
      requests.push(path);
      return path.includes("refresh=1") ? refresh() : catalog();
    },
    "GET /api/harnesses/setup/commands": () => ({}),
    "GET /api/harnesses/acp": () => [],
    "GET /api/harnesses/acp/presets": () => [],
    "GET /api/opencode/providers": () => [],
    "GET /api/local-models": () => [],
  });
  const { workspace } = await render(createElement(AgentList, { remote }));
  await settle();
  return { workspace, requests };
}

test("Refresh announces a check and its completion even when the catalog is unchanged", async () => {
  let answer;
  const { requests } = await agents(() => new Promise((resolve) => { answer = resolve; }));
  const refresh = find("button", m.settings_page_refresh());
  assert.equal(find('[role="status"]')?.textContent.trim() ?? "", "");

  await click(refresh);
  assert.ok(refresh.disabled);
  assert.ok(find('[role="status"]', m.common_checking()));
  assert.ok(requests.at(-1).includes("refresh=1"));
  assert.ok(requests.at(-1).includes("retry=1"));

  await act(async () => answer({ harnesses: [codex] }));
  await settle();
  assert.equal(refresh.disabled, false);
  const complete = find('[role="status"]');
  assert.ok(complete?.textContent.trim(), "an unchanged catalog still has a visible result");
  assert.ok(!complete.textContent.includes(m.common_checking()));
  assert.ok(!find('[role="alert"]'));
});

test("a double-click starts one agent check", async () => {
  const answers = [];
  const { requests } = await agents(() => new Promise((resolve) => answers.push(resolve)));
  const refresh = find("button", m.settings_page_refresh());
  await act(async () => { refresh.click(); refresh.click(); });
  await settle();
  const checks = requests.filter((path) => path.includes("refresh=1")).length;
  await act(async () => answers.forEach((answer) => answer({ harnesses: [codex] })));
  await settle();
  assert.equal(checks, 1);
});

test("a failed Refresh explains the failure beside the last catalog and can be retried", async () => {
  const message = "OpenResearch is restarting. Try again.";
  let answer;
  let calls = 0;
  const { workspace } = await agents(() => ++calls === 2
    ? new Promise((resolve) => { answer = resolve; })
    : { harnesses: [codex] });

  await click(find("button", m.settings_page_refresh()));
  assert.ok(find('[role="status"]', m.harness_refresh_complete()));

  await click(find("button", m.settings_page_refresh()));
  assert.ok(find('[role="status"]', m.common_checking()));
  assert.ok(!find('[role="status"]', m.harness_refresh_complete()));

  await act(async () => answer(Response.json({ error: message }, { status: 503 })));
  await settle();
  assert.ok(find('[role="alert"]', message));
  assert.deepEqual(workspace.queryClient.getQueryData(workspace.queries.getHarnessesQuery().queryKey), [codex]);
  assert.equal(find('[role="status"]')?.textContent.trim() ?? "", "");

  await click(find("button", m.settings_page_refresh()));
  assert.ok(!find('[role="alert"]'));
  assert.ok(find('[role="status"]', m.harness_refresh_complete()));
});

test("Retry announces completion after recovering from a failed catalog load", async () => {
  const message = "Could not load the agent catalog.";
  await agents(() => ({ harnesses: [codex] }), { catalog: () => Response.json({ error: message }, { status: 503 }) });
  assert.ok(find('[role="alert"]', message));
  assert.ok(!find("button", m.settings_page_refresh()));

  const retry = find("button", m.app_retry());
  await click(retry);
  assert.ok(!find('[role="alert"]'));
  assert.equal(find("button", m.settings_page_refresh()), retry);
  assert.ok(find('[role="status"]', m.harness_refresh_complete()));
});

test("a failed Retry shows only the current error after a failed catalog load", async () => {
  const catalogError = "Could not load the agent catalog.";
  const retryError = "The agent check failed.";
  await agents(() => Response.json({ error: retryError }, { status: 500 }), {
    catalog: () => Response.json({ error: catalogError }, { status: 503 }),
  });
  assert.ok(find('[role="alert"]', catalogError));

  await click(find("button", m.app_retry()));
  const alerts = [...document.querySelectorAll('[role="alert"]')];
  assert.deepEqual(alerts.map((alert) => alert.textContent), [retryError]);
});

test("Retry keeps its error if the catalog recovers while it is pending", async () => {
  let answer;
  let loaded = false;
  const { workspace } = await agents(() => new Promise((resolve) => { answer = resolve; }), { catalog: () => loaded
    ? { harnesses: [codex] }
    : Response.json({ error: "Could not load the agent catalog." }, { status: 503 }) });
  await click(find("button", m.app_retry()));

  loaded = true;
  await act(async () => { await workspace.queries.readHarnesses(); });
  await settle();
  assert.ok(find('[role="status"]', m.common_checking()));

  await act(async () => answer(Response.json({ error: "The agent check failed." }, { status: 500 })));
  await settle();
  assert.ok(find('[role="alert"]', "The agent check failed."));
});

test("Add agent can Refresh and show its own error while Settings is checking", async () => {
  const missing = { ...codex, installed: false, agentReady: false, models: [] };
  const answers = [];
  await agents(() => new Promise((resolve) => answers.push(resolve)), { remote: true, catalog: () => ({ harnesses: [missing] }) });
  const settingsRefresh = find("button", m.settings_page_refresh());
  await click(settingsRefresh);
  await click(find("button", m.harness_add()));
  await click(find('[role="dialog"] button', "OpenAI Codex"));
  const setupRefresh = find('[role="dialog"] button', m.settings_page_refresh());
  await click(setupRefresh);
  assert.ok(setupRefresh.disabled);

  await act(async () => answers[1](Response.json({ error: "The remote agent check failed." }, { status: 503 })));
  await settle();
  assert.ok(settingsRefresh.disabled);
  assert.ok(find('[role="dialog"] [role="alert"]', "The remote agent check failed."));

  await act(async () => answers[0]({ harnesses: [missing] }));
  await settle();
});

test("Re-check blocks Finish and keeps its result when models appear in the background", async () => {
  const cursor = { ...codex, id: "cursor", name: "Cursor", models: [] };
  const found = { harnesses: [{ ...cursor, models: codex.models }] };
  let answer;
  let catalog = { harnesses: [cursor] };
  const { requests, workspace } = await agents(() => answer ? found : new Promise((resolve) => { answer = resolve; }), { remote: true, catalog: () => catalog });
  await click(find("button", m.harness_add()));
  await click(find('[role="dialog"] button', "Cursor"));
  await click(find('[role="dialog"] button', m.onboarding_continue()));
  const finish = find('[role="dialog"] button', m.add_agent_finish({ name: cursor.name }));
  await click(find('[role="dialog"] button', m.onboarding_re_check()));
  assert.ok(requests.at(-1).includes("harness=cursor"));
  assert.ok(finish.disabled);

  catalog = found;
  await act(async () => { await workspace.queries.readHarnesses(); });
  await settle();
  assert.ok(finish.disabled);
  assert.ok(find('[role="dialog"] [role="status"]', m.common_checking()));
  assert.ok(find('[role="dialog"]', codex.models[0].parsed.label));

  await act(async () => answer(Response.json({ error: "Could not check Cursor models." }, { status: 503 })));
  await settle();
  assert.ok(find('[role="dialog"] [role="alert"]', "Could not check Cursor models."));
  assert.equal(finish.disabled, false);

  await click(find('[role="dialog"] button', m.onboarding_re_check()));
  assert.ok(!find('[role="dialog"] [role="alert"]'));
  assert.ok(!find('[role="dialog"] button', m.onboarding_re_check()));
  assert.ok(find('[role="dialog"] [role="status"]', m.harness_refresh_complete()));
  assert.ok(find('[role="dialog"]', codex.models[0].parsed.label));
});

test("switching one provider keeps every provider's model count while the agents reload", async () => {
  const opencode = {
    id: "opencode", name: "OpenCode", installed: true, authenticated: true, authState: "ready", agentReady: true,
    version: "1.14.0", options: {}, models: [
      model("openai/gpt-6.1-sol", "GPT-6.1 Sol"), model("openai/gpt-6.1-luna", "GPT-6.1 Luna"),
      model("openrouter/kimi-k3", "Kimi K3"), model("openrouter/glm-5.3", "GLM 5.3"), model("openrouter/qwen-4", "Qwen 4"),
      model("wandb/qwen-4", "Qwen 4"),
    ],
  };
  let reloads = 0;
  let answer;
  serveApp({
    "GET /api/harnesses": () => reloads++ === 0 ? { harnesses: [opencode] } : new Promise((resolve) => { answer = resolve; }),
    "GET /api/harnesses/setup/commands": () => ({}),
    "GET /api/harnesses/acp": () => [],
    "GET /api/harnesses/acp/presets": () => [],
    "GET /api/opencode/providers": () => ["OpenAI", "OpenRouter", "Wandb"].map((name) => ({ id: name.toLowerCase(), kind: "key", name })),
    "GET /api/local-models": () => [],
  });
  await render(createElement(AgentList, { remote: false }));
  await settle();
  /** The row whose switch uses `name`: the switch's nearest ancestor that names it. */
  const row = (name) => {
    let element = document.querySelector(`button[aria-label="${m.settings_agent_use({ name })}"]`);
    while (element && !element.textContent.includes(name)) element = element.parentElement;
    return element?.textContent ?? "";
  };
  const counts = () => {
    assert.ok(row("OpenAI").includes(m.settings_models_available({ count: 2 })), row("OpenAI"));
    assert.ok(row("OpenRouter").includes(m.settings_models_available({ count: 3 })), row("OpenRouter"));
    assert.ok(!document.body.textContent.includes(m.common_checking()), "no row reads as checking");
  };
  counts();
  assert.ok(row("Wandb").includes(m.settings_models_available_one({ count: 1 })), row("Wandb"));

  await click(document.querySelector(`button[aria-label="${m.settings_agent_use({ name: "Wandb" })}"]`));
  assert.equal(reloads, 2, "the switch reloads the agents");
  counts();
  assert.ok(row("Wandb").includes(m.settings_agent_off()), row("Wandb"));

  await act(async () => answer({ harnesses: [opencode] }));
  await settle();
  counts();
});
