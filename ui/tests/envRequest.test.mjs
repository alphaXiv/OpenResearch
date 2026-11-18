import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { ltr } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, renderChat, session, userMessage } from "./chat.mjs";
import { find, press, type } from "./render.mjs";

const request = (variables, extra = {}) => ({
  id: "env_1",
  type: "prompt",
  prompt: { kind: "env", nativeId: "env_1", question: "Logging the sweep to W&B", variables, ...extra },
});

/** Chat `s`, its agent waiting on `card`; the server holds `saved` for project `p`. */
async function waiting(card, saved = { vars: [], projectVars: [] }) {
  const chat = await renderChat("/projects/p/tasks/s", {
    sessions: [session("s", { busy: true })],
    transcripts: { s: [userMessage("u1", "start the sweep"), assistantMessage("a1", [card], "u1")] },
    routes: {
      "GET /api/settings/env": () => saved,
      "POST /api/chat/sessions/s/respond": () => ({ ok: true }),
    },
  });
  const form = () => document.querySelector("form.env");
  return {
    form,
    field: (name) => form().querySelector(`input[aria-label="${m.a11y_value_for({ name: ltr(name) })}"]`),
    button: (label) => [...form().querySelectorAll("button")].find((element) => element.textContent === label),
    answers: () => chat.requests.filter(({ request }) => request === "POST /api/chat/sessions/s/respond").map(({ body }) => body),
    interrupted: () => chat.requests.some(({ request }) => request === "POST /api/chat/sessions/s/interrupt"),
  };
}

const click = (element) => act(async () => element.click());

test("an env card saves to this project by default and warns on a mismatch without blocking", async (t) => {
  const original = Object.getOwnPropertyDescriptor(globalThis, "Worker");
  let reportValidation;
  Object.defineProperty(globalThis, "Worker", {
    configurable: true,
    value: class {
      postMessage() { reportValidation = (names) => this.onmessage?.({ data: names }); }
      terminate() {}
    },
  });
  t.after(() => {
    if (original) Object.defineProperty(globalThis, "Worker", original);
    else delete globalThis.Worker;
  });

  const view = await waiting(request([
    { name: "WANDB_API_KEY", description: "From wandb.ai/authorize", secret: true, pattern: "^[0-9a-f]{40}$", example: "40 hex characters" },
    { name: "WANDB_PROJECT", secret: false },
  ]));

  assert.ok(view.form().textContent.includes("Logging the sweep to W&B"));
  assert.ok(view.form().textContent.includes("From wandb.ai/authorize"));
  assert.equal(view.field("WANDB_API_KEY").type, "password");
  assert.equal(view.field("WANDB_PROJECT").type, "text");
  assert.equal(view.button(m.common_save()).disabled, true);

  await type(view.field("WANDB_API_KEY"), "not-hex");
  assert.equal(view.button(m.common_save()).disabled, false);
  const warning = m.chat_env_mismatch_example({ example: ltr("40 hex characters") });
  assert.ok(!view.form().textContent.includes(warning));
  await act(async () => reportValidation(["WANDB_API_KEY"]));
  assert.ok(view.form().textContent.includes(warning));
  assert.equal(view.button(m.common_save()).disabled, false);

  const scope = m.chat_env_this_project();
  const trigger = view.button(scope);
  assert.equal(trigger.getAttribute("aria-haspopup"), "menu");
  assert.equal(trigger.closest("p").textContent, m.chat_env_footer({ scope }));
  await click(view.button(m.common_save()));

  assert.deepEqual(view.answers(), [{
    promptId: "env_1",
    approve: true,
    env: [{ name: "WANDB_API_KEY", value: "not-hex", scope: "project", mismatched: true }],
  }]);
});

test("choosing all projects on the env card applies to each value entered", async () => {
  const view = await waiting(request([
    { name: "HF_TOKEN", secret: true },
    { name: "WANDB_PROJECT", secret: false },
  ]), { vars: [{ key: "HF_TOKEN", maskedValue: "hf_…abcd", inProcessEnv: false }], projectVars: [] });

  assert.ok(!view.form().textContent.includes(m.chat_env_saved()));
  await type(view.field("HF_TOKEN"), "hf_test");
  await type(view.field("WANDB_PROJECT"), "sweeps");
  const trigger = view.button(m.chat_env_this_project());
  await click(trigger);
  assert.ok(find('[role="menuitemradio"][aria-checked="true"]', m.chat_env_save_to_project()));
  await press("Escape");
  assert.equal(trigger.getAttribute("aria-expanded"), "false");
  assert.equal(view.interrupted(), false);

  await click(trigger);
  await click(find('[role="menuitemradio"]', m.chat_env_save_for_all_projects()));

  assert.deepEqual(view.answers(), []);
  const scope = m.chat_env_all_projects();
  const selected = view.button(scope);
  assert.equal(selected.getAttribute("aria-expanded"), "false");
  assert.equal(selected.closest("p").textContent, m.chat_env_footer({ scope }));
  assert.ok(view.form().textContent.includes(m.chat_env_saved()));
  await click(view.button(m.common_save()));
  assert.deepEqual(view.answers(), [{
    promptId: "env_1",
    approve: true,
    env: [
      { name: "HF_TOKEN", value: "hf_test", scope: "global", mismatched: false },
      { name: "WANDB_PROJECT", value: "sweeps", scope: "global", mismatched: false },
    ],
  }]);
});

test("declining an env card sends no values, and a saved variable is marked", async () => {
  const view = await waiting(
    request([{ name: "HF_TOKEN", secret: true }]),
    { vars: [], projectVars: [{ key: "HF_TOKEN", maskedValue: "hf_…abcd", inProcessEnv: false }] },
  );
  assert.ok(view.form().textContent.includes(m.chat_env_saved()));
  await click(view.button(m.chat_env_decline()));
  assert.deepEqual(view.answers(), [{ promptId: "env_1", approve: false }]);
});

test("valid names shared with object properties start blank and can be saved", async () => {
  const names = ["constructor", "toString", "__proto__"];
  const view = await waiting(request(names.map((name) => ({ name, secret: false }))));

  for (const name of names) assert.equal(view.field(name).value, "");
  assert.ok(view.button(m.common_save()).disabled);
  await type(view.field("__proto__"), "configured");
  await click(view.button(m.common_save()));

  assert.deepEqual(view.answers(), [{
    promptId: "env_1",
    approve: true,
    env: [{ name: "__proto__", value: "configured", scope: "project", mismatched: false }],
  }]);
});

test("an answered env card records where each variable went, never its value", async () => {
  const chat = await renderChat("/projects/p/tasks/s", {
    transcripts: { s: [userMessage("u1", "start"), assistantMessage("a1", [request([
      { name: "WANDB_API_KEY", secret: true, savedTo: "global" },
      { name: "WANDB_ENTITY", secret: false },
    ], { resolved: true, approved: true })], "u1")] },
  });
  const text = chat.container.ownerDocument.body.textContent;
  assert.ok(text.includes(m.chat_env_saved_global({ name: ltr("WANDB_API_KEY") })));
  assert.ok(text.includes(m.chat_env_left_unset({ name: ltr("WANDB_ENTITY") })));
  assert.equal(document.querySelector("form.env"), null);
});
