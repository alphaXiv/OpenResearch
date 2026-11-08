import assert from "node:assert/strict";
import test from "node:test";
import { assistantMessage, codex, composer, emit, renderChat, session, userMessage } from "./chat.mjs";

const plan = (id, synthesized = false) => assistantMessage(id, [{
  id: `${id}-plan`, type: "prompt",
  prompt: { kind: "plan", plan: "Inspect the inputs before making changes.", resolved: false, synthesized },
}], "u1");

const activeReply = (parentId) => ({ ...assistantMessage("working", "Inspecting the inputs.", parentId), completedAt: null });

for (const synthesized of [false, true]) {
  for (const started of [false, true]) {
    test(`an older ${synthesized ? "synthesized" : "native"} plan cannot block steering ${started ? "during" : "before"} the next reply`, async () => {
      const messages = [userMessage("u1", "Make a plan"), plan("old", synthesized), userMessage("u2", "Inspect the inputs", "old")];
      if (started) messages.push(activeReply("u2"));
      const chat = await renderChat("/projects/p/tasks/s", {
        sessions: [session("s", { busy: true, planMode: true })],
        harnesses: [{ ...codex, supportsSteering: true }], transcripts: { s: messages },
        routes: { "POST /api/chat/sessions/s/message": () => ({ ok: true }) },
      });
      assert.equal(Boolean(document.querySelector(".plan-strip")), false);
      await composer().type("Also inspect the metadata");
      await composer().send();
      assert.equal(chat.sent()[0].mode, "steer");
    });
  }
}

test("a current plan stays pending when a message is only queued, then clears when a new turn arrives", async () => {
  const messages = [userMessage("u1", "Make a plan"), plan("current")];
  const chat = await renderChat("/projects/p/tasks/s", {
    sessions: [session("s", { busy: true, planMode: true })],
    harnesses: [{ ...codex, supportsSteering: true }], transcripts: { s: messages },
    routes: { "POST /api/chat/sessions/s/message": () => ({ ok: true }) },
  });
  assert.ok(document.querySelector(".plan-strip"));
  await emit("chat.queued", { sessionId: "s", items: [{ id: "q1", text: "Wait for the plan" }] });
  assert.ok(document.querySelector(".plan-strip"));
  await composer().type("A follow-up while the plan is pending");
  await composer().send();
  assert.equal(chat.sent()[0].mode, undefined);

  await emit("chat.message", { sessionId: "s", message: userMessage("u2", "Inspect the inputs", "current") });
  await emit("chat.message", { sessionId: "s", message: activeReply("u2") });
  assert.equal(Boolean(document.querySelector(".plan-strip")), false);
  await composer().type("Also inspect the metadata");
  await composer().send();
  assert.equal(chat.sent()[1].mode, "steer");
});

test("a plan on the latest idle reply remains actionable", async () => {
  await renderChat("/projects/p/tasks/s", {
    transcripts: { s: [userMessage("u1", "Make a plan"), plan("current")] },
  });
  assert.ok(document.querySelector(".plan-strip"));
});

test("an obsolete plan cannot block steering in a side chat", async () => {
  const pane = encodeURIComponent(JSON.stringify({ kind: "side", sessionId: "s" }));
  const chat = await renderChat(`/projects/p/tasks/parent?pane=${pane}`, {
    sessions: [session("parent"), session("s", { busy: true, planMode: true })],
    harnesses: [{ ...codex, supportsSteering: true }],
    transcripts: { s: [userMessage("u1", "Make a plan"), plan("old"), userMessage("u2", "Inspect the inputs", "old"), activeReply("u2")] },
    routes: { "POST /api/chat/sessions/s/message": () => ({ ok: true }) },
  });
  assert.equal(Boolean(document.querySelector(".plan-strip")), false);
  await composer(1).type("Also inspect the metadata");
  await composer(1).send();
  assert.equal(chat.sent()[0].mode, "steer");
});
