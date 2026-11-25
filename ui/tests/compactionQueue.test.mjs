import assert from "node:assert/strict";
import test from "node:test";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, emit, renderChat, session, userMessage } from "./chat.mjs";

const compaction = (status) => assistantMessage("c1", [{ id: "compacted", type: "tool", tool: "compacted", state: { status } }], "u1");
const queued = (text) => ({ id: `q-${text}`, text, dispatchState: "queued" });

const open = (status) => renderChat("/projects/p/tasks/s", {
  sessions: [session("s", { busy: status === "running" })],
  transcripts: { s: [userMessage("u1", "benchmark it"), compaction(status)] },
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

test("a stopped compaction leaves its message queued", async () => {
  await open("error");
  await emit("chat.queued", { sessionId: "s", items: [queued("now summarize")] });

  assert.equal(thread().querySelector(".msg-steer-pending"), null);
  assert.ok(chips()[0].includes("now summarize"));
});
