import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { codex, composer, renderChat, session } from "./chat.mjs";
import { settle } from "./render.mjs";
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
  assert.ok(!document.body.textContent.includes(m.model_picker_new_chat_with({ agent: "Claude Code" })));

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

test("picking another agent's model on a busy chat opens an uncreated draft with it", async () => {
  const chat = await renderChat("/projects/p/tasks/s", { sessions: [session("s", { busy: true })], harnesses: [codex, claude] });
  await act(async () => document.querySelector('[data-onboarding="model-picker"] button').click());
  const label = m.model_picker_model_on({ model: "Opus 5.5", agent: "Claude Code" });
  const other = [...document.querySelectorAll(`button[aria-label="${label}"]`)];
  assert.ok(other.length > 0 && other.every((button) => button.getAttribute("aria-description") === m.model_picker_new_chat_with({ agent: "Claude Code" })));
  const own = document.querySelector(`button[aria-label="${m.model_picker_model_on({ model: "GPT-6 Astra", agent: "Codex" })}"]`);
  assert.equal(own.getAttribute("aria-description"), null);

  await act(async () => other[0].click());
  await settle();
  assert.equal(location.pathname, "/projects/p/tasks/new");
  assert.deepEqual(agentSwitches(chat), []);
  const preference = chat.requests.filter(({ request, body }) => request === "POST /api/settings/ui-state" && body.preferredAgent).at(-1).body.preferredAgent;
  assert.deepEqual(preference, { harness: "claude-code", profile: null, model: "opus", serviceTier: null, permissionMode: "auto", reasoningLevel: "default" });
  assert.match(document.querySelector('[data-onboarding="model-picker"] button').getAttribute("aria-label"), /Opus 5\.5/);
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
