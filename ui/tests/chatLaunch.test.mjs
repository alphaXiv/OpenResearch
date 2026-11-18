import assert from "node:assert/strict";
import test from "node:test";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, composer, deferred, emit, renderChat, session, userMessage } from "./chat.mjs";

const thread = () => document.querySelector(".chat-thread")?.textContent ?? "";

/** A new chat whose creation and first history read answer when the test says. */
async function launch() {
  const created = deferred();
  const history = deferred();
  const chat = await renderChat("/projects/p/tasks/new", { sessions: [session("other"), session("fresh")], routes: {
    "POST /api/chat/sessions": () => created.promise,
    "GET /api/chat/sessions/fresh/messages": () => history.promise,
    "POST /api/chat/sessions/fresh/message": () => ({ ok: true }),
  } });
  await composer().type("map the loss landscape");
  await composer().send();
  return { chat, created, history };
}

test("a new chat shows the sent message and its setup before the chat exists", async () => {
  const { chat, created, history } = await launch();
  assert.match(thread(), /map the loss landscape/);
  assert.match(thread(), new RegExp(m.chat_setting_up_worktree()));
  assert.ok([...document.querySelectorAll('[role="status"]')].some((region) => region.textContent === m.chat_setting_up_worktree()));
  assert.equal(document.querySelector(".chat-loading"), null);

  await created.resolve({ session: session("fresh") });
  assert.equal(location.pathname, "/projects/p/tasks/fresh");
  assert.match(thread(), new RegExp(m.chat_setting_up_worktree()));
  assert.equal(document.querySelector(".chat-loading"), null);

  await history.resolve({ messages: [], queued: [], activeLeafId: null });
  assert.equal(thread().split("map the loss landscape").length, 2);
  assert.equal(chat.sent()[0].text, "map the loss landscape");

  // The agent has said nothing yet, so its first turn is still setting up.
  await emit("chat.message", { sessionId: "fresh", message: userMessage("u1", "map the loss landscape") });
  assert.match(thread(), new RegExp(m.chat_setting_up_worktree()));
  assert.doesNotMatch(thread(), new RegExp(m.chat_thinking()));

  await emit("chat.message", { sessionId: "fresh", message: { ...assistantMessage("a1", [{ id: "r1", type: "reasoning", text: "" }], "u1"), completedAt: undefined } });
  assert.doesNotMatch(thread(), new RegExp(m.chat_setting_up_worktree()));
});

test("a new chat opened while the last one loads starts empty", async () => {
  const { chat, created } = await launch();
  await created.resolve({ session: session("fresh") });
  await chat.navigate("/projects/p/tasks/new");
  assert.doesNotMatch(document.body.textContent, /map the loss landscape/);
  assert.match(document.body.textContent, new RegExp(m.chat_panel_what_should_we_research()));
});

test("leaving a new chat before it exists ends its launch", async () => {
  const { chat, created } = await launch();
  await chat.navigate("/projects/p/tasks/other");
  await chat.navigate("/projects/p/tasks/new");
  assert.doesNotMatch(document.body.textContent, /map the loss landscape/);
  await created.resolve({ session: session("fresh") });
  assert.equal(location.pathname, "/projects/p/tasks/new");
  assert.doesNotMatch(document.body.textContent, /map the loss landscape/);
});

test("a chat that cannot be created takes its sent message back to the composer", async () => {
  const { created } = await launch();
  await created.resolve(Response.json({ error: "creation failed" }, { status: 500 }));
  assert.equal(document.querySelector(".chat-thread"), null);
  assert.equal(composer().draft, "map the loss landscape");
});
