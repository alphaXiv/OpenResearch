import assert from "node:assert/strict";
import test from "node:test";
import {
  partIsVisible,
  partsTailToolId,
  streamTailIsText,
  streamTailTool,
} from "../src/chatRendering.ts";

const message = (...parts) => ({ id: "assistant", role: "assistant", parts, createdAt: 0 });

test("invisible transcript parts do not displace a visible tool tail", () => {
  const tool = { id: "tool", type: "tool", state: { status: "completed" } };

  assert.equal(partIsVisible({ id: "reasoning", type: "reasoning" }), false);
  assert.equal(partIsVisible({ id: "interrupted", type: "tool", tool: "interrupted" }), false);
  assert.equal(partsTailToolId([tool, { id: "reasoning", type: "reasoning" }]), "tool");
  assert.deepEqual(streamTailTool([message(tool)]), { messageId: "assistant", toolId: "tool" });
});

test("errored tools and visible text end the tool tail", () => {
  const error = { id: "error", type: "tool", state: { status: "error" } };
  const text = { id: "text", type: "text", text: "answer" };

  assert.equal(partsTailToolId([error]), null);
  assert.equal(partsTailToolId([{ id: "tool", type: "tool" }, text]), null);
  assert.equal(streamTailIsText([message(text)]), true);
});

test("only the selected unresolved permission is visible when one is active", () => {
  const permission = { id: "permission", type: "prompt", prompt: { kind: "permission", resolved: false } };

  assert.equal(partIsVisible(permission), true);
  assert.equal(partIsVisible(permission, "permission"), true);
  assert.equal(partIsVisible(permission, "other"), false);
  assert.equal(partsTailToolId([{ id: "tool", type: "tool" }, permission]), null);
});

test("thinking replaces a text tail while steer and status parts do not", () => {
  const text = { id: "text", type: "text", text: "answer" };

  assert.equal(streamTailIsText([message(text, { id: "reasoning", type: "reasoning" })]), false);
  assert.equal(streamTailIsText([message(text, { id: "steer", type: "steer" })]), true);
  assert.equal(streamTailIsText([message(text, { id: "turn-retry", type: "tool" })]), true);
});
