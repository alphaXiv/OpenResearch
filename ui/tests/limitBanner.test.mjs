import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { LimitBanner } from "../src/components/LimitBanner.tsx";
import { fmtClock } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { render, serve, settle } from "./render.mjs";

const hour = 3_600_000;

function banner(props) {
  return createElement(LimitBanner, { agent: "Codex", resetAt: null, stopped: false, queued: 0, ...props });
}

/** Records each timer request the banner makes. */
function serveTimer() {
  const sent = [];
  serve((path, init) => {
    if (path === "/api/chat/sessions/chat-1/limit-timer" && init.method === "POST") {
      sent.push(JSON.parse(init.body));
      return { ok: true };
    }
  });
  return sent;
}

const button = (container, label) => [...container.querySelectorAll("button")].find((b) => b.textContent === label);

test("an account out of usage says when it resets and lets the chat stop its timer", async () => {
  const sent = serveTimer();
  const resetAt = Date.now() + 2 * hour;
  const { container } = await render(banner({ resetAt, sessionId: "chat-1", queued: 1 }));
  assert.ok(container.textContent.includes(m.chat_limit_title({ agent: "Codex" })));
  assert.ok(container.textContent.includes(m.chat_limit_continues({ time: fmtClock(resetAt) })));
  await act(async () => button(container, m.chat_limit_stop_timer()).click());
  await settle();
  assert.deepEqual(sent, [{ stopped: true }]);
});

test("a stopped timer can start again, and sends once usage is back", async () => {
  const sent = serveTimer();
  const resetAt = Date.now() + hour;
  const view = await render(banner({ resetAt, sessionId: "chat-1", stopped: true, queued: 1 }));
  assert.ok(view.container.textContent.includes(m.chat_limit_waits({ time: fmtClock(resetAt) })));
  await act(async () => button(view.container, m.chat_limit_start_timer()).click());
  await settle();

  await view.rerender(banner({ resetAt: null, sessionId: "chat-1", stopped: true, queued: 2 }));
  assert.ok(view.container.textContent.includes(m.chat_limit_back_title({ agent: "Codex" })));
  await act(async () => button(view.container, m.chat_limit_send_now()).click());
  await settle();
  assert.deepEqual(sent, [{ stopped: false }, { stopped: false }]);
});

test("nothing shows once the account has usage and the chat holds nothing", async () => {
  const { container } = await render(banner({ resetAt: null, sessionId: "chat-1", stopped: true, queued: 0 }));
  assert.equal(container.textContent, "");
});

test("a new chat on an account out of usage is told its messages wait, with a reset to spend", async () => {
  const resetAt = Date.now() + hour;
  const plan = { harness: "codex", plan: "Pro", account: null, windows: [], readAt: null, resets: { available: 1, credits: null } };
  const { container } = await render(banner({ resetAt, plan }));
  assert.ok(container.textContent.includes(m.chat_limit_new_chat({ time: fmtClock(resetAt) })));
  assert.ok(button(container, m.usage_reset_confirm()), "a Codex reset is offered");
  assert.equal(button(container, m.chat_limit_stop_timer()), undefined, "a chat not yet started has no timer");
});
