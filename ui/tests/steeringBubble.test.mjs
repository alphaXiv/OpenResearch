import assert from "node:assert/strict";
import test from "node:test";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, codex, emit, renderChat, session, userMessage } from "./chat.mjs";

const working = assistantMessage("a1", [{ id: "t1", type: "tool", tool: "Bash", state: { status: "running", input: { command: "sleep 20" } } }], "u1");
const transcripts = { s: [userMessage("u1", "benchmark it"), working] };

const open = () => renderChat("/projects/p/tasks/s", {
  sessions: [session("s", { busy: true })],
  transcripts,
  harnesses: [{ ...codex, supportsSteering: true }],
});
const thread = () => document.querySelector(".chat-thread-inner");
const steering = (text) => ({ id: `q-${text}`, text, dispatchState: "steering" });

test("an unread steer sits below the running turn as your message, marked steering", async () => {
  await open();
  await emit("chat.queued", { sessionId: "s", items: [steering("stop"), { id: "q-later", text: "then summarize", dispatchState: "queued" }] });

  const pending = thread().querySelector(".msg-steer-pending");
  assert.equal(pending.querySelector('[role="note"]').textContent, "stop");
  assert.equal(pending.querySelector('[role="status"]').textContent, m.chat_steering());
  assert.equal(pending, thread().lastElementChild);
  const chips = [...document.querySelectorAll(".queued-chip")].map((chip) => chip.textContent);
  assert.equal(chips.length, 1);
  assert.ok(chips[0].includes("then summarize"));
});

test("a steer the agent read moves to where it read it", async () => {
  await open();
  await emit("chat.queued", { sessionId: "s", items: [steering("stop")] });
  await emit("chat.message", {
    sessionId: "s",
    message: { ...working, parts: [{ ...working.parts[0], state: { status: "completed", input: { command: "sleep 20" } } }, { id: "steer-1", type: "steer", text: "stop" }] },
  });
  await emit("chat.queued", { sessionId: "s", items: [] });

  assert.equal(thread().querySelector(".msg-steer-pending"), null);
  assert.equal(document.body.textContent.includes(m.chat_steering()), false);
  assert.equal(thread().querySelectorAll(".msg-steer").length, 1);
});

test("several unread steers share one steering status", async () => {
  await open();
  await emit("chat.queued", { sessionId: "s", items: [steering("stop"), steering("pause the runs")] });

  const notes = [...thread().querySelectorAll('.msg-steer-pending [role="note"]')].map((note) => note.textContent);
  assert.deepEqual(notes, ["stop", "pause the runs"]);
  assert.equal(thread().querySelectorAll('.msg-steer-pending [role="status"]').length, 1);
});
