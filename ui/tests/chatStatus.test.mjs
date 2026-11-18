import assert from "node:assert/strict";
import { test } from "node:test";
import { chatStatus, chatStatusWorking } from "../src/chatStatus.ts";

const session = (lastTurn) => ({ id: "chat", updatedAt: 500, lastTurn });
const turn = (state, extra = {}) => ({ state, startedAt: 100, errorKind: null, error: null, ...extra });
const run = (status, createdAt, startedAt = null) => ({ sessionId: "chat", experimentName: null, run: { status, createdAt, startedAt } });

test("a question outranks an experiment, which outranks the agent's own work", () => {
  const runs = [run("running", 10, 20)];
  assert.deepEqual(chatStatus(session(turn("running")), true, true, runs), { kind: "input" });
  assert.deepEqual(chatStatus(session(turn("running")), true, false, runs), { kind: "running", since: 20, runs: 1 });
  assert.deepEqual(chatStatus(session(turn("running")), true, false, []), { kind: "working", since: 100 });
});

test("an experiment times from its earliest start, and waits as queued until one runs", () => {
  assert.deepEqual(chatStatus(session(null), false, false, [run("starting", 30), run("running", 10, 40), run("running", 5, 25)]),
    { kind: "running", since: 25, runs: 2 });
  assert.deepEqual(chatStatus(session(null), false, false, [run("submitting", 30), run("starting", 15)]),
    { kind: "queued", since: 15, runs: 2 });
});

test("a busy chat whose turn isn't listed yet shows working without a timer", () => {
  assert.deepEqual(chatStatus(session(turn("completed")), true, false, []), { kind: "working", since: null });
});

test("a failed turn reads as a usage limit only when its error says so", () => {
  assert.deepEqual(chatStatus(session(turn("failed", { errorKind: "claude_usage_limit", error: "limit" })), false, false, []), { kind: "limited" });
  assert.deepEqual(chatStatus(session(turn("failed", { errorKind: "codex_terminal", error: "usageLimitExceeded: try later" })), false, false, []), { kind: "limited" });
  assert.deepEqual(chatStatus(session(turn("failed", { errorKind: "codex_terminal", error: "boom" })), false, false, []), { kind: "failed", error: "boom" });
});

test("finished, stopped and never-started chats are done, and only active states count as working", () => {
  for (const last of [turn("completed"), turn("interrupted"), null]) {
    const status = chatStatus(session(last), false, false, []);
    assert.deepEqual(status, { kind: "done", since: 500 });
    assert.equal(chatStatusWorking(status), false);
  }
  assert.equal(chatStatusWorking({ kind: "working", since: null }), true);
  assert.equal(chatStatusWorking({ kind: "input" }), false);
  assert.equal(chatStatusWorking({ kind: "limited" }), false);
});

test("a task the agent left running keeps the chat working until it finishes", () => {
  const waitingOnTask = { ...session(turn("completed")), background: { count: 2, since: 300 } };
  const status = chatStatus(waitingOnTask, false, false, []);
  assert.deepEqual(status, { kind: "background", since: 300, tasks: 2 });
  assert.equal(chatStatusWorking(status), true);
  assert.equal(chatStatus(waitingOnTask, true, false, []).kind, "working");
  assert.equal(chatStatus({ ...waitingOnTask, background: null }, false, false, []).kind, "done");
});
