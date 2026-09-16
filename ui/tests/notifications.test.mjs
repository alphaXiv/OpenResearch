import assert from "node:assert/strict";
import test from "node:test";
import { newPendingPrompts, runJustFinished } from "../src/notifications.ts";

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
  assert.deepEqual(newPendingPrompts(message("m1", text, prompt("p1", true))), []);
  assert.deepEqual(newPendingPrompts(message("m1", text, prompt("p2"))).map((part) => part.id), ["p2"]);
  assert.deepEqual(newPendingPrompts(message("m1", text, prompt("p2"))), [], "streaming re-sends the same card");
  assert.deepEqual(newPendingPrompts(message("m2", prompt("p2"))).map((part) => part.id), ["p2"], "keyed per message");
});
