import assert from "node:assert/strict";
import test from "node:test";
import {
  unreadAfterBusyChange,
  partIsVisible,
  pendingQuestionId,
  partsTailToolId,
  streamTailIsText,
  streamTailTool,
  lastResponseText,
  transcriptFileName,
  transcriptMarkdown,
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
  const { isUsageLimitPart } = await import("../src/chatRendering.ts");
  const text = "You've reached your Fable limit. Switch to another model, or manage usage credits at claude.ai/settings/usage?from=cc_cli_limit_message, to continue.";
  const sessionLimit = "You've hit your session limit · resets 3:10pm (America/Los_Angeles)";
  const duplicates = [
    { type: "text", text },
    { type: "text", text: sessionLimit },
    { type: "tool", tool: "error", state: { error: `claude: ${sessionLimit}` } },
    { type: "tool", tool: "error", state: { error: `claude: ${text}` } },
    { type: "tool", tool: "error", state: { input: { errorKind: "claude_usage_limit" } } },
  ];
  assert.ok(duplicates.every(isUsageLimitPart));
  assert.equal(isUsageLimitPart({ type: "text", text: "Earlier useful output" }), false);
  assert.equal(isUsageLimitPart({ type: "tool", tool: "bash", state: { error: text } }), false);
  assert.equal(isUsageLimitPart({ type: "tool", tool: "error", state: { error: "File not found" } }), false);
});


test("Cursor model restrictions use the limit disclosure without treating them as session quotas", async () => {
  const { isUsageLimitPart, isModelAccessLimitPart } = await import("../src/chatRendering.ts");
  const error = "Named models unavailable Free plans can only use Auto. Switch to Auto or upgrade plans to continue.";
  for (const text of [error, `ActionRequiredError: ${error}`]) {
    const part = { id: "turn-recovery", type: "tool", tool: "error", state: { error: text } };
    assert.equal(isUsageLimitPart(part), true);
    assert.equal(isModelAccessLimitPart(part), true);
    assert.equal(isUsageLimitPart({ type: "tool", tool: "bash", state: { error: text } }), false);
    assert.equal(isUsageLimitPart({ type: "text", text }), false);
  }
  assert.equal(isModelAccessLimitPart({ type: "tool", tool: "error", state: { error: "Rate limit reached" } }), false);
});

test("Codex and OpenCode terminal limits use the shared disclosure without classifying ordinary tool failures", async () => {
  const { isUsageLimitPart } = await import("../src/chatRendering.ts");
  for (const error of [
    "You've hit your usage limit. Try again later.",
    'Limit reached\n\ncodexErrorInfo: "usageLimitExceeded"',
    "You exceeded your current quota, please check your plan and billing details.",
    "Rate limit reached for model. Please try again later.",
    "Insufficient credits",
  ]) {
    assert.equal(isUsageLimitPart({ type: "tool", tool: "error", state: { error } }), true);
    assert.equal(isUsageLimitPart({ type: "tool", tool: "bash", state: { error } }), false);
    assert.equal(isUsageLimitPart({ type: "text", text: error }), false);
  }
  for (const error of ["Invalid API key", "Context length exceeded", "Connection refused"]) {
    assert.equal(isUsageLimitPart({ type: "tool", tool: "error", state: { error } }), false);
  }
});

test("live OpenCode questions route composer text to the existing prompt", () => {
  const question = { id: "question", type: "prompt", prompt: { kind: "question", nativeId: '["frm_test","answer"]', resolved: false } };
  for (const harness of ["opencode", "claude-code", "codex"]) {
    assert.equal(pendingQuestionId([message(question)], harness, true), "question");
    assert.equal(pendingQuestionId([message(question)], harness, false), null);
  }
  assert.equal(pendingQuestionId([message({ ...question, prompt: { ...question.prompt, resolved: true } })], "opencode", true), null);
  assert.equal(pendingQuestionId([message(question)], "cursor", true), null);
});

const user = (id, text) => ({ id, role: "user", createdAt: 0, parts: [{ id: `${id}-t`, type: "text", text }] });
const assistant = (id, parts) => ({ id, role: "assistant", createdAt: 0, parts });

test("copy takes the latest answer, skipping commentary and tool work", () => {
  const messages = [
    user("u1", "first"),
    assistant("a1", [{ id: "t1", type: "text", text: "old answer", phase: "final_answer" }]),
    user("u2", "second"),
    assistant("a2", [
      { id: "c", type: "text", text: "checking files", phase: "commentary" },
      { id: "tool", type: "tool", tool: "read" },
      { id: "f", type: "text", text: "new answer", phase: "final_answer" },
    ]),
  ];
  assert.equal(lastResponseText(messages), "new answer");
});

test("copy falls back to an earlier turn when the latest has no answer", () => {
  const messages = [
    assistant("a1", [{ id: "t1", type: "text", text: "kept", phase: "final_answer" }]),
    assistant("a2", [{ id: "tool", type: "tool", tool: "bash" }]),
  ];
  assert.equal(lastResponseText(messages), "kept");
  assert.equal(lastResponseText([user("u", "only a question")]), "");
});

test("export keeps commentary that copy leaves out", () => {
  const messages = [
    user("u1", "run it"),
    assistant("a1", [
      { id: "c", type: "text", text: "checking files", phase: "commentary" },
      { id: "f", type: "text", text: "done", phase: "final_answer" },
    ]),
  ];
  assert.equal(lastResponseText(messages), "done");
  assert.match(
    transcriptMarkdown("T", messages, { user: "You", assistant: "Codex" }),
    /checking files\n\ndone/,
  );
});

test("export writes each speaker's text under a heading", () => {
  const markdown = transcriptMarkdown(
    "Sweep",
    [
      user("u1", "run it"),
      assistant("a1", [
        { id: "r", type: "reasoning", text: "hidden" },
        { id: "t", type: "text", text: "done" },
      ]),
    ],
    { user: "You", assistant: "Codex" },
  );
  assert.equal(markdown, "# Sweep\n\n## You\n\nrun it\n\n## Codex\n\ndone\n");
  assert.equal(transcriptMarkdown("Empty", [assistant("a", [{ id: "x", type: "tool" }])], { user: "You", assistant: "Codex" }), null);
});

test("export file names are safe slugs", () => {
  assert.equal(transcriptFileName("LR sweep: v2 / final?"), "lr-sweep-v2-final.md");
  assert.equal(transcriptFileName("???"), "chat.md");
  // Trimmed after the cut, so a title severed at a separator keeps a clean name.
  assert.equal(transcriptFileName(`${"a".repeat(79)} tail`), `${"a".repeat(79)}.md`);
});
