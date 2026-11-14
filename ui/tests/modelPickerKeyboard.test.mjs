import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { codex, renderChat, session } from "./chat.mjs";
import { press, settle, type } from "./render.mjs";
import { model } from "./models.mjs";

const claude = {
  ...codex, id: "claude-code", name: "Claude Code", models: [model("opus", "Opus 5.5")],
  options: { permissionModes: [{ id: "auto", label: "Auto" }], defaultPermissionMode: "auto", planActivation: "permission", reasoningLevels: [] },
};
const openPicker = async () => {
  await renderChat("/projects/p/tasks/s", { harnesses: [codex, claude] });
  await act(async () => document.querySelector('[data-onboarding="model-picker"] button').click());
  await settle();
};
const search = () => document.querySelector(`input[aria-label="${m.model_picker_search_models()}"]`);
const rail = (id) => document.querySelector(`[data-rail="${id}"]`);
const row = (model, agent) => document.querySelector(`button[aria-label="${m.model_picker_model_on({ model, agent })}"]`);

test("typing after moving focus into the list continues the search", async () => {
  await openPicker();
  await press("ArrowDown", search());
  assert.notEqual(document.activeElement, search());

  await press("o");
  assert.equal(document.activeElement, search());
  assert.equal(search().value, "o");
});

test("Backspace outside the search deletes from it", async () => {
  await openPicker();
  await type(search(), "opus");
  await press("ArrowDown", search());
  await press("Backspace");
  assert.equal(document.activeElement, search());
  assert.equal(search().value, "opu");
});

test("Left from a row enters the rail, Up and Down walk it, Right returns to its section", async () => {
  await openPicker();
  const first = row("Opus 5.5", "Claude Code");
  first.focus();
  await press("ArrowLeft");
  assert.equal(document.activeElement, rail("claude"));

  await press("ArrowDown");
  assert.equal(document.activeElement, rail("openai"));
  await press("ArrowRight");
  assert.equal(document.activeElement, document.querySelector('[data-section="openai"] [data-nav]'));
});

test("Left from a section's header enters the rail at that section", async () => {
  await openPicker();
  document.querySelector('[data-section="openai"] button[aria-expanded]').focus();
  await press("ArrowLeft");
  assert.equal(document.activeElement, rail("openai"));
});

test("Left from a search result stays on the result", async () => {
  await openPicker();
  await type(search(), "astra");
  const result = row("GPT-6 Astra", "Codex");
  result.focus();
  await press("ArrowLeft");
  assert.equal(document.activeElement, result);
});

test("Enter in the search picks the first match, and does nothing without one", async () => {
  const chat = await renderChat("/projects/p/tasks/s", {
    harnesses: [codex, claude],
    routes: { "PATCH /api/chat/sessions/s": (init) => ({ session: session("s", { harness: "claude-code", model: JSON.parse(init.body).agent.model }) }) },
  });
  await act(async () => document.querySelector('[data-onboarding="model-picker"] button').click());
  await press("Enter", search());
  await type(search(), "zzz");
  await press("Enter", search());
  await settle();
  assert.deepEqual(chat.requests.filter(({ request }) => request.startsWith("PATCH")), []);

  await type(search(), "opus");
  await press("Enter", search());
  await settle();
  assert.deepEqual(chat.requests.filter(({ request }) => request.startsWith("PATCH")).map(({ body }) => body.agent.model), ["opus"]);
});
