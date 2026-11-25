import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, composer, renderChat, session, userMessage } from "./chat.mjs";

const compactMarker = {
  id: "compact", role: "assistant", createdAt: 4, completedAt: null,
  parts: [{ id: "compacted", type: "tool", tool: "compacted", state: { status: "running" } }],
};

/** A chat holding `usedTokens` whose agent reported a five-minute cache, last replying `repliedAt`. */
async function chat({ cacheTtlSecs = 300, repliedAt = 3, usedTokens = 200_000 } = {}) {
  const reply = { ...assistantMessage("a1", "Hi.", "u1"), completedAt: repliedAt };
  const view = await renderChat("/projects/p/tasks/s", {
    sessions: [session("s", { contextUsage: { usedTokens, contextWindow: 1_000_000, cacheTtlSecs } })],
    transcripts: { s: [userMessage("u1", "hello"), reply] },
    routes: {
      "POST /api/chat/sessions/s/compact": () => ({ message: compactMarker }),
      "POST /api/chat/sessions/s/message": () => ({ ok: true, turn: { turnId: "t", queued: true } }),
    },
  });
  const posts = () => view.requests.map(({ request }) => request).filter((request) => /\/(compact|message)$/.test(request));
  return { posts };
}

const toggle = () => document.querySelector('[data-onboarding="composer"] button[role="switch"]');

test("an expired cache compacts the chat before the message goes out", async () => {
  const { posts } = await chat();
  assert.equal(toggle()?.textContent, m.chat_auto_compact());
  const input = composer();
  await input.type("next step");
  await input.send();
  assert.deepEqual(posts(), ["POST /api/chat/sessions/s/compact", "POST /api/chat/sessions/s/message"]);
});

test("switching to full context sends without compacting", async () => {
  const { posts } = await chat();
  await act(async () => toggle().click());
  assert.equal(toggle().textContent, m.chat_full_context());
  assert.equal(toggle().getAttribute("aria-checked"), "false");
  const input = composer();
  await input.type("next step");
  await input.send();
  assert.deepEqual(posts(), ["POST /api/chat/sessions/s/message"]);
});

test("a warm cache offers no compaction", async () => {
  await chat({ repliedAt: Date.now() });
  assert.equal(toggle(), null);
});

test("an agent that reports no cache lifetime offers no compaction", async () => {
  await chat({ cacheTtlSecs: null });
  assert.equal(toggle(), null);
});

test("a chat under 200K tokens offers no compaction", async () => {
  await chat({ usedTokens: 199_999 });
  assert.equal(toggle(), null);
});
