import assert from "node:assert/strict";
import { once } from "node:events";
import test from "node:test";
import { act } from "react";
import { autoDir } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, codex, renderChat, session, userMessage } from "./chat.mjs";
import { clients, find, settle, type } from "./render.mjs";

/** The page's sockets; a test plays the server's side on them. */
const sockets = [];
globalThis.WebSocket = class extends EventTarget {
  static OPEN = 1;
  readyState = 1;
  constructor(url) {
    super();
    this.url = String(url);
    sockets.push(this);
  }
  send(data) {
    if (data instanceof Uint8Array) {
      this.dispatchEvent(new MessageEvent("terminal-input", { data: new TextDecoder().decode(data) }));
    }
  }
  close() { this.readyState = 3; }
};

const signedOutClaude = {
  ...codex, id: "claude-code", name: "Claude Code", authenticated: false, authState: "needsLogin", loginEligible: true,
  agentReady: false, models: [], defaultModel: undefined, hasChats: true,
};
const commands = { "GET /api/harnesses/setup/commands": () => ({ "claude-code": { install: "install", login: "claude auth login" } }) };
const click = async (element) => {
  await act(async () => element.click());
  await settle();
};
const lastLogin = () => new URL(sockets.at(-1).url, location.href);

const openPicker = async (claude) => {
  await renderChat("/projects/p/tasks/s", { harnesses: [codex, claude], routes: commands });
  await click(document.querySelector('[data-onboarding="model-picker"] button'));
};

test("the model picker keeps an agent research ran on while it is signed out, one click from signing in", async () => {
  await openPicker(signedOutClaude);
  assert.ok(find(".model-menu", m.settings_page_not_signed_in()));
  await click(find(".model-menu button", m.harness_setup_login()));
  assert.equal(lastLogin().searchParams.get("harness"), "claude-code");
  assert.equal(lastLogin().searchParams.get("action"), "login");
});

test("a collapsed sign-in terminal answers the console's cursor query", async () => {
  await openPicker(signedOutClaude);
  await click(find(".model-menu button", m.harness_setup_login()));

  const socket = sockets.at(-1);
  const response = once(socket, "terminal-input", { signal: AbortSignal.timeout(3000) });
  const query = new TextEncoder().encode("\x1b[6n").buffer;
  await act(async () => socket.dispatchEvent(new MessageEvent("message", { data: query })));

  const [input] = await response;
  assert.match(input.data, /^\x1b\[\d+;\d+R$/);
});

test("the model picker leaves out a signed-out agent no research ran on", async () => {
  await openPicker({ ...signedOutClaude, hasChats: false });
  assert.ok(document.querySelector(".model-menu"));
  assert.ok(!find(".model-menu button", m.harness_setup_login()));
});

test("a saved sign-out offers one sign-in from the composer and sends the turn again", async () => {
  const harnesses = [codex, signedOutClaude];
  const recovery = {
    id: "turn-recovery", type: "tool", tool: "error",
    state: { status: "error", error: "Sign in with `claude auth login`, then re-check this agent.", input: { turnId: "t1", errorKind: "claude_signed_out", cause: "sign_in", recoveryAction: "retry" } },
  };
  const chat = await renderChat("/projects/p/tasks/s", {
    sessions: [session("s", { harness: "claude-code", model: "opus" })],
    transcripts: { s: [userMessage("u1", "Profile the loader."), assistantMessage("a1", [recovery], "u1")] },
    harnesses,
    routes: { ...commands, "POST /api/chat/sessions/s/turns/t1/recover": () => ({ turn: { existing: false } }) },
  });
  const card = document.querySelector(".turn-failure");
  const banner = document.querySelector(".composer-harness-warning");
  const signedOut = m.chat_failure_signed_out({ name: autoDir("Claude Code") });
  assert.ok(card.textContent.includes(signedOut));
  assert.ok(!card.textContent.includes(m.chat_turn_incomplete()));
  assert.ok(!find(".turn-failure button", m.app_retry()));
  assert.ok(!find(".turn-failure button", m.harness_setup_login()));
  assert.equal(banner.querySelector('[role="status"]').textContent, signedOut);
  assert.equal(banner.querySelector("code"), null);
  assert.ok(find(".composer-input .cm-placeholder", m.add_agent_sign_in_title({ name: autoDir("Claude Code") })));
  await click(find(".composer-harness-warning button", m.harness_setup_login()));
  assert.equal(lastLogin().searchParams.get("action"), "login");
  assert.ok(card.isConnected);

  // The sign-in finishes and the agent now works.
  harnesses[1] = { ...signedOutClaude, authenticated: true, authState: "ready", agentReady: true, models: codex.models };
  await act(async () => sockets.at(-1).dispatchEvent(new MessageEvent("message", { data: JSON.stringify({ type: "complete" }) })));
  await act(async () => { for (const client of clients) await client.invalidateQueries(); });
  await settle();
  assert.equal(chat.requests.filter(({ request }) => request === "POST /api/chat/sessions/s/turns/t1/recover").length, 1);
});

test("without a sign-in here, the composer keeps the agent's note and the command it names", async () => {
  await renderChat("/projects/p/tasks/s", {
    sessions: [session("s", { harness: "claude-code", model: "opus" })],
    harnesses: [codex, { ...signedOutClaude, agentNote: "Sign in with `claude auth login`, then re-check this agent." }],
  });
  const banner = document.querySelector(".composer-harness-warning");
  assert.ok(banner.querySelector("code")?.textContent.includes("claude auth login"));
  assert.ok(!find(".composer-harness-warning button", m.harness_setup_login()));
});

test("searching the model picker narrows its signed-out agents too", async () => {
  await openPicker(signedOutClaude);
  await type(document.querySelector(`input[aria-label="${m.model_picker_search_models()}"]`), "sol");
  assert.ok(!find(".model-menu button", m.harness_setup_login()));
});

test("a sign-out whose agent now reads as signed in offers Retry beside Sign in", async () => {
  const recovery = {
    id: "turn-recovery", type: "tool", tool: "error",
    state: { status: "error", error: "Sign in with `claude auth login`, then re-check this agent.", input: { turnId: "t1", errorKind: "claude_signed_out", cause: "sign_in", recoveryAction: "retry", harness: "claude-code" } },
  };
  const signedIn = { ...signedOutClaude, authenticated: true, authState: "ready", agentReady: true, models: codex.models };
  const chat = await renderChat("/projects/p/tasks/s", {
    sessions: [session("s", { harness: "claude-code", model: "opus" })],
    transcripts: { s: [userMessage("u1", "Profile the loader."), assistantMessage("a1", [recovery], "u1")] },
    harnesses: [codex, signedIn],
    routes: { ...commands, "POST /api/chat/sessions/s/turns/t1/recover": () => ({ turn: { existing: false } }) },
  });
  // A ready agent may still hold the token the turn saw rejected.
  assert.ok(find(".turn-failure button", m.harness_setup_login()));
  await click(find(".turn-failure button", m.app_retry()));
  assert.equal(chat.requests.filter(({ request }) => request === "POST /api/chat/sessions/s/turns/t1/recover").length, 1);
});
