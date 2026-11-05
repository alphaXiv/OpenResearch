import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { questionReplies } from "../src/chatRendering.ts";
import { Message } from "../src/components/ChatPanel.tsx";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, deferred, renderChat, userMessage } from "./chat.mjs";
import { render, settle, type } from "./render.mjs";

/** Renders `part` as an assistant reply; `answer`, when given, is the user's reply that follows it. */
async function card(part, { onRespond, answer } = {}) {
  const message = { id: "msg-1", role: "assistant", parts: [part], createdAt: 0, completedAt: 1000 };
  const transcript = answer ? [message, { id: "msg-2", role: "user", parts: [{ id: "t", type: "text", text: answer }], createdAt: 1 }] : [message];
  const { container } = await render(createElement(Message, {
    message, replies: questionReplies(transcript), activePermissionId: part.prompt.kind === "permission" ? part.id : null,
    forkDisabled: false, branchDisabled: false, onFork: () => {}, onSelectFork: () => {}, onRecover: () => {}, onRespond,
  }));
  return container;
}

const click = (element) => act(async () => element.click());
const submit = (container) => act(async () => container.querySelector("form").requestSubmit());
const field = (container) => container.querySelector(`textarea[aria-label="${m.chat_question_your_answer()}"]`);
const button = (container, text) => [...container.querySelectorAll("button")].find((element) => element.textContent === text || element.getAttribute("aria-label") === text);

test("native permission cards render exact agent labels in their original order", async () => {
  const part = { id: "permission", type: "prompt", prompt: { kind: "permission", header: "Inspect the file", nativeChoices: [
    { id: "opaque/reject", label: "Skip this operation", kind: "reject_once" },
    { id: "opaque/allow", label: "Allow exactly once", kind: "allow_once" },
  ] } };
  const responses = [];
  const live = (await card(part, { onRespond: async (answer) => { responses.push(answer); return true; } })).querySelector('[role="group"]');
  assert.ok(live.textContent.includes("Inspect the file"));
  assert.deepEqual([...live.querySelectorAll("button")].map((element) => element.textContent), ["Skip this operation", "Allow exactly once"]);
  await click(button(live, "Allow exactly once"));
  assert.deepEqual(responses, [{ promptId: "permission", answers: ["opaque/allow"], approve: true }]);
  assert.equal((await card({ ...part, prompt: { ...part.prompt, resolved: true } })).querySelector('[role="group"]')?.outerHTML, undefined);
});

/** Chat `s` waiting on `prompt`; the dock's card answers through `respond`. */
async function asked(prompt, respond = () => ({ ok: true })) {
  const chat = await renderChat("/projects/p/tasks/s", {
    transcripts: { s: [userMessage("u1", "free some space"), assistantMessage("a1", [{ id: "question", type: "prompt", prompt }], "u1")] },
    routes: { "POST /api/chat/sessions/s/respond": respond },
  });
  return {
    view: document.querySelector('[role="group"]'),
    deliveries: () => chat.requests.filter(({ request }) => request === "POST /api/chat/sessions/s/respond").map(({ body }) => body),
  };
}

test("a free-text question provides its own labeled answer field and submit action", async () => {
  const part = { id: "question", type: "prompt", prompt: { kind: "question", question: "Can you free space?" } };
  const { view } = await asked(part.prompt);
  assert.ok(field(view));
  assert.equal(button(view, m.chat_panel_send()).type, "submit");
  assert.equal(button(view, m.chat_panel_send()).disabled, true);
  const readonly = await card(part);
  assert.equal(readonly.querySelector("form")?.outerHTML, undefined);
  assert.equal(field(readonly)?.outerHTML, undefined);
});

test("answered questions show the agent question followed by one quoted answer", async () => {
  const part = { id: "question", type: "prompt", prompt: { kind: "question", question: "Can you free space?", resolved: true, note: "What is using memory?" } };
  const inTranscript = await card(part, { answer: "In response to: Can you free space?\n\nWhat is using memory?" });
  assert.ok(inTranscript.textContent.includes("Can you free space"));
  assert.equal(inTranscript.textContent.includes("What is using memory"), false);
  const native = await card(part);
  assert.ok(native.textContent.includes("What is using memory"));
  assert.equal(native.textContent.split("Can you free space").length - 1, 2);
});

test("custom answers submit explicitly and keep their draft when delivery fails", async () => {
  let delivery;
  const { view, deliveries } = await asked({ kind: "question", question: "Can you free space?" }, () => { delivery = deferred(); return delivery.promise; });
  await type(field(view), "What is using memory?\nPlease list the files.");
  assert.deepEqual(deliveries(), []);
  await submit(view);
  assert.deepEqual(deliveries(), [{ promptId: "question", answers: [], note: "What is using memory?\nPlease list the files." }]);
  assert.equal(field(view).disabled, true);
  await delivery.resolve(Response.json({ error: "agent gone" }, { status: 409 }));
  assert.equal(field(view).value, "What is using memory?\nPlease list the files.");
  assert.equal(field(view).disabled, false);
  await submit(view);
  await delivery.resolve({ ok: true });
  assert.equal(field(view).value, "");
});

test("question choices wait for Send, custom text replaces a choice, and Skip is explicit", async () => {
  const { view, deliveries } = await asked({ kind: "question", question: "Which one?", options: [{ label: "First" }, { label: "Second" }] });
  const first = [...view.querySelectorAll("button[aria-pressed]")].find((element) => element.textContent.includes("First"));
  await click(first);
  assert.deepEqual(deliveries(), []);
  assert.equal(first.getAttribute("aria-pressed"), "true");
  assert.equal(first.type, "button");
  await submit(view);
  await settle();
  assert.deepEqual(deliveries()[0], { promptId: "question", answers: ["First"] });
  await click(first);
  await type(field(view), "Something else");
  assert.equal(view.querySelector('button[aria-pressed="true"]')?.outerHTML, undefined);
  await submit(view);
  await settle();
  assert.deepEqual(deliveries()[1], { promptId: "question", answers: [], note: "Something else" });
  await click(button(view, m.chat_question_skip()));
  await settle();
  assert.deepEqual(deliveries()[2], { promptId: "question", approve: false });
});

test("skipped questions show their outcome while preserving historical answer text", async () => {
  const part = { id: "question", type: "prompt", prompt: { kind: "question", question: "Which color?", resolved: true, approved: false } };
  const skipped = await card(part);
  assert.ok(skipped.textContent.includes(m.chat_question_skipped()));
  assert.ok(skipped.querySelector("details > summary"));
  const historical = await card({ ...part, prompt: { ...part.prompt, note: "Previously entered answer" } });
  assert.ok(historical.textContent.includes("Previously entered answer"));
  assert.equal(historical.textContent.includes(m.chat_question_skipped()), false);
  const stale = await card({ ...part, prompt: { ...part.prompt, approved: undefined } });
  assert.equal(stale.textContent.includes(m.chat_question_skipped()), false);
});
