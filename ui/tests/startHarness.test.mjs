import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { codex, composer, renderChat, session } from "./chat.mjs";
import { settle } from "./render.mjs";

const claude = {
  ...codex, id: "claude-code", name: "Claude Code", models: [{ id: "opus", displayName: "Opus 5.5" }],
  options: { permissionModes: [{ id: "auto", label: "Auto" }], defaultPermissionMode: "auto", planActivation: "permission", reasoningLevels: [] },
};
const sideTab = (sessionId) => `?pane=${encodeURIComponent(JSON.stringify({ kind: "side", sessionId }))}`;
const sessions = () => [session("s"), session("side", { sideParentSessionId: "s" }), session("other")];

test("picking another harness on a busy chat opens an uncreated draft with it", async () => {
  const chat = await renderChat("/projects/p/tasks/s", { sessions: [session("s", { busy: true })], harnesses: [codex, claude] });
  await act(async () => document.querySelector('[data-onboarding="model-picker"] button').click());
  await act(async () => [...document.querySelectorAll(`button[aria-description="${m.model_picker_new_chat_harness()}"]`)].find((item) => item.textContent.includes("Claude Code")).click());
  await settle();
  assert.equal(location.pathname, "/projects/p/tasks/new");
  const preference = chat.requests.filter(({ request, body }) => request === "POST /api/settings/ui-state" && body.preferredAgent).at(-1).body.preferredAgent;
  assert.deepEqual(preference, { harness: "claude-code", model: "opus", serviceTier: null, permissionMode: "auto", reasoningLevel: "default" });
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
