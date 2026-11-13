import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { SkillsTab } from "../src/components/SkillsTab.tsx";
import { TARGET_LABELS } from "../src/computeTargets.ts";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, codex, composer, deferred, emit, renderChat, session, userMessage } from "./chat.mjs";
import { clients, find, press, render, renderApp, serve, serveApp, settle, type } from "./render.mjs";
import { model } from "./models.mjs";

const click = async (element) => {
  await act(async () => element.click());
  await settle();
};
const refresh = async () => {
  await act(async () => { for (const client of clients) void client.invalidateQueries(); });
  await settle();
};
/** The text field labelled `label`. */
const field = (label) => [...document.querySelectorAll("label")].find((element) => element.textContent.includes(label))?.querySelector("input");

const targets = {
  k8s: {
    fields: { namespace: () => m.settings_page_namespace() },
    reply: (value) => ({ configured: true, contexts: [], currentContext: null, context: null, namespace: value, preflight: { kubectlFound: true, reachable: true, canCreateJobs: true, canPatchJobs: true } }),
  },
  slurm: {
    fields: { partition: () => m.settings_page_partition(), account: () => m.settings_page_account(), timeLimit: () => m.settings_page_time_limit() },
    reply: (value) => ({ host: null, partition: value, account: value, timeLimit: value, cpusPerTask: null, mem: null, hosts: [] }),
  },
  ray: {
    fields: { address: () => m.settings_page_jobs_dashboard_url() },
    reply: (value) => ({ address: value, resolvedAddress: value, source: "settings" }),
  },
};

for (const [id, { fields, reply }] of Object.entries(targets)) {
  test(`${id} settings refresh clean fields while preserving edits`, async () => {
    let value = "original";
    serveApp({
      "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project" }] }),
      "GET /api/settings/compute": () => ({ defaultBackend: null, defaultFlavor: null, targets: [{ id, configured: true, enabled: true, summary: "" }] }),
      [`GET /api/settings/${id}`]: () => reply(value),
    });
    await renderApp("/projects/p/settings/compute");
    await click(document.querySelector(`button[aria-label="${TARGET_LABELS[id]()}"]`));
    const values = () => Object.values(fields).map((label) => field(label()).value);
    assert.deepEqual(values(), Object.keys(fields).map(() => "original"));

    value = "refreshed";
    await refresh();
    assert.deepEqual(values(), Object.keys(fields).map(() => "refreshed"));

    await type(field(Object.values(fields)[0]()), "my edit");
    const edited = values();
    value = "external";
    await refresh();
    assert.deepEqual(values(), edited);
  });
}

test("agent settings list what works, OpenResearch's accounts and installs, and saved custom agents", async () => {
  const harness = (id, fields = {}) => ({ ...codex, id, name: id, installed: true, agentReady: true, ...fields });
  const work = harness("claude-code", { profile: { id: "claude-0123456789", name: "Work" } });
  let harnesses = [
    harness("opencode", { agentReady: false }), work, harness("codex", { installed: false, agentReady: false }), harness("claude-code", { agentReady: false }),
    harness("acp:custom", { installed: false, agentReady: false }), harness("acp:preset", { installed: false, agentReady: false }),
    // A program that only shares an agent's name: found, but never working.
    harness("acp:fx", { agentReady: false, authState: "unknown" }),
  ];
  serveApp({
    "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project" }] }),
    "GET /api/settings/ui-state": () => ({ onboardingCompleted: true, hiddenAgents: [], hiddenModels: [], modelSectionOrder: [] }),
    "GET /api/harnesses": () => ({ harnesses }),
    "GET /api/harnesses/acp": () => [{ id: "acp:custom", name: "acp:custom", executable: "custom", arguments: [] }],
    "GET /api/harnesses/acp/presets": () => [],
    "GET /api/harnesses/setup/commands": () => ({}),
    "GET /api/opencode/providers": () => [],
    "GET /api/local-models": () => [],
  });
  await renderApp("/projects/p/settings/harnesses");
  const rows = () => [...document.querySelectorAll('button[aria-controls*="agent-details-"]')].map((row) => row.getAttribute("aria-controls").split("agent-details-")[1]);
  // An agent found on this computer shows once it works; what was added here always shows.
  assert.deepEqual(rows(), ["claude-0123456789", "acp:custom"]);

  harnesses = harnesses.map((h) => h.id === "claude-code" && !h.profile ? { ...h, agentReady: true } : h);
  await refresh();
  // What OpenResearch keeps lists first, then what it found.
  assert.deepEqual(rows(), ["claude-0123456789", "acp:custom", "claude-code"]);

  // An agent OpenResearch installed for itself is kept, signed in or not; its copy of Codex stands in for the user's.
  harnesses = [...harnesses.filter((h) => h.id !== "codex"), harness("antigravity", { managed: true, agentReady: false }), harness("codex", { managed: true, agentReady: false })];
  await refresh();
  assert.ok(rows().includes("antigravity"));
  assert.ok(!rows().includes("codex"));
});

/** Settings → Agents over `harnesses`, with the free models switched off; records each preference written. */
async function agentSwitches(harnesses) {
  let uiState = { onboardingCompleted: true, hiddenAgents: [], hiddenModels: [], modelSectionOrder: [], opencodeFreeModels: false };
  const state = { writes: [], harnessReads: 0 };
  serveApp({
    "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project" }] }),
    "GET /api/settings/ui-state": () => uiState,
    "POST /api/settings/ui-state": (init) => {
      const { workspace, ...changes } = JSON.parse(init.body);
      if (!workspace) state.writes.push(changes);
      uiState = { ...uiState, ...changes };
      return uiState;
    },
    "GET /api/harnesses": () => {
      state.harnessReads++;
      return { harnesses };
    },
    "GET /api/harnesses/acp": () => [],
    "GET /api/harnesses/acp/presets": () => [],
    "GET /api/harnesses/setup/commands": () => ({}),
    "GET /api/opencode/providers": () => [],
    "GET /api/local-models": () => [],
  });
  await renderApp("/projects/p/settings/harnesses");
  return state;
}
const freeModels = () => document.querySelector(`button[aria-label="${m.settings_free_models()}"]`);
const agentSwitch = (name) => document.querySelector(`button[aria-label="${m.settings_agent_use({ name })}"]`);
const opencodeWith = (...ids) => ({ ...codex, id: "opencode", name: "OpenCode", models: ids.map((id) => model(id)), defaultModel: ids[0] });

test("the free models come back on with the last other agent turned off, and stay on until another is", async () => {
  const state = await agentSwitches([codex, opencodeWith("opencode/big-pickle")]);
  assert.ok(freeModels().closest(".settings-card").textContent.includes(m.settings_free_models_description()), "with no model it can name, the row says what the free models are");
  assert.equal(freeModels().getAttribute("aria-disabled"), null, "with Codex on, the free models can be switched off");

  const reads = state.harnessReads;
  await click(agentSwitch("Codex"));
  assert.deepEqual(state.writes, [{ hiddenAgents: ["codex"], opencodeFreeModels: true }]);
  assert.ok(state.harnessReads > reads, "the agents reload to list the free models");

  assert.equal(freeModels().getAttribute("aria-disabled"), "true");
  assert.equal(freeModels().getAttribute("aria-description"), m.settings_free_models_locked());
  await click(freeModels());
  assert.equal(state.writes.length, 1, "the free models stay on while nothing else runs a chat");

  await click(agentSwitch("Codex"));
  assert.deepEqual(state.writes.at(-1), { hiddenAgents: [] });
  assert.equal(freeModels().getAttribute("aria-disabled"), null);
});

test("a provider signed in to in OpenCode itself is listed, and switching it off brings the free models back", async () => {
  // Signed in through the environment, so OpenCode lists its models but names no account for it.
  const state = await agentSwitches([opencodeWith("opencode/big-pickle", "deepseek/deepseek-v4")]);
  assert.ok(agentSwitch("OpenCode"), "OpenCode's own sign-ins have a row");
  await click(agentSwitch("OpenCode"));
  assert.deepEqual(state.writes, [{ hiddenAgents: ["opencode"], opencodeFreeModels: true }]);
});

test("the free models name two models a user may know", async () => {
  const named = [
    model("opencode/big-pickle", "Big Pickle"), model("opencode/glm-5.3-flash-free", "GLM-5.3 Flash"),
    model("opencode/inkling-2-small-free", "Inkling 2 Small"), model("opencode/deepseek-v4.1-flash-free", "DeepSeek V4.1 Flash"),
  ];
  await agentSwitches([codex, { ...opencodeWith("opencode/big-pickle"), models: named }]);
  const row = () => freeModels().closest(".settings-card").textContent;
  for (const name of ["DeepSeek V4.1 Flash", "Inkling 2 Small"]) assert.ok(row().includes(name), name);
  assert.ok(!row().includes("GLM-5.3 Flash"), "the featured labs' models come first");
  assert.ok(!row().includes("Big Pickle"), "a model of no known lab isn't named");
});

test("queued chat writes stop when their captured workspace retires", async () => {
  const reply = deferred();
  const chat = await renderChat("/projects/p/tasks/s", { remote: true, routes: { "POST /api/chat/sessions/s/message": () => reply.promise } });
  await composer().type("first");
  await composer().send();
  await composer().type("/plan");
  await press("Escape", composer().field());
  await composer().send();
  await chat.reconnect();
  await reply.resolve({ ok: true });
  assert.deepEqual(chat.requests.filter(({ request }) => request === "PATCH /api/chat/sessions/s"), []);
});

for (const retired of [true, false]) {
  test(`${retired ? "old prompt responses do not reconcile against the replacement workspace" : "a prompt response reconciles its chat"}`, async () => {
    const answer = deferred();
    const prompt = { kind: "question", question: "Which one?", options: [{ label: "First" }] };
    const chat = await renderChat("/projects/p/tasks/s", {
      remote: true,
      transcripts: { s: [userMessage("u1", "choose"), assistantMessage("a1", [{ id: "question", type: "prompt", prompt }], "u1")] },
      routes: { "POST /api/chat/sessions/s/respond": () => answer.promise },
    });
    await click(find('[role="group"] button', "First"));
    await act(async () => document.querySelector('[role="group"] form').requestSubmit());
    if (retired) await chat.reconnect();
    const before = chat.requests.length;
    await answer.resolve({ ok: true });
    const reads = chat.requests.slice(before).filter(({ request }) => request === "GET /api/chat/sessions/s/messages" || request === "GET /api/chat/sessions");
    assert.equal(reads.length, retired ? 0 : 2);
  });
}

for (const [card, path, field, error] of [
  ["skills", "/api/user-skills", "skills", () => m.skills_tab_could_not_load_skills()],
  ["LaTeX templates", "/api/latex-templates", "templates", () => m.skills_tab_could_not_load_templates()],
]) {
  test(`the ${card} card keeps cached rows when a refresh fails`, async () => {
    let fail = false, calls = 0;
    serve((url) => {
      if (url !== path) return;
      calls++;
      if (fail) return Response.json({ error: "offline" }, { status: 503 });
      return { [field]: [{ name: "cached", description: "", entry: "main.tex", supportFiles: [], bytes: 1, updatedAt: 1 }] };
    });
    const { container } = await render(createElement(SkillsTab));
    await settle();
    assert.ok(container.textContent.includes("cached"));
    fail = true;
    if (card === "skills") await click(find("button", m.settings_page_refresh()));
    else await act(async () => { for (const client of clients) void client.refetchQueries({ type: "active" }); });
    await settle();
    assert.ok(calls >= 2);
    assert.ok(container.textContent.includes("cached"));
    assert.ok(container.textContent.includes(`${error()} offline`));
  });
}

test("first send waits for cold history so an accepted reply replaces its optimistic branch", async () => {
  const history = deferred();
  let snapshot = null;
  const chat = await renderChat("/projects/p/tasks/s", { routes: {
    "GET /api/chat/sessions/s/messages": () => snapshot ?? history.promise,
    "POST /api/chat/sessions/s/message": () => ({ ok: true }),
  } });
  await composer().type("new turn");
  await composer().send();
  assert.deepEqual(chat.sent(), []);
  snapshot = { messages: [userMessage("history", "earlier")] };
  await history.resolve(snapshot);
  assert.deepEqual(chat.sent().map(({ text }) => text), ["new turn"]);
  assert.ok(document.querySelector('[data-message-id^="local-"]'));

  snapshot = { messages: [userMessage("history", "earlier"), userMessage("accepted", "new turn", "history"), assistantMessage("reply", "Done.", "accepted")] };
  await emit("resync.required", {});
  assert.deepEqual([...document.querySelectorAll("[data-message-id]")].map((row) => row.dataset.messageId), ["history", "accepted", "reply"]);
});

test("history preparation clears the composer up front but never sends early", async () => {
  const chat = await renderChat("/projects/p/tasks/s", { routes: { "GET /api/chat/sessions/s/messages": () => new Promise(() => {}) } });
  const input = composer();
  await input.type("unsent");
  await input.attach();
  await input.send();
  assert.equal(input.draft, "");
  assert.equal(input.attachments, 0);
  assert.deepEqual(chat.sent(), []);
});

test("send joins a history repair already under way", async () => {
  const repair = deferred();
  let reads = 0, readsAtSend;
  const transcript = { messages: [userMessage("history", "earlier")] };
  const chat = await renderChat("/projects/p/tasks/s", { routes: {
    "GET /api/chat/sessions/s/messages": () => ++reads === 1 ? transcript : repair.promise,
    "POST /api/chat/sessions/s/message": () => { readsAtSend = reads; return { ok: true }; },
  } });
  await emit("resync.required", {});
  assert.equal(reads, 2);
  await composer().type("new");
  await composer().send();
  assert.deepEqual(chat.sent(), []);
  await repair.resolve(transcript);
  assert.deepEqual(chat.sent().map(({ text }) => text), ["new"]);
  assert.equal(readsAtSend, 2);
});

test("an edit typed while the send prepares survives the send", async () => {
  const history = deferred();
  const chat = await renderChat("/projects/p/tasks/s", { routes: {
    "GET /api/chat/sessions/s/messages": () => history.promise,
    "POST /api/chat/sessions/s/message": () => ({ ok: true }),
  } });
  const input = composer();
  await input.type("sent");
  await input.send();
  await input.type("typed since");
  await history.resolve({ messages: [] });
  assert.deepEqual(chat.sent().map(({ text }) => text), ["sent"]);
  assert.equal(input.draft, "typed since");
});

test("a late agent switch cannot replace a newer chat's selection or draft settings", async () => {
  const claude = {
    ...codex, id: "claude-code", name: "Claude Code", models: [model("target-model"), model("newer-model")],
    options: { ...codex.options, permissionModes: [{ id: "auto", label: "Auto" }, { id: "plan", label: "Plan" }] },
  };
  const switched = deferred();
  const chat = await renderChat("/projects/p/tasks/original", {
    sessions: [session("original"), session("side", { harness: "claude-code", model: "target-model", sideParentSessionId: "original" })],
    harnesses: [codex, claude],
    routes: { "PATCH /api/chat/sessions/original": () => switched.promise },
  });
  const picker = (index = 0) => [...document.querySelectorAll('[data-onboarding="composer"]')[index].querySelectorAll('[data-onboarding="model-picker"] button')]
    .map((button) => button.getAttribute("aria-label"));
  await composer().pick("Target Model", "Claude Code");

  await chat.navigate(`/projects/p/tasks/new?pane=${encodeURIComponent(JSON.stringify({ kind: "side", sessionId: "side" }))}`);
  await composer().type("/plan");
  await press("Enter", composer().field());
  await composer(1).pick("Newer Model", "Claude Code");
  const newer = picker();
  assert.deepEqual(newer, [m.a11y_chat_model({ label: "Newer Model" }), `${m.model_picker_permissions()}: Plan, ${m.model_picker_autonomy_agentic()}`]);

  await switched.resolve({ session: session("original", { harness: "claude-code", model: "target-model" }) });
  assert.deepEqual(picker(), newer);
  const preferred = chat.requests.filter(({ request, body }) => request === "POST /api/settings/ui-state" && body.preferredAgent);
  assert.deepEqual(preferred.map(({ body }) => body.preferredAgent.model), ["target-model", "newer-model"]);

  await chat.navigate("/projects/p/tasks/original");
  assert.equal(picker()[0], m.a11y_chat_model({ label: "Target Model" }));
});
