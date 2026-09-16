import assert from "node:assert/strict";
import test from "node:test";
import { newPendingPrompts, runJustFinished, turnJustFinished } from "../src/notifications.ts";

const run = (id, status, cancelRequested = false) => ({ id, status, cancelRequested });
const prompt = (id, resolved = false) => ({ id, type: "prompt", prompt: { kind: "permission", resolved } });
const message = (id, ...parts) => ({ id, role: "assistant", parts, createdAt: 0 });

test("a run counts as finished only on a live to done or failed transition", () => {
  assert.equal(runJustFinished(run("a", "done")), false, "first sighting is the connect snapshot");
  assert.equal(runJustFinished(run("b", "starting")), false);
  assert.equal(runJustFinished(run("b", "running")), false);
  assert.equal(runJustFinished(run("b", "done")), true);
  assert.equal(runJustFinished(run("b", "done")), false, "an unchanged status never repeats");
  assert.equal(runJustFinished(run("c", "running")), false);
  assert.equal(runJustFinished(run("c", "failed")), true);
});

test("a cancelled run never counts as finished", () => {
  assert.equal(runJustFinished(run("d", "running")), false);
  assert.equal(runJustFinished(run("d", "cancelled", true)), false);
  assert.equal(runJustFinished(run("e", "running", true)), false);
  assert.equal(runJustFinished(run("e", "failed", true)), false);
});

test("pending prompts are reported once and skip resolved cards and other parts", () => {
  const text = { id: "t", type: "text", text: "hello" };
  assert.deepEqual(newPendingPrompts("s1", message("m1", text, prompt("p1", true))), []);
  assert.deepEqual(newPendingPrompts("s1", message("m1", text, prompt("p2"))).map((part) => part.id), ["p2"]);
  assert.deepEqual(newPendingPrompts("s1", message("m1", text, prompt("p2"))), [], "streaming re-sends the same card");
  assert.deepEqual(newPendingPrompts("s1", message("m2", prompt("p2"))).map((part) => part.id), ["p2"], "keyed per message");
});

test("a turn counts as finished only when a busy session goes idle", () => {
  assert.equal(turnJustFinished("t1", false), false, "never seen busy");
  assert.equal(turnJustFinished("t1", true), false);
  assert.equal(turnJustFinished("t1", false), true);
  assert.equal(turnJustFinished("t1", false), false, "already idle");
});

test("a turn that ends on an unanswered card is left to the prompt notification", () => {
  turnJustFinished("t2", true);
  newPendingPrompts("t2", message("m3", prompt("p3")));
  assert.equal(turnJustFinished("t2", false), false);
  newPendingPrompts("t2", message("m3", prompt("p3", true)));
  turnJustFinished("t2", true);
  assert.equal(turnJustFinished("t2", false), true, "answered cards no longer hold it back");
});
