import assert from "node:assert/strict";
import test from "node:test";
import { inputSummaryChanged } from "../src/queries/chatState.ts";

test("branch changes and prompt transitions refresh summaries without refreshing on text tokens", () => {
  const states = new Map();
  const event = (parts) => ({ type: "message", sessionId: "other-project-chat", message: { id: "reply", parts } });
  const text = { id: "text", type: "text", text: "Working" };
  const prompt = { id: "question", type: "prompt", prompt: { kind: "question", resolved: false } };
  assert.equal(inputSummaryChanged(event([text]), states), false);
  assert.equal(inputSummaryChanged(event([text, prompt]), states), true);
  assert.equal(inputSummaryChanged(event([{ ...text, text: "Still working" }, prompt]), states), false);
  assert.equal(inputSummaryChanged(event([text, { ...prompt, prompt: { ...prompt.prompt, resolved: true } }]), states), true);
  assert.equal(inputSummaryChanged(event([text]), states), true);
  assert.equal(inputSummaryChanged(event([text]), states), false);
  assert.equal(inputSummaryChanged({ type: "branch", sessionId: "other-project-chat", activeLeafId: "alternate" }, states), true);
  assert.equal(inputSummaryChanged({ type: "sessionDeleted", sessionId: "other-project-chat" }, states), true);
  assert.equal(inputSummaryChanged({ type: "busy", sessionId: "other-project-chat", busy: true }, states), false);
});
