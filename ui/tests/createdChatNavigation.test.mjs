import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, deferred, renderChat, session, userMessage } from "./chat.mjs";

const created = "chat_0f8fad5b-d9cb-469f-a165-70867728950e";
const spawn = assistantMessage("a1", [{
  id: "tool-1", type: "tool", tool: "bash",
  state: { status: "completed", input: { command: "orx agent spawn --title Sweep 'run the sweep'" }, output: `Started ${created}` },
}], "u1");

/** Chat `s` whose agent spawned `created`; looking it up answers with `lookup`. */
async function spawned(lookup) {
  const chat = await renderChat("/projects/p/tasks/s", {
    sessions: [session("s"), session("other"), session("elsewhere", { projectId: "q" })],
    transcripts: { s: [userMessage("u1", "start a sweep"), spawn] },
    routes: { [`GET /api/chat/sessions/${created}`]: lookup },
  });
  const open = () => act(async () => document.querySelector(`button[title="${m.chat_panel_open_the_session_this_agent_spawned()}"]`).click());
  return { chat, open, location: () => location.pathname };
}

test("a created-chat link navigates to its destination project", async () => {
  const { chat, open, location } = await spawned(() => ({ session: session(created, { projectId: "q" }) }));
  await open();
  await chat.navigate(location());
  assert.equal(location(), `/projects/q/tasks/${created}`);
});

test("a missing created chat reports the error without changing navigation", async () => {
  const { chat, open, location } = await spawned(() => Response.json({ error: "Chat unavailable" }, { status: 404 }));
  await open();
  await chat.navigate(location());
  assert.equal(location(), "/projects/p/tasks/s");
  assert.match(document.body.textContent, /Chat unavailable/);
});

for (const [change, destination] of [["chat", "/projects/p/tasks/other"], ["project", "/projects/q/tasks/elsewhere"]]) {
  for (const failure of [false, true]) {
    test(`a late ${failure ? "error" : "chat lookup"} cannot override a changed ${change}`, async () => {
      const lookup = deferred();
      const { chat, open, location } = await spawned(() => lookup.promise);
      await open();
      await chat.navigate(destination);
      await lookup.resolve(failure ? Response.json({ error: "Chat unavailable" }, { status: 404 }) : { session: session(created, { projectId: "q" }) });
      assert.equal(location(), destination);
      assert.doesNotMatch(document.body.textContent, /Chat unavailable/);
    });
  }
}
