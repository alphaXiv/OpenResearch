import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { Message } from "../src/components/ChatPanel.tsx";
import { m } from "../src/paraglide/messages.js";
import { autoDir } from "../src/i18n.ts";
import { render } from "./render.mjs";

const renderMessage = async (parts, id = "msg-1") => (await render(createElement(Message, {
  message: { id, role: "assistant", parts, createdAt: 0, completedAt: 1000 },
  activePermissionId: null,
  forkDisabled: false,
  branchDisabled: false,
  onFork: () => {},
  onSelectFork: () => {},
  onRecover: () => {},
}))).container;
const count = (container, text) => container.textContent.split(text).length - 1;

const detail = "API Error: Connection dropped (ECONNRESET)";
const bash = { id: "tool-1", type: "tool", tool: "Bash", state: { status: "completed", input: { command: "echo ready" } } };
const apiError = { id: "api-error-1", type: "tool", tool: "api_error", state: { status: "error", error: detail } };
const recovery = {
  id: "turn-recovery", type: "tool", tool: "error",
  state: { status: "error", error: detail, input: { turnId: "turn-1", errorKind: "claude_terminal", recoveryAction: "continue" } },
};

test("a full message presents one nonfatal transport status with details and recovery", async () => {
  const html = await renderMessage([bash, apiError, recovery]);
  assert.equal(count(html, m.chat_panel_temporary_api_error()), 1);
  assert.equal(count(html, "Connection dropped"), 1);
  assert.ok(html.textContent.includes(m.chat_continue()));
  assert.equal(count(html, m.chat_turn_incomplete()), 0);
});

test("a transport status stays visible outside a preceding collapsed tool group", async () => {
  const html = await renderMessage([bash, bash, apiError]);
  assert.equal(count(html, m.chat_panel_temporary_api_error()), 1);
  assert.equal(count(html, "Connection dropped"), 1);
  assert.equal(count(html, m.chat_turn_incomplete()), 0);
});

test("a nontransport failure headlines its own error", async () => {
  const html = await renderMessage([bash, { ...apiError, tool: "error" }, recovery]);
  assert.equal(count(html, m.chat_turn_incomplete()), 0);
  assert.equal(html.querySelector(".turn-failure p").textContent, detail);
  assert.equal(count(html, m.chat_panel_temporary_api_error()), 0);
});

test("unrelated errors are not absorbed into recovery", async () => {
  const html = await renderMessage([apiError, { ...recovery, state: { ...recovery.state, error: "Other failure" } }]);
  assert.equal(count(html, m.chat_panel_temporary_api_error()), 1);
  assert.deepEqual([...html.querySelectorAll(".turn-failure p")].map((p) => p.textContent), [m.chat_panel_temporary_api_error(), "Other failure"]);
});

test("local send uncertainty is visible in an alert", async () => {
  const warning = "Send not confirmed. Check the conversation before sending again: network";
  const html = await renderMessage([{ id: "p0", type: "tool", tool: "error", state: { status: "error", error: warning } }], "local-senderr-1");
  assert.ok(html.querySelector('[role="alert"]').textContent.includes("Send not confirmed"));
  assert.equal(html.querySelector("details")?.outerHTML, undefined);
});

const renderFailure = async (input, failure, error = "claude: Not logged in · Please run /login") => {
  const recovered = [];
  const container = (await render(createElement(Message, {
    message: { id: "msg-2", role: "assistant", parts: [{ id: "turn-recovery", type: "tool", tool: "error", state: { status: "error", error, input: { turnId: "turn-2", ...input } } }], createdAt: 0, completedAt: 1000 },
    activePermissionId: null,
    forkDisabled: false,
    branchDisabled: false,
    onFork: () => {},
    onSelectFork: () => {},
    onRecover: (turnId, action) => recovered.push([turnId, action]),
    failure,
  }))).container;
  const button = (label) => [...container.querySelectorAll("button")].find((element) => element.textContent === label);
  return { container, button, recovered };
};

const claudeAgent = { id: "claude-code", name: "Claude Code", authState: "needsLogin" };

test("a sign-in failure names the agent and signs in with one button that sends the turn again", async () => {
  const fixes = [];
  const failure = { agent: () => claudeAgent, fix: (cause, input) => cause === "sign_in" ? () => fixes.push([input.turnId, input.recoveryAction]) : undefined };
  const { container, button, recovered } = await renderFailure({ errorKind: "claude_signed_out", cause: "sign_in", recoveryAction: "retry" }, failure);
  assert.ok(container.textContent.includes(m.chat_failure_signed_out({ name: autoDir("Claude Code") })));
  assert.equal(count(container, m.chat_turn_incomplete()), 0);
  const signIn = button(m.harness_setup_login());
  assert.ok(signIn && !signIn.closest("details"), "the sign-in sits outside the error details");
  assert.ok(!button(m.app_retry()), "signing in sends the turn again by itself");
  signIn.click();
  assert.deepEqual(fixes, [["turn-2", "retry"]]);
  assert.deepEqual(recovered, []);
});

test("a sign-in failure names the agent it ran on, not the chat's current one", async () => {
  const codex = { id: "codex", name: "Codex", authState: "needsLogin" };
  const failure = { agent: (input) => input.harness === "codex" ? codex : claudeAgent, fix: () => undefined };
  const { container } = await renderFailure({ errorKind: "codex_terminal", cause: "sign_in", recoveryAction: "retry", harness: "codex" }, failure);
  assert.ok(container.textContent.includes(m.chat_failure_signed_out({ name: autoDir("Codex") })));
});

test("a sign-in failure waits for the composer's sign-in while its agent is signed out, and offers Retry once it is not", async () => {
  const signedOut = await renderFailure({ errorKind: "codex_terminal", cause: "sign_in", recoveryAction: "retry" }, { agent: () => claudeAgent, fix: () => undefined });
  assert.ok(!signedOut.button(m.harness_setup_login()));
  assert.ok(!signedOut.button(m.app_retry()));

  const signedInElsewhere = await renderFailure({ errorKind: "codex_terminal", cause: "sign_in", recoveryAction: "retry" }, { agent: () => ({ ...claudeAgent, authState: "ready" }), fix: () => undefined });
  const retry = signedInElsewhere.button(m.app_retry());
  assert.ok(retry, "an agent signed in some other way can send the turn again");
  retry.click();
  assert.deepEqual(signedInElsewhere.recovered, [["turn-2", "retry"]]);
});

test("an unrecognized failure headlines its error and keeps its recovery outside any details", async () => {
  for (const cause of [undefined, "new_failure_cause"]) {
    const { container, button, recovered } = await renderFailure({ errorKind: "adapter_error", cause, recoveryAction: "continue" }, undefined, "claude exited mid-turn");
    assert.equal(container.querySelector(".turn-failure p").textContent, "claude exited mid-turn");
    assert.equal(container.querySelector("details"), null);
    const resume = button(m.chat_continue());
    assert.ok(resume);
    resume.click();
    assert.deepEqual(recovered, [["turn-2", "continue"]]);
  }

  const { container } = await renderFailure({ errorKind: "adapter_error", recoveryAction: "continue" }, undefined, "claude exited mid-turn\nexit status 137");
  assert.equal(container.querySelector(".turn-failure p").textContent, "claude exited mid-turn");
  assert.ok(container.querySelector("details").textContent.includes("exit status 137"));
});

test("only the last turn in a row offers recovery", async () => {
  const failed = (id) => ({ id, role: "assistant", parts: [{ id: "turn-recovery", type: "tool", tool: "error", state: { status: "error", error: `${id} failed`, input: { turnId: id, errorKind: "adapter_error", recoveryAction: "continue" } } }], createdAt: 0, completedAt: 1000 });
  const responses = [failed("older"), failed("newer")];
  const recovered = [];
  const { container } = await render(createElement(Message, {
    message: responses[1],
    responseMessages: responses,
    activePermissionId: null,
    forkDisabled: false,
    branchDisabled: false,
    onFork: () => {},
    onSelectFork: () => {},
    onRecover: (turnId) => recovered.push(turnId),
  }));
  assert.ok(container.textContent.includes("older failed"));
  const resume = [...container.querySelectorAll("button")].filter((button) => button.textContent === m.chat_continue());
  assert.equal(resume.length, 1);
  resume[0].click();
  assert.deepEqual(recovered, ["newer"]);

  const compacted = { id: "compacted", role: "assistant", parts: [{ id: "c", type: "tool", tool: "compacted", state: { status: "completed" } }], createdAt: 1, completedAt: 1 };
  const afterCompaction = await render(createElement(Message, {
    message: compacted,
    responseMessages: [failed("only"), compacted],
    activePermissionId: null,
    forkDisabled: false,
    branchDisabled: false,
    onFork: () => {},
    onSelectFork: () => {},
    onRecover: () => {},
  }));
  assert.ok([...afterCompaction.container.querySelectorAll("button")].some((button) => button.textContent === m.chat_continue()), "a compaction takes no turn's place");

  const approval = { id: "approval", role: "assistant", parts: [{ id: "p", type: "prompt", prompt: { kind: "permission", id: "perm", title: "Run tests", options: [] }, state: { status: "completed" } }], createdAt: 2, completedAt: 2 };
  const afterApproval = await render(createElement(Message, {
    message: approval,
    responseMessages: [failed("only"), approval],
    activePermissionId: null,
    forkDisabled: false,
    branchDisabled: false,
    onFork: () => {},
    onSelectFork: () => {},
    onRecover: () => {},
  }));
  assert.ok([...afterApproval.container.querySelectorAll("button")].some((button) => button.textContent === m.chat_continue()), "a resolved approval takes no turn's place");
});

const openCodeFreeLimit = "Free usage limit reached. Connect a provider or subscribe to OpenCode Go to keep going, or switch models.";

test("a usage-limited OpenCode turn offers Try again outside the raw error", async () => {
  const { container, button, recovered } = await renderFailure(
    { errorKind: "opencode_terminal", cause: "usage_limit", recoveryAction: "continue" },
    undefined,
    openCodeFreeLimit,
  );
  assert.ok(container.textContent.includes(m.chat_session_limit_reached()));
  const tryAgain = button(m.chat_try_again());
  assert.ok(tryAgain && !tryAgain.closest("details"), "Try again sits outside the raw error");
  tryAgain.click();
  assert.deepEqual(recovered, [["turn-2", "continue"]]);
});

test("a usage limit recorded before the cause existed still reads as one and recovers", async () => {
  const { container, button, recovered } = await renderFailure(
    { errorKind: "opencode_terminal", recoveryAction: "continue" },
    undefined,
    openCodeFreeLimit,
  );
  assert.ok(container.textContent.includes(m.chat_session_limit_reached()));
  button(m.chat_try_again()).click();
  assert.deepEqual(recovered, [["turn-2", "continue"]]);
});

test("each cause the server names reads as itself", async () => {
  for (const [cause, label] of [
    ["usage_limit", m.chat_session_limit_reached()],
    ["credits", m.chat_failure_credits()],
    ["model_unavailable", m.chat_model_unavailable()],
    ["context_full", m.chat_failure_context_full()],
    ["overloaded", m.chat_failure_overloaded()],
    ["network", m.chat_panel_temporary_api_error()],
  ]) {
    const { container } = await renderFailure({ errorKind: "codex_terminal", cause, recoveryAction: "retry" }, undefined, "raw");
    assert.ok(container.textContent.includes(label), cause);
    assert.equal(count(container, m.chat_turn_incomplete()), 0, cause);
  }
});
