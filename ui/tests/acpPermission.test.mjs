import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { questionReplies } from "../src/chatRendering.ts";
import { Message } from "../src/components/ChatPanel.tsx";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, composer, deferred, press, renderChat, session, settle, userMessage } from "./chat.mjs";
import { render } from "./render.mjs";

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

/** Chat `s` waiting on `prompts` (`q1`, `q2`, …); the composer answers them through `respond`. */
async function asked(prompts, respond = () => ({ ok: true })) {
  const parts = prompts.map((prompt, index) => ({ id: `q${index + 1}`, type: "prompt", prompt: { kind: "question", ...prompt } }));
  const chat = await renderChat("/projects/p/tasks/s", {
    sessions: [session("s"), session("other")],
    transcripts: { s: [userMessage("u1", "free some space"), assistantMessage("a1", parts, "u1")] },
    routes: { "POST /api/chat/sessions/s/respond": respond },
  });
  const box = () => document.querySelector('[data-onboarding="composer"]');
  return {
    /** The open question's text, or undefined once none is open. */
    question: () => box().querySelector('[role="group"] [id]')?.textContent,
    option: (label) => [...box().querySelectorAll("button[aria-pressed]")].find((element) => element.textContent.startsWith(label)),
    control: (label) => button(box(), label),
    leave: () => chat.navigate("/projects/p/tasks/other"),
    deliveries: () => chat.requests.filter(({ request }) => request === "POST /api/chat/sessions/s/respond").map(({ body }) => body),
  };
}

/** Lets a single choice land and move the question on. */
const land = () => act(async () => new Promise((resolve) => setTimeout(resolve, 250)));

test("an open question turns the composer into its answer field", async () => {
  const view = await asked([{ question: "Can you free space?" }]);
  assert.equal(view.question(), "Can you free space?");
  assert.equal(composer().field().getAttribute("aria-label"), m.chat_question_your_answer());
  assert.equal(view.control(m.chat_panel_stop()), undefined);
  assert.equal(view.control(m.chat_panel_submit()).disabled, true);
  assert.equal(view.control(m.chat_panel_send()), undefined);
  await composer().type("Yes");
  assert.equal(view.control(m.chat_panel_submit()).disabled, false);
  const transcript = await card({ id: "q1", type: "prompt", prompt: { kind: "question", question: "Can you free space?" } });
  assert.equal(transcript.textContent.includes("Can you free space?"), false);
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

test("a typed answer goes out on Enter and survives a failed delivery", async () => {
  let delivery;
  const view = await asked([{ question: "Can you free space?" }], () => { delivery = deferred(); return delivery.promise; });
  await composer().type("What is using memory?");
  assert.deepEqual(view.deliveries(), []);
  await composer().send();
  assert.deepEqual(view.deliveries(), [{ promptId: "q1", answers: [], note: "What is using memory?" }]);
  assert.equal(view.control(m.chat_panel_submit()).disabled, true);
  assert.equal(composer().field().getAttribute("aria-readonly"), "true");
  await composer().type(" More");
  assert.equal(composer().draft, "What is using memory?");
  await delivery.resolve(Response.json({ error: "agent gone" }, { status: 409 }));
  assert.equal(composer().field().getAttribute("aria-readonly"), null);
  assert.equal(composer().draft, "What is using memory?");
  assert.equal(view.control(m.chat_panel_submit()).disabled, false);
  await composer().send();
  await delivery.resolve({ ok: true });
  assert.equal(view.question(), undefined);
  assert.equal(composer().draft, "");
});

test("one choice answers a single-select question, with the note typed before it", async () => {
  const view = await asked([{ question: "Which one?", options: [{ label: "First" }, { label: "Second" }] }]);
  assert.equal(composer().field().getAttribute("aria-label"), m.chat_question_custom_response());
  await composer().type("but keep the logs");
  await click(view.option("Second"));
  assert.equal(view.option("Second").getAttribute("aria-pressed"), "true");
  assert.deepEqual(view.deliveries(), []);
  await land();
  await settle();
  assert.deepEqual(view.deliveries(), [{ promptId: "q1", answers: ["Second"], note: "but keep the logs" }]);
});

test("a multi-select question takes several choices and waits for Submit", async () => {
  const view = await asked([{ question: "Which ones?", multiSelect: true, options: [{ label: "First" }, { label: "Second" }, { label: "Third" }] }]);
  assert.equal(view.control(m.chat_panel_submit()).disabled, true);
  await click(view.option("First"));
  await click(view.option("Third"));
  await click(view.option("First"));
  await click(view.option("Second"));
  await land();
  assert.deepEqual(view.deliveries(), []);
  await click(view.control(m.chat_panel_submit()));
  await settle();
  assert.deepEqual(view.deliveries(), [{ promptId: "q1", answers: ["Third", "Second"] }]);
});

test("several questions are stepped through, revisited with Back, and delivered together", async () => {
  const view = await asked([
    { question: "Which one?", options: [{ label: "First" }, { label: "Second" }] },
    { question: "Which ones?", multiSelect: true, options: [{ label: "Red" }, { label: "Green" }] },
    { question: "Anything else?" },
  ]);
  assert.equal(view.control(m.chat_question_back()).disabled, true);
  await click(view.option("First"));
  await land();
  assert.equal(view.question(), "Which ones?");
  assert.deepEqual(view.deliveries(), []);

  await click(view.control(m.chat_question_back()));
  assert.equal(view.question(), "Which one?");
  assert.equal(view.option("First").getAttribute("aria-pressed"), "true");
  await click(view.option("Second"));
  await land();

  await click(view.option("Green"));
  await click(view.control(m.chat_question_next()));
  assert.equal(view.question(), "Anything else?");
  assert.equal(view.control(m.chat_question_next()), undefined);
  await click(view.control(m.chat_question_skip()));
  await settle();
  assert.deepEqual(view.deliveries(), [
    { promptId: "q1", answers: ["Second"] },
    { promptId: "q2", answers: ["Green"] },
    { promptId: "q3", approve: false },
  ]);
  assert.equal(view.question(), undefined);
});

test("a delivery that fails partway keeps the answers that did not go out", async () => {
  const replies = [{ ok: true }, Response.json({ error: "agent gone" }, { status: 409 }), { ok: true }];
  const view = await asked(
    [{ question: "Which one?", options: [{ label: "First" }] }, { question: "Why?" }],
    () => replies.shift(),
  );
  await click(view.option("First"));
  await land();
  await composer().type("Because");
  await composer().send();
  await settle();
  assert.equal(view.deliveries().length, 2);
  assert.equal(view.question(), "Why?");
  assert.equal(composer().draft, "Because");
  assert.equal(view.control(m.chat_question_back()), undefined);
  await composer().send();
  await settle();
  assert.deepEqual(view.deliveries().at(-1), { promptId: "q2", answers: [], note: "Because" });
});

test("answers on their way still go out when the chat is left", async () => {
  let delivery;
  const view = await asked(
    [{ question: "Which one?", options: [{ label: "First" }] }, { question: "Why?" }],
    () => { delivery = deferred(); return delivery.promise; },
  );
  await click(view.option("First"));
  await land();
  await composer().type("Because");
  await composer().send();
  await view.leave();
  await delivery.resolve({ ok: true });
  assert.deepEqual(view.deliveries().at(-1), { promptId: "q2", answers: [], note: "Because" });
  await delivery.resolve({ ok: true });
});

test("number keys choose an option unless a text field has focus", async () => {
  const view = await asked([{ question: "Which ones?", multiSelect: true, options: [{ label: "First" }, { label: "Second" }] }]);
  await press("2", composer().field());
  assert.equal(view.option("Second").getAttribute("aria-pressed"), "false");
  await press("2", document.body);
  assert.equal(view.option("Second").getAttribute("aria-pressed"), "true");
  await press("3", document.body);
  await press("2", document.body, { metaKey: true });
  assert.equal(view.option("Second").getAttribute("aria-pressed"), "true");
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
