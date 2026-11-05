import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { OptionPicker } from "../src/components/ModelPicker.tsx";
import { m } from "../src/paraglide/messages.js";
import { composer, renderChat, session } from "./chat.mjs";
import { render, settle } from "./render.mjs";

const sideTab = (sessionId) => `?pane=${encodeURIComponent(JSON.stringify({ kind: "side", sessionId }))}`;
const pointer = (target, type, pointerType = "mouse") => target.dispatchEvent(new PointerEvent(type, { bubbles: true, button: 0, pointerType }));

/** Picks the autonomy `label` in the `index`th composer's permissions menu. */
async function chooseAutonomy(index, label) {
  const box = document.querySelectorAll('[data-onboarding="composer"]')[index];
  const settings = [...box.querySelectorAll('[data-onboarding="model-picker"] button')].find((button) => button.getAttribute("aria-label")?.startsWith(m.model_picker_permissions()));
  for (const type of ["pointerdown", "mousedown", "pointerup", "mouseup", "click"]) {
    await act(async () => settings.dispatchEvent(new (type.startsWith("pointer") ? PointerEvent : MouseEvent)(type, { bubbles: true, button: 0, pointerType: "mouse" })));
  }
  await settle();
  await act(async () => [...document.querySelectorAll('[role="menuitemradio"]')].find((item) => item.textContent.includes(label)).click());
  await settle();
}

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

for (const side of [true, false]) {
  test(`${side ? "an unsaved side chat keeps its autonomy local" : "a new root chat's autonomy updates the default"}`, async () => {
    const chat = await renderChat(side ? `/projects/p/tasks/s${sideTab(null)}` : "/projects/p/tasks/new", { sessions: [session("s")] });
    await chooseAutonomy(side ? 1 : 0, m.model_picker_autonomy_copilot());
    const defaults = chat.requests.filter(({ request, body }) => request === "POST /api/settings/ui-state" && body.preferredAutonomy);
    assert.deepEqual(defaults.map(({ body }) => body.preferredAutonomy), side ? [] : ["copilot"]);
  });
}

test("first send applies draft autonomy to the newly created side chat", async () => {
  const sessions = [session("s")];
  const child = session("child", { sideParentSessionId: "s", autonomy: "agentic" });
  const chat = await renderChat(`/projects/p/tasks/s${sideTab(null)}`, { sessions, routes: {
    "POST /api/chat/sessions/s/side": () => { sessions.push(child); return { session: child }; },
    "PATCH /api/chat/sessions/child": (init) => ({ session: { ...child, ...JSON.parse(init.body) } }),
    "GET /api/chat/sessions/child/messages": () => ({ messages: [] }),
    "POST /api/chat/sessions/child/message": () => ({ ok: true }),
  } });
  await chooseAutonomy(1, m.model_picker_autonomy_copilot());
  await composer(1).type("side question");
  await composer(1).send();
  const order = chat.requests.map(({ request }) => request).filter((request) => /^(POST|PATCH) \/api\/chat/.test(request));
  assert.deepEqual(order, ["POST /api/chat/sessions/s/side", "PATCH /api/chat/sessions/child", "POST /api/chat/sessions/child/message"]);
  assert.deepEqual(chat.requests.find(({ request }) => request === "PATCH /api/chat/sessions/child").body, { autonomy: "copilot" });
});
