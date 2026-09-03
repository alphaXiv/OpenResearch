import assert from "node:assert/strict";
import test from "node:test";
import { stepLabel, turnOutline } from "../src/turnOutline.ts";

const text = (id, body) => ({ id, type: "text", text: body });
const tool = (id, name = "Bash") => ({ id, type: "tool", tool: name, state: { status: "completed", input: {} } });

test("step labels take the first sentence without markdown or tags", () => {
  assert.equal(stepLabel("I'll **read** `loader.py` first, then the tests. Then edit it."), "I'll read loader.py first, then the tests.");
  assert.equal(stepLabel("## Plan\nLooking at <file path=\"src/a.py\" /> now"), "Plan Looking at src/a.py now");
  assert.equal(stepLabel("x".repeat(200)).length, 110);
  assert.equal(stepLabel("   "), "");
  assert.equal(stepLabel("1. Read the config e.g. loader.py first. Then edit."), "Read the config e.g. loader.py first.");
  assert.equal(stepLabel("Clean tree. Now let's write the module."), "Clean tree. Now let's write the module.");
  assert.equal(stepLabel("Running:\n```bash\npytest -q\n```\nThen report."), "Running: Then report.");
});

test("narration opens a step and following tools attach to it", () => {
  const parts = [
    { id: "r", type: "reasoning", text: "hidden" },
    text("t1", "I'll survey the repo first."),
    tool("b1"),
    tool("todo", "TodoWrite"),
    tool("b2", "Read"),
    text("t2", "Now the tests."),
    tool("b3"),
    { id: "turn-retry", type: "tool", tool: "retry", state: { status: "running", input: {} } },
  ];
  const steps = turnOutline(parts);
  assert.deepEqual(steps.map((step) => [step.id, step.label, step.toolParts.map((p) => p.id), step.done]), [
    ["t1", "I'll survey the repo first.", ["b1", "b2"], true],
    ["t2", "Now the tests.", ["b3"], false],
  ]);
});

test("tools before any narration form an unlabeled first step", () => {
  const steps = turnOutline([tool("b1"), text("t1", "Done.")]);
  assert.deepEqual(steps.map((step) => [step.id, step.label, step.toolParts.length, step.done]), [["b1", "", 1, true], ["t1", "Done.", 0, false]]);
  assert.deepEqual(turnOutline([]), []);
});
