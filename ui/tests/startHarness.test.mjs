import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { codex, composer, renderChat, session } from "./chat.mjs";
import { settle } from "./render.mjs";

/** The open dialog's button labelled `label`. */
const dialogButton = (label) => [...document.querySelectorAll('[role="dialog"] button, [role="alertdialog"] button')].find((button) => button.textContent.trim() === label);
import { model } from "./models.mjs";

const claude = {
  ...codex, id: "claude-code", name: "Claude Code", models: [model("opus", "Opus 5.5")],
  options: { permissionModes: [{ id: "auto", label: "Auto" }], defaultPermissionMode: "auto", planActivation: "permission", reasoningLevels: [] },
};
const sideTab = (sessionId) => `?pane=${encodeURIComponent(JSON.stringify({ kind: "side", sessionId }))}`;
const sessions = () => [session("s"), session("side", { sideParentSessionId: "s" }), session("other")];

const agentSwitches = (chat) => chat.requests.filter(({ request, body }) => request === "PATCH /api/chat/sessions/s" && body.agent).map(({ body }) => body.agent);

test("picking another agent's model in a started chat hands the chat over, with every agent's models in their labs", async () => {
  const chat = await renderChat("/projects/p/tasks/s", {
    harnesses: [codex, claude],
    routes: { "PATCH /api/chat/sessions/s": (init) => ({ session: session("s", { harness: "claude-code", model: JSON.parse(init.body).agent.model }) }) },
  });
  await act(async () => document.querySelector('[data-onboarding="model-picker"] button').click());
  const sections = [...document.querySelectorAll("[data-section]")].map((section) => section.dataset.section);
  assert.deepEqual(sections, ["claude", "openai"]);

  await act(async () => document.querySelector(`button[aria-label="${m.model_picker_model_on({ model: "Opus 5.5", agent: "Claude Code" })}"]`).click());
  await settle();
  assert.equal(location.pathname, "/projects/p/tasks/s");
  assert.deepEqual(agentSwitches(chat), [{ harness: "claude-code", profile: null, model: "opus", serviceTier: null, permissionMode: "auto", reasoningLevel: "default" }]);
  assert.match(document.querySelector('[data-onboarding="model-picker"] button').getAttribute("aria-label"), /Opus 5\.5/);
});

test("picking a model on another account of the chat's agent hands the chat to that account", async () => {
  const work = { ...codex, profile: { id: "codex-0123456789", name: "Work" } };
  const chat = await renderChat("/projects/p/tasks/s", {
    harnesses: [codex, work],
    routes: { "PATCH /api/chat/sessions/s": () => ({ session: session("s", { profile: "codex-0123456789" }) }) },
  });
  await composer().pick("GPT-6 Astra", "Work");
  assert.deepEqual(agentSwitches(chat).map(({ harness, profile, model }) => ({ harness, profile, model })), [{ harness: "codex", profile: "codex-0123456789", model: "gpt-6-astra" }]);
});

test("a handoff the server refuses says why and keeps the chat on its agent", async () => {
  const error = "Claude Code lists no models yet. Sign in to it, then try again.";
  const chat = await renderChat("/projects/p/tasks/s", {
    harnesses: [codex, claude],
    routes: { "PATCH /api/chat/sessions/s": () => Response.json({ error }, { status: 400 }) },
  });
  await composer().pick("Opus 5.5", "Claude Code");
  assert.equal(agentSwitches(chat).length, 1);
  assert.equal(location.pathname, "/projects/p/tasks/s");
  assert.ok(document.body.textContent.includes(error));
});

test("another agent picked during a turn takes the chat over with the next message", async () => {
  const chat = await renderChat("/projects/p/tasks/s", {
    sessions: [session("s", { busy: true })],
    harnesses: [{ ...codex, supportsSteering: true }, claude],
    routes: { "POST /api/chat/sessions/s/takeover": () => ({ ok: true, turn: { turnId: "ct", queued: true, existing: false } }) },
  });
  await composer().pick("Opus 5.5", "Claude Code");
  assert.equal(location.pathname, "/projects/p/tasks/s");
  assert.deepEqual(agentSwitches(chat), []);
  assert.match(document.querySelector('[data-onboarding="model-picker"] button').getAttribute("aria-label"), /Opus 5\.5/);

  await composer().type("take it from here");
  await composer().send();
  const posts = () => chat.requests.filter(({ request }) => request === "POST /api/chat/sessions/s/takeover").map(({ body }) => body);
  assert.deepEqual(posts(), [], "nothing is sent until the user agrees to stop the turn");
  const dialog = document.querySelector('[role="dialog"], [role="alertdialog"]');
  assert.match(dialog.textContent, /Codex/, "the dialog names the agent whose turn stops");
  assert.match(dialog.textContent, /Claude Code/, "and the agent that takes over");
  await act(async () => dialogButton(m.chat_takeover_confirm()).click());
  await settle();
  const sent = posts();
  assert.equal(sent.length, 1);
  assert.equal(sent[0].mode, undefined, "a takeover never steers the running turn");
  assert.deepEqual(sent[0].agent, { harness: "claude-code", profile: null, model: "opus", serviceTier: null, permissionMode: "auto", reasoningLevel: "default" });
});

test("a side composer restores its draft and attachments after unmount", async () => {
  const chat = await renderChat(`/projects/p/tasks/s${sideTab("side")}`, { sessions: sessions() });
  await composer(1).type("unsent");
  await composer(1).attach();
  const draft = composer(1).draft;
  await chat.navigate("/projects");
  await chat.navigate(`/projects/p/tasks/s${sideTab("side")}`);
  assert.equal(composer(1).draft, draft);
  assert.equal(composer(1).attachments, 1);
});

test("a new workspace starts with empty composers, and late panel cleanups cannot restore old drafts", async () => {
  const chat = await renderChat(`/projects/p/tasks/s${sideTab("side")}`, { sessions: sessions(), remote: true });
  await composer(0).type("main");
  await composer(1).type("side");
  await chat.navigate("/projects/p/tasks/other");
  await chat.navigate(`/projects/p/tasks/s${sideTab("side")}`);
  assert.deepEqual([composer(0).draft, composer(1).draft], ["main", "side"]);

  await chat.reconnect();
  await chat.navigate(`/projects/p/tasks/s${sideTab("side")}`);
  assert.deepEqual([composer(0).draft, composer(1).draft], ["", ""]);

  await composer(0).type("main");
  await composer(1).type("side");
  await chat.navigate("/projects/p/tasks/other");
  await chat.navigate(`/projects/p/tasks/s${sideTab("side")}`);
  assert.deepEqual([composer(0).draft, composer(1).draft], ["main", "side"]);
});

test("a refused takeover keeps the picked agent for the next try", async () => {
  const error = "This chat already uses another ACP agent. Start a new chat for this agent.";
  await renderChat("/projects/p/tasks/s", {
    sessions: [session("s", { busy: true })],
    harnesses: [codex, claude],
    routes: { "POST /api/chat/sessions/s/takeover": () => Response.json({ error }, { status: 400 }) },
  });
  await composer().pick("Opus 5.5", "Claude Code");
  await composer().type("take it from here");
  await composer().send();
  await act(async () => dialogButton(m.chat_takeover_confirm()).click());
  await settle();
  assert.ok(document.body.textContent.includes(error));
  assert.match(document.querySelector('[data-onboarding="model-picker"] button').getAttribute("aria-label"), /Opus 5\.5/);
});

test("permissions picked for the pending agent wait for the takeover", async () => {
  const asking = { ...claude, options: { ...claude.options, permissionModes: [{ id: "auto", label: "Auto" }, { id: "ask", label: "Ask" }] } };
  const chat = await renderChat("/projects/p/tasks/s", {
    sessions: [session("s", { busy: true })],
    harnesses: [codex, asking],
    routes: { "POST /api/chat/sessions/s/takeover": () => ({ ok: true, turn: { turnId: "ct", queued: true, existing: false } }) },
  });
  await composer().pick("Opus 5.5", "Claude Code");
  const settings = [...document.querySelectorAll('[data-onboarding="model-picker"] button')].find((button) => button.getAttribute("aria-label")?.startsWith(m.model_picker_permissions()));
  for (const type of ["pointerdown", "mousedown", "pointerup", "mouseup", "click"]) {
    await act(async () => settings.dispatchEvent(new (type.startsWith("pointer") ? PointerEvent : MouseEvent)(type, { bubbles: true, button: 0, pointerType: "mouse" })));
  }
  await settle();
  await act(async () => [...document.querySelectorAll('[role="menuitemradio"]')].find((item) => item.textContent.includes("Ask")).click());
  await settle();
  assert.deepEqual(chat.requests.filter(({ request }) => request.startsWith("PATCH")), [], "the running Codex turn keeps its own permissions");

  await composer().type("take it from here");
  await composer().send();
  await act(async () => dialogButton(m.chat_takeover_confirm()).click());
  await settle();
  const sent = chat.requests.find(({ request }) => request === "POST /api/chat/sessions/s/takeover");
  assert.equal(sent.body.agent.permissionMode, "ask");
});

test("permissions picked after a model on the chat's own agent are saved at once", async () => {
  const asking = { ...codex, options: { ...codex.options, permissionModes: [{ id: "auto", label: "Auto" }, { id: "ask", label: "Ask" }] } };
  const chat = await renderChat("/projects/p/tasks/s", {
    harnesses: [asking],
    routes: { "PATCH /api/chat/sessions/s": (init) => ({ session: session("s", JSON.parse(init.body)) }) },
  });
  await composer().pick("GPT-6 Astra");
  const settings = [...document.querySelectorAll('[data-onboarding="model-picker"] button')].find((button) => button.getAttribute("aria-label")?.startsWith(m.model_picker_permissions()));
  for (const type of ["pointerdown", "mousedown", "pointerup", "mouseup", "click"]) {
    await act(async () => settings.dispatchEvent(new (type.startsWith("pointer") ? PointerEvent : MouseEvent)(type, { bubbles: true, button: 0, pointerType: "mouse" })));
  }
  await settle();
  await act(async () => [...document.querySelectorAll('[role="menuitemradio"]')].find((item) => item.textContent.includes("Ask")).click());
  await settle();
  const saved = chat.requests.filter(({ request, body }) => request === "PATCH /api/chat/sessions/s" && body.permissionMode).map(({ body }) => body.permissionMode);
  assert.deepEqual(saved, ["ask"]);
});

test("a remote too old to take over says to stop the turn instead", async () => {
  const chat = await renderChat("/projects/p/tasks/s", { sessions: [session("s", { busy: true })], harnesses: [codex, claude], remote: true });
  await composer().pick("Opus 5.5", "Claude Code");
  assert.ok(document.body.textContent.includes(m.chat_takeover_needs_update()));
  assert.match(document.querySelector('[data-onboarding="model-picker"] button').getAttribute("aria-label"), /GPT/);
  assert.deepEqual(chat.requests.filter(({ request }) => request.startsWith("PATCH") || request.endsWith("/takeover")), []);
});

test("another agent picked in a busy side chat stays in the side chat", async () => {
  const chat = await renderChat(`/projects/p/tasks/s${sideTab("side")}`, {
    sessions: [session("s"), session("side", { sideParentSessionId: "s", busy: true })],
    harnesses: [codex, claude],
    routes: { "POST /api/chat/sessions/side/takeover": () => ({ ok: true, turn: { turnId: "ct", queued: true, existing: false } }) },
  });
  await composer(1).pick("Opus 5.5", "Claude Code");
  assert.equal(location.pathname, "/projects/p/tasks/s");
  assert.deepEqual(chat.requests.filter(({ request }) => request.startsWith("PATCH")), []);

  await composer(1).type("side takeover");
  await composer(1).send();
  await act(async () => dialogButton(m.chat_takeover_confirm()).click());
  await settle();
  const sent = chat.requests.find(({ request }) => request === "POST /api/chat/sessions/side/takeover");
  assert.equal(sent.body.agent.harness, "claude-code");
  assert.equal(location.pathname, "/projects/p/tasks/s");
});
