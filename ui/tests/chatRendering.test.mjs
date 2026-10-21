import assert from "node:assert/strict";
import test from "node:test";
import {
  unreadAfterBusyChange,
  isParentUpdate,
  parentUpdateText,
  partIsVisible,
  pendingQuestionId,
  partsTailToolId,
  streamTailIsText,
  streamTailTool,
  lastResponseText,
  transcriptFileName,
  transcriptMarkdown,
  withoutDuplicateTurnError,
} from "../src/chatRendering.ts";

const message = (...parts) => ({ id: "assistant", role: "assistant", parts, createdAt: 0 });

test("a recovery error replaces the identical terminal error without hiding other errors", () => {
  const duplicate = { id: "err-0", type: "tool", tool: "error", state: { error: "Invalid model" } };
  const other = { id: "err-1", type: "tool", tool: "error", state: { error: "Other failure" } };
  const recovery = { id: "turn-recovery", type: "tool", tool: "error", state: { error: "Invalid model" } };
  assert.deepEqual(withoutDuplicateTurnError([other, duplicate], recovery), [other]);
  assert.deepEqual(withoutDuplicateTurnError([duplicate, other], recovery), [duplicate, other]);
  assert.deepEqual(withoutDuplicateTurnError([duplicate], undefined), [duplicate]);
});

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

test("Z.ai billing rejection is model access unavailable rather than a session quota", async () => {
  const { isModelAccessLimitPart } = await import("../src/chatRendering.ts");
  assert.equal(isModelAccessLimitPart({ type: "tool", tool: "error", state: { error: "ZCode turn failed: 1113 Insufficient balance or no resource package" } }), true);
  assert.equal(isModelAccessLimitPart({ type: "tool", tool: "error", state: { error: "ZCode turn failed: CONFIGURATION_ERROR" } }), false);
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

test("pending input outlives async turns but held prompts require a live turn", async () => {
  const { hasPendingInput } = await import("../src/chatRendering.ts");
  const question = { id: "q", type: "prompt", prompt: { kind: "question", resolved: false } };
  assert.equal(hasPendingInput([message(question)], false), true);
  assert.equal(hasPendingInput([message(question)], true), true);
  const held = { ...question, prompt: { ...question.prompt, nativeId: "rpc-1" } };
  assert.equal(hasPendingInput([message(held)], false), false);
  assert.equal(hasPendingInput([message(held)], true), true);
  assert.equal(hasPendingInput([message({ ...question, prompt: { ...question.prompt, resolved: true } })], true), false);
  assert.equal(hasPendingInput([message({ id: "plan", type: "prompt", prompt: { kind: "plan", resolved: false } })], false), false);
});

test("switching conversation branches clears hidden pending questions", async () => {
  const { activePath } = await import("../src/transcriptTree.ts");
  const { hasPendingInput } = await import("../src/chatRendering.ts");
  const messages = [
    { id: "u", role: "user", parts: [], parentId: null },
    { id: "old", role: "assistant", parentId: "u", parts: [{ id: "q", type: "prompt", prompt: { kind: "question", resolved: false } }] },
    { id: "retry", role: "assistant", parentId: "u", parts: [{ id: "text", type: "text", text: "Done" }] },
  ];
  assert.equal(hasPendingInput(activePath(messages, "old"), false), true);
  assert.equal(hasPendingInput(activePath(messages, "retry"), false), false);
  assert.equal(hasPendingInput(activePath(messages, "old"), false), true);
});

test("automatic continuations share a response until the next user message", async () => {
  const { groupResponseMessages } = await import("../src/chatRendering.ts");
  const user = { id: "user", role: "user", parts: [], createdAt: 0 };
  const first = { ...message(), id: "first" };
  const continuation = { ...message(), id: "continuation" };
  const nextUser = { ...user, id: "next-user" };
  const groups = groupResponseMessages([user, first, continuation, nextUser]);
  assert.deepEqual(groups, [[user], [first, continuation], [nextUser]]);
  assert.equal(groupResponseMessages([user, first, continuation, nextUser], groups)[1], groups[1]);
});

test("a continuation retains the previous answer until a new answer arrives", async () => {
  const { splitResponseParts } = await import("../src/chatRendering.ts");
  const oldAnswer = { id: "old-answer", type: "text", phase: "final_answer", text: "Run launched" };
  const progress = { id: "progress", type: "text", phase: "commentary", text: "Checking the result" };
  const newAnswer = { id: "new-answer", type: "text", phase: "final_answer", text: "Run repaired" };
  const first = { ...message(oldAnswer), id: "first", completedAt: 10 };
  const continuation = { ...message(progress), id: "continuation", createdAt: 100 };
  const waiting = splitResponseParts([first, continuation], true);
  assert.deepEqual(waiting.answer, [oldAnswer]);
  assert.deepEqual(waiting.work, [{ id: "continuation", parts: [progress] }]);
  const updated = splitResponseParts([first, { ...continuation, parts: [progress, newAnswer] }], true);
  assert.deepEqual(updated.answer, [newAnswer]);
  assert.deepEqual(updated.work, [
    { id: "first", parts: [oldAnswer] },
    { id: "continuation", parts: [progress] },
  ]);
});

test("continuation prompts stay outside collapsed activity", async () => {
  const { splitResponseParts } = await import("../src/chatRendering.ts");
  const first = message({ id: "answer", type: "text", phase: "final_answer", text: "Started" });
  const prompt = { id: "question", type: "prompt", prompt: { kind: "question", resolved: false } };
  assert.deepEqual(splitResponseParts([first, { ...message(prompt), id: "next" }], true).answer, [prompt]);
});

test("every event for an agent resolves to its combined live transcript", async () => {
  const { agentTranscript } = await import("../src/chatRendering.ts");
  const event = (id, threads, children = [], status = "completed") => ({
    id, type: "tool", tool: "subagent", children,
    state: { status, input: { agentThreadIds: threads } },
  });
  const first = { id: "child:first", type: "text", text: "First update" };
  const next = { id: "child:next", type: "text", text: "Still working" };
  const other = { id: "other:text", type: "text", text: "Different agent" };
  const spawn = event("spawn", ["child"], [first]);
  const report = event("report", ["child"]);
  const continuation = event("continuation", ["child", "other"], [next, other], "running");
  const messages = [message(spawn, report, continuation)];
  for (const id of ["spawn", "report"]) {
    const transcript = agentTranscript(messages, id);
    assert.deepEqual(transcript.children, [first, next]);
    assert.equal(transcript.state.status, "running");
  }
  assert.equal(agentTranscript(messages, "missing"), null);
});

test("an agent transcript can open before content arrives and then receive updates", async () => {
  const { agentTranscript } = await import("../src/chatRendering.ts");
  const spawn = { id: "spawn", type: "tool", tool: "subagent", children: [],
    state: { status: "running", input: { agentThreadIds: ["child"] } } };
  assert.deepEqual(agentTranscript([message(spawn)], "spawn").children, []);
  const update = { id: "child:update", type: "text", text: "Working" };
  assert.deepEqual(agentTranscript([message({ ...spawn, children: [update] })], "spawn").children, [update]);
  const legacy = { ...spawn, state: { status: "running" }, children: [update] };
  assert.equal(agentTranscript([message(legacy)], "spawn"), legacy);
});

test("nested report-back markers retain their own update rather than reopening the agent", async () => {
  const { agentTranscript } = await import("../src/chatRendering.ts");
  const markers = ["one", "two", "three", "four"].map((id) => ({
    id: `audit:${id}`, type: "tool", tool: "subagent",
    state: { status: "completed", input: { kind: "interacted" } },
  }));
  const audit = { id: "audit-spawn", type: "tool", tool: "subagent",
    state: { status: "completed", input: { nickname: "audit" } },
    children: [{ id: "audit:read", type: "tool", tool: "read" }, ...markers],
  };
  for (const marker of markers) {
    assert.equal(agentTranscript([message(audit)], marker.id), marker);
  }
  const linkedMarker = { ...markers[0], state: { ...markers[0].state,
    input: { kind: "interacted", agentThreadIds: ["parent"] } } };
  const linkedAudit = { ...audit, children: [linkedMarker] };
  assert.equal(agentTranscript([message(linkedAudit)], linkedMarker.id), linkedMarker);
});


test("outgoing parent updates are distinct from transcript-owning agent events", async () => {
  const { isParentUpdate } = await import("../src/chatRendering.ts");
  const update = { id: "child:report", type: "tool", tool: "subagent",
    state: { input: { kind: "interacted" } } };
  assert.equal(isParentUpdate(update), true);
  assert.equal(isParentUpdate({ ...update, id: "agent-spawn" }), false);
  assert.equal(isParentUpdate({ ...update, children: [{ id: "child:message", type: "text", text: "Result" }] }), false);
});

test("Claude handbacks show their actual update and empty Codex markers remain static", () => {
  const handback = { id: "spawn:handback", type: "tool", tool: "SubagentHandback", state: { input: { message: "HELPER_DONE" } } };
  assert.equal(isParentUpdate(handback), true);
  assert.equal(parentUpdateText(handback), "HELPER_DONE");
  assert.equal(parentUpdateText({ id: "child:update", tool: "subagent", state: { input: { kind: "interacted" } } }), null);
});

test("a wakeup continuation retains each earlier turn's recovery controls", async () => {
  const { groupResponseMessages, splitResponseTurn, splitResponseParts } = await import("../src/chatRendering.ts");
  const recovery = { id: "turn-recovery", type: "tool", tool: "error", state: {
    status: "error", error: "Disconnected", input: { turnId: "failed-turn", recoveryAction: "continue" },
  } };
  const failed = { ...message({ id: "answer", type: "text", text: "Experiment launched" }, recovery), id: "failed" };
  const wakeup = { ...message({ id: "wakeup-answer", type: "text", text: "Experiment finished" }), id: "wakeup" };
  const turns = groupResponseMessages([failed, wakeup])[0].map(splitResponseTurn);
  assert.equal(turns[0].turnStatus, recovery);
  assert.equal(turns[0].turnStatus.state.input.turnId, "failed-turn");
  assert.equal(turns[1].turnStatus, undefined);
  const response = splitResponseParts(turns.map(turn => turn.message), false);
  assert.deepEqual(response.answer, wakeup.parts);
  assert.ok(response.work.every(turn => !turn.parts.includes(recovery)));
});
