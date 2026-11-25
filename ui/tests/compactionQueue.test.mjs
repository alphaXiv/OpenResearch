import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, emit, renderChat, session, userMessage } from "./chat.mjs";

const compaction = (status) => assistantMessage("c1", [{ id: "compacted", type: "tool", tool: "compacted", state: { status } }], "u1");
const queued = (text) => ({ id: `q-${text}`, text, dispatchState: "queued" });

const open = (status, fields = {}) => renderChat("/projects/p/tasks/s", {
  sessions: [session("s", { busy: status === "running", ...fields })],
  transcripts: { s: [userMessage("u1", "benchmark it"), compaction(status)] },
  routes: { "DELETE /api/chat/sessions/s/queue/*": () => ({ ok: true, removed: true }) },
});
const thread = () => document.querySelector(".chat-thread-inner");
const chips = () => [...document.querySelectorAll(".queued-chip")].map((chip) => chip.textContent);

test("the message sent during a compaction waits below it like a steer, and later ones stay queued", async () => {
  await open("running");
  await emit("chat.queued", { sessionId: "s", items: [queued("now summarize"), queued("then plot it")] });

  const pending = thread().querySelector(".msg-steer-pending");
  assert.equal(pending.querySelector('[role="note"]').textContent, "now summarize");
  assert.equal(pending.querySelector('[role="status"]').textContent, m.chat_sends_after_compacting());
  assert.equal(chips().length, 1);
  assert.ok(chips()[0].includes("then plot it"));
});

test("the message waiting on a compaction can still be removed", async () => {
  const { requests } = await open("running");
  await emit("chat.queued", { sessionId: "s", items: [queued("now summarize")] });

  const remove = document.querySelector(`[aria-label="${m.a11y_remove_queued_message({ text: "now summarize" })}"]`);
  await act(async () => remove.click());

  assert.ok(requests.some(({ request }) => request === "DELETE /api/chat/sessions/s/queue/q-now%20summarize"));
});

test("a done compaction keeps its message waiting until the queue sends it", async () => {
  await open("completed");
  await emit("chat.queued", { sessionId: "s", items: [queued("now summarize")] });

  assert.equal(thread().querySelector('.msg-steer-pending [role="note"]').textContent, "now summarize");
});

const held = [
  ["a stopped compaction", "error", {}],
  ["a usage limit after a done compaction", "completed", { limitTimerStopped: true }],
];
for (const [name, status, fields] of held) {
  test(`${name} leaves the message queued`, async () => {
    await open(status, fields);
    await emit("chat.queued", { sessionId: "s", items: [queued("now summarize")] });

    assert.equal(thread().querySelector(".msg-steer-pending"), null);
    assert.ok(chips()[0].includes("now summarize"));
  });
}
