import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { SkillsTab } from "../src/components/SkillsTab.tsx";
import { TARGET_LABELS } from "../src/computeTargets.ts";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, codex, composer, deferred, emit, renderChat, session, userMessage } from "./chat.mjs";
import { clients, find, press, render, renderApp, serve, serveApp, settle, type } from "./render.mjs";

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
    reply: (value) => ({ configured: true, contexts: [], currentContext: null, context: null, namespace: value, preflight: { kubectlFound: true, reachable: true, canCreateJobs: true } }),
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

test("harness settings list installed harnesses and saved custom agents, but not presets", async () => {
  const harness = (id, installed) => ({ ...codex, id, name: id, installed, agentReady: true });
  serveApp({
    "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project" }] }),
    "GET /api/harnesses": () => ({ harnesses: [harness("opencode", true), { ...harness("codex", false), agentReady: false }, harness("claude-code", true), harness("acp:custom", false), harness("acp:preset", false)] }),
    "GET /api/harnesses/acp": () => [{ id: "acp:custom", name: "acp:custom", executable: "custom", arguments: [] }],
    "GET /api/harnesses/acp/presets": () => [],
    "GET /api/harnesses/setup/commands": () => ({}),
  });
  await renderApp("/projects/p/settings/harnesses");
  const rows = [...document.querySelectorAll('button[aria-controls*="harness-details-"]')].map((row) => row.getAttribute("aria-controls").split("harness-details-")[1]);
  assert.deepEqual(rows, ["claude-code", "opencode", "acp:custom"]);
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
    ...codex, id: "claude-code", name: "Claude Code", models: [{ id: "target-model" }, { id: "newer-model" }],
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
  await composer().pick("target-model");

  await chat.navigate(`/projects/p/tasks/new?pane=${encodeURIComponent(JSON.stringify({ kind: "side", sessionId: "side" }))}`);
  await composer().type("/plan");
  await press("Enter", composer().field());
  await composer(1).pick("newer-model");
  const newer = picker();
  assert.deepEqual(newer, [m.a11y_chat_model({ label: "Newer Model" }), `${m.model_picker_permissions()}: Plan, ${m.model_picker_autonomy_agentic()}`]);

  await switched.resolve({ session: session("original", { harness: "claude-code", model: "target-model" }) });
  assert.deepEqual(picker(), newer);
  const preferred = chat.requests.filter(({ request, body }) => request === "POST /api/settings/ui-state" && body.preferredAgent);
  assert.deepEqual(preferred.map(({ body }) => body.preferredAgent.model), ["target-model", "newer-model"]);

  await chat.navigate("/projects/p/tasks/original");
  assert.equal(picker()[0], m.a11y_chat_model({ label: "Target Model" }));
});
