import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { SHELL_TOOL } from "../src/queries/chatState.ts";
import { assistantMessage, codex, composer, deferred, emit, refused, renderChat, session, settle, userMessage } from "./chat.mjs";
import { find } from "./render.mjs";

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

for (const synthesized of [false, true]) {
  for (const outcome of ["success", "exit failure", "request failure"]) {
    test(`a ${synthesized ? "synthesized" : "native"} plan stays actionable through shell ${outcome}`, async () => {
      const pending = deferred();
      const current = plan("current", synthesized);
      const messages = [userMessage("u1", "Make a plan"), current];
      const shell = {
        id: "shell", role: "user", parentId: "current", createdAt: 4,
        parts: [{ id: "p0", type: "tool", tool: SHELL_TOOL, state: {
          status: outcome === "success" ? "completed" : "error",
          input: { command: "git status", exitCode: outcome === "success" ? 0 : 1 },
          output: "Shell command finished.",
        } }],
      };
      const chat = await renderChat("/projects/p/tasks/s", {
        transcripts: { s: messages },
        routes: {
          "POST /api/chat/sessions/s/shell": () => pending.promise,
          "POST /api/chat/sessions/s/respond": (init) => {
            const answer = JSON.parse(init.body);
            assert.equal(answer.promptId, "current-plan");
            assert.equal(answer.approve, true);
            current.parts[0].prompt = { ...current.parts[0].prompt, resolved: true, approved: true };
            return { ok: true };
          },
        },
      });
      const assertApproval = () => {
        assert.ok(find("button", m.plan_strip_accept_plan()));
        assert.ok(find("button", m.plan_strip_reject()));
      };
      await composer().type("!git status");
      await composer().send();
      assert.deepEqual(chat.requests.filter(({ request }) => request.endsWith("/shell")).map(({ body }) => body), [{ command: "git status" }]);
      assertApproval();
      if (outcome === "request failure") {
        await pending.resolve(refused("Shell unavailable"));
        assert.ok(document.body.textContent.includes("Shell unavailable"));
      } else {
        messages.push(shell);
        await pending.resolve({ message: shell });
        assertApproval();
        await chat.navigate("/projects/p/tasks/new");
        await chat.navigate("/projects/p/tasks/s");
      }
      assertApproval();
      await act(async () => find("button", m.plan_strip_accept_plan()).click());
      await settle();
      assert.equal(chat.requests.filter(({ request }) => request.endsWith("/respond")).length, 1);
      assert.equal(Boolean(document.querySelector(".plan-strip")), false);
    });
  }
}

for (const synthesized of [false, true]) {
  test(`accepting a ${synthesized ? "synthesized" : "native"} plan cannot uncover an older approval box`, async () => {
    let current = { ...plan("current", synthesized), parentId: "u2" };
    let messages = [userMessage("u1", "Make a plan"), plan("old"), userMessage("u2", "Refine the plan", "old"), current];
    const sessions = [session("s", { planMode: true })];
    const chat = await renderChat("/projects/p/tasks/s", {
      sessions, harnesses: [{ ...codex, supportsSteering: true }],
      routes: {
        "GET /api/chat/sessions/s/messages": () => ({ messages, queued: [], activeLeafId: messages.at(-1).id }),
        "POST /api/chat/sessions/s/respond": (init) => {
          const answer = JSON.parse(init.body);
          assert.equal(answer.promptId, "current-plan");
          assert.equal(answer.approve, true);
          current = { ...current, parts: current.parts.map((part) => ({ ...part, prompt: { ...part.prompt, resolved: true, approved: true } })) };
          messages = [...messages.slice(0, -1), current];
          sessions[0] = { ...sessions[0], planMode: false, busy: true };
          return { ok: true };
        },
        "POST /api/chat/sessions/s/message": () => ({ ok: true }),
      },
    });
    await act(async () => find("button", m.plan_strip_accept_plan()).click());
    await settle();
    assert.equal(chat.requests.filter(({ request }) => request.endsWith("/respond")).length, 1);
    assert.equal(Boolean(document.querySelector(".plan-strip")), false);
    assert.equal(messages[1].parts[0].prompt.resolved, false);
    await emit("chat.message", { sessionId: "s", message: current });
    assert.equal(Boolean(document.querySelector(".plan-strip")), false);

    const implementation = userMessage("u3", "Implement the plan.", "current");
    const working = activeReply("u3");
    messages = [...messages, implementation, working];
    await emit("chat.session", { session: sessions[0] });
    await emit("chat.message", { sessionId: "s", message: implementation });
    await emit("chat.message", { sessionId: "s", message: working });
    await chat.navigate("/projects/p/tasks/new");
    await chat.navigate("/projects/p/tasks/s");
    assert.equal(Boolean(document.querySelector(".plan-strip")), false);
    await composer().type("Also inspect the metadata");
    await composer().send();
    assert.equal(chat.sent()[0].mode, "steer");
  });
}

test("a failed plan acceptance keeps the current approval box available", async () => {
  const chat = await renderChat("/projects/p/tasks/s", {
    transcripts: { s: [userMessage("u1", "Make a plan"), plan("current")] },
    routes: { "POST /api/chat/sessions/s/respond": () => refused("Plan acceptance failed") },
  });
  await act(async () => find("button", m.plan_strip_accept_plan()).click());
  await settle();
  assert.equal(chat.requests.filter(({ request }) => request.endsWith("/respond")).length, 1);
  assert.ok(document.querySelector(".plan-strip"));
  assert.ok(document.body.textContent.includes("Plan acceptance failed"));
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
