import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { OptionPicker } from "../src/components/ModelPicker.tsx";
import { render } from "./render.mjs";

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
