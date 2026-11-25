import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { OptionPicker } from "../src/components/ModelPicker.tsx";
import { codex, composer, renderChat, session } from "./chat.mjs";
import { model } from "./models.mjs";
import { render } from "./render.mjs";

const sideTab = (sessionId) => `?pane=${encodeURIComponent(JSON.stringify({ kind: "side", sessionId }))}`;

const pointer = (target, type, pointerType = "mouse") => target.dispatchEvent(new PointerEvent(type, { bubbles: true, button: 0, pointerType }));

test("side-chat trigger opens the list on touch and a draft on mouse click", async () => {
  let created = 0;
  const { container } = await render(createElement(OptionPicker, {
    triggerIcon: "+", triggerLabel: "Side chat", onTriggerClick: () => created++, openOnHover: true,
    choices: [{ id: "saved", label: "Saved chat" }], value: null, onSelect: () => {},
  }));
  const trigger = container.querySelector('button[aria-haspopup="menu"]');
  await act(async () => {
    pointer(trigger, "pointerdown", "touch");
    trigger.click();
  });
  assert.equal(created, 0);
  assert.equal(trigger.getAttribute("aria-expanded"), "true");
  await act(async () => {
    pointer(trigger, "pointerdown");
    trigger.click();
  });
  assert.equal(created, 1);
  assert.equal(trigger.getAttribute("aria-expanded"), "false");
});

test("an unsent side chat starts on another agent's model and leaves the main chat open", async () => {
  const claude = {
    ...codex, id: "claude-code", name: "Claude Code", models: [model("opus", "Opus 5.5")],
    options: { permissionModes: [{ id: "auto", label: "Auto" }], defaultPermissionMode: "auto", planActivation: "permission", reasoningLevels: [] },
  };
  const sessions = [session("s")];
  const child = session("child", { sideParentSessionId: "s", harness: "claude-code", model: "opus" });
  const chat = await renderChat(`/projects/p/tasks/s${sideTab(null)}`, { sessions, harnesses: [codex, claude], routes: {
    "POST /api/chat/sessions/s/side": () => { sessions.push(child); return { session: child }; },
    "GET /api/chat/sessions/child/messages": () => ({ messages: [] }),
    "POST /api/chat/sessions/child/message": () => ({ ok: true }),
  } });
  await composer(1).pick("Opus 5.5", "Claude Code");
  assert.equal(location.pathname, "/projects/p/tasks/s");
  assert.match(document.querySelectorAll('[data-onboarding="model-picker"]')[0].querySelector("button").getAttribute("aria-label"), /GPT-6\.1 Sol/i);

  await composer(1).type("side question");
  await composer(1).send();
  assert.equal(location.pathname, "/projects/p/tasks/s");
  const open = chat.requests.find(({ request }) => request === "POST /api/chat/sessions/s/side");
  assert.deepEqual(open.body.agent, { harness: "claude-code", profile: null, model: "opus", serviceTier: null, permissionMode: "auto", reasoningLevel: "default" });
});
