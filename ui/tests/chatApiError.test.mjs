import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { Message } from "../src/components/ChatPanel.tsx";
import { m } from "../src/paraglide/messages.js";
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

test("nontransport failures retain the generic recovery label", async () => {
  const html = await renderMessage([bash, { ...apiError, tool: "error" }, recovery]);
  assert.equal(count(html, m.chat_turn_incomplete()), 1);
  assert.equal(count(html, m.chat_panel_temporary_api_error()), 0);
});

test("unrelated errors are not absorbed into recovery", async () => {
  const html = await renderMessage([apiError, { ...recovery, state: { ...recovery.state, error: "Other failure" } }]);
  assert.equal(count(html, m.chat_panel_temporary_api_error()), 1);
  assert.equal(count(html, m.chat_turn_incomplete()), 1);
  assert.ok(html.textContent.includes("Other failure"));
});

test("local send uncertainty is visible in an alert", async () => {
  const warning = "Send not confirmed. Check the conversation before sending again: network";
  const html = await renderMessage([{ id: "p0", type: "tool", tool: "error", state: { status: "error", error: warning } }], "local-senderr-1");
  assert.ok(html.querySelector('[role="alert"]').textContent.includes("Send not confirmed"));
  assert.equal(html.querySelector("details")?.outerHTML, undefined);
});
