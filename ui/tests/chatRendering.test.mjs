import assert from "node:assert/strict";
import test from "node:test";
import {
  unreadAfterBusyChange,
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

test("completion marks only unseen existing chats unread and opening clears the dot", () => {
  const sessions = [{ id: "active" }, { id: "background" }];
  const busy = new Set(["active", "background", "deleted"]);
  const initial = new Set();
  assert.equal(unreadAfterBusyChange(initial, busy, busy, sessions, "active"), initial);
  const finished = unreadAfterBusyChange(initial, busy, new Set(), sessions, "active");
  assert.deepEqual([...finished], ["background"]);
  assert.deepEqual([...unreadAfterBusyChange(finished, new Set(), new Set(), sessions, "background")], []);
  assert.deepEqual([...unreadAfterBusyChange(initial, new Set(["active"]), new Set(), sessions, null)], ["active"]);
});

test("work collapses at an explicit final phase while its text is still streaming", async () => {
  const { splitTurnParts } = await import("../src/chatRendering.ts");
  const progress = { id: "progress", type: "text", text: "Reading…", phase: "commentary" };
  const tool = { id: "tool", type: "tool", state: { status: "completed" } };
  const final = { id: "final", type: "text", text: "", phase: "final_answer" };
  assert.deepEqual(splitTurnParts([progress, tool], true), { work: [], answer: [progress, tool] });
  assert.deepEqual(splitTurnParts([progress, tool, final], true), { work: [progress, tool], answer: [final] });
  assert.deepEqual(splitTurnParts([progress, tool, final], false), { work: [], answer: [progress, tool, final] });
});

test("legacy transcripts retain trailing answer and pending prompts remain exposed", async () => {
  const { splitTurnParts } = await import("../src/chatRendering.ts");
  const text = { id: "text", type: "text", text: "Progress" };
  const tool = { id: "tool", type: "tool" };
  const final = { id: "final", type: "text", text: "Done" };
  const parts = [text, tool, final];
  assert.deepEqual(splitTurnParts(parts, true), { work: [], answer: parts });
  assert.deepEqual(splitTurnParts(parts, false), { work: [text, tool], answer: [final] });
  const prompt = { id: "question", type: "prompt", prompt: { resolved: false } };
  const pending = [...parts, prompt];
  assert.deepEqual(splitTurnParts(pending, false), { work: [], answer: pending });
  assert.deepEqual(splitTurnParts([text, tool], false), { work: [], answer: [text, tool] });
});

test("reasoning-only work never creates an empty disclosure", async () => {
  const { splitTurnParts } = await import("../src/chatRendering.ts");
  const parts = [{ id: "thought", type: "reasoning", text: "Thinking" }, { id: "answer", type: "text", text: "Done", phase: "final_answer" }];
  assert.deepEqual(splitTurnParts(parts, false), { work: [], answer: parts });
});

test("Claude quota notices recognize typed errors and legacy duplicates without hiding real output", async () => {
  const { isClaudeUsageLimitPart } = await import("../src/chatRendering.ts");
  const text = "You've reached your Fable limit. Switch to another model, or manage usage credits at claude.ai/settings/usage?from=cc_cli_limit_message, to continue.";
  const sessionLimit = "You've hit your session limit · resets 3:10pm (America/Los_Angeles)";
  const duplicates = [
    { type: "text", text },
    { type: "text", text: sessionLimit },
    { type: "tool", tool: "error", state: { error: `claude: ${sessionLimit}` } },
    { type: "tool", tool: "error", state: { error: `claude: ${text}` } },
    { type: "tool", tool: "error", state: { input: { errorKind: "claude_usage_limit" } } },
  ];
  assert.ok(duplicates.every(isClaudeUsageLimitPart));
  assert.equal(isClaudeUsageLimitPart({ type: "text", text: "Earlier useful output" }), false);
  assert.equal(isClaudeUsageLimitPart({ type: "tool", tool: "bash", state: { error: text } }), false);
  assert.equal(isClaudeUsageLimitPart({ type: "tool", tool: "error", state: { error: "File not found" } }), false);
});
