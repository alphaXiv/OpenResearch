import assert from "node:assert/strict";
import test from "node:test";
import { EMPTY_COMPOSER_STASH, mergeFailedComposer } from "../src/composerStash.ts";
import { ltr } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { annotate, assistantMessage, codex, composer, deferred, emit, press, refused, renderChat, session, userMessage } from "./chat.mjs";
import { model } from "./models.mjs";

const transcripts = { s: [userMessage("u1", "hello"), assistantMessage("a1", "Keep the old selection in mind.", "u1"), userMessage("u2", "and next?", "a1")] };
const sideTab = (sessionId) => `?pane=${encodeURIComponent(JSON.stringify({ kind: "side", sessionId }))}`;
const lost = () => new TypeError("Failed to fetch");

/**
 * Chat `s` with "original" citing an image and a selected passage in its composer; its first send waits on `reply`.
 * `side` hosts `s` as a side chat of `parent`; `steering` sends text alone, which a running turn takes.
 */
async function compose({ busy = false, steering = false, side = false, remote = false, submission = () => ({ accepted: false }), routes = {} } = {}) {
  const reply = deferred();
  let sends = 0;
  const sessions = [session("s", { busy, ...(side ? { sideParentSessionId: "parent" } : {}) }), session("other"), session("parent")];
  const chat = await renderChat(side ? `/projects/p/tasks/parent${sideTab("s")}` : "/projects/p/tasks/s", {
    sessions,
    transcripts: { ...transcripts, parent: [userMessage("p1", "parent question"), assistantMessage("p2", "Parent reply.", "p1")] },
    harnesses: [{ ...codex, supportsSteering: steering }],
    remote,
    routes: {
      "POST /api/chat/sessions/s/message": () => sends++ ? { ok: true } : reply.promise,
      "GET /api/chat/sessions/s/submissions/*": submission,
      ...routes,
    },
  });
  const input = composer(side ? 1 : 0);
  await input.type("original");
  if (!steering) {
    await input.attach();
    await annotate("old selection");
  }
  const leave = () => side ? chat.navigate(`/projects/p/tasks/parent${sideTab("other")}`) : chat.navigate("/projects/p/tasks/other");
  const unmount = () => chat.navigate("/projects");
  const reopen = async () => {
    await chat.navigate(side ? `/projects/p/tasks/parent${sideTab("s")}` : "/projects/p/tasks/s");
    return composer(side ? 1 : 0);
  };
  return { chat, input, original: input.draft, reply, sessions, leave, unmount, reopen };
}

/** Whether the transcript reports that a message was not sent. */
const reported = (error = "refused") => document.body.textContent.includes(m.chat_message_not_sent({ error: ltr(error) }));

for (const busy of [false, true]) {
  for (const unmounted of [false, true]) {
    for (const identical of [true, false]) {
      test(`failed ${busy ? "queued" : "normal"} send preserves ${identical ? "identical" : "different"} later draft ${unmounted ? "after unmount" : "in the active chat"}`, async () => {
        const chat = await compose({ busy });
        await chat.input.send();
        assert.equal(chat.input.draft, "");
        const newer = identical ? chat.original : "newer";
        await chat.input.type(newer);
        if (unmounted) await chat.unmount();
        await chat.reply.resolve(refused());
        const input = unmounted ? await chat.reopen() : chat.input;
        assert.equal(input.draft, `${chat.original}\n\n${newer}`);
      });
    }
  }
}

test("a failed history read restores one copy of the draft before anything is sent", async () => {
  const chat = await renderChat("/projects/p/tasks/s", { routes: {
    "GET /api/chat/sessions/s/messages": () => Response.json({ error: "history unavailable" }, { status: 503 }),
  } });
  const input = composer();
  await input.type("original");
  await input.send();
  assert.equal(input.draft, "original");
  assert.deepEqual(chat.sent(), []);
  assert.ok(reported("history unavailable"));
});

test("leaving before a failed send renders its clear restores one copy of the draft", async () => {
  const chat = await compose();
  await chat.input.send();
  await chat.unmount();
  await chat.reply.resolve(refused());
  assert.equal((await chat.reopen()).draft, chat.original);
});

for (const unmounted of [false, true]) {
  test(`a failed plan toggle restores one copy of the command${unmounted ? " after unmount" : ""}`, async () => {
    const toggle = deferred();
    const chat = await renderChat("/projects/p/tasks/s", { routes: { "PATCH /api/chat/sessions/s": () => toggle.promise } });
    const input = composer();
    await input.type("/plan");
    await press("Escape", input.field());
    await input.send();
    if (unmounted) await chat.navigate("/projects");
    await toggle.resolve(Response.json({ error: "toggle failed" }, { status: 500 }));
    if (unmounted) await chat.navigate("/projects/p/tasks/s");
    assert.equal(composer().draft, "/plan");
    if (!unmounted) assert.match(document.body.textContent, new RegExp(m.chat_toggle_plan_failed()));
  });
}

for (const side of [false, true]) {
  test(`failed ${side ? "side" : "main"} send after switching restores all content to its own chat`, async () => {
    const chat = await compose({ side });
    await chat.input.send();
    await chat.leave();
    await chat.reply.resolve(refused());
    assert.equal(composer(side ? 1 : 0).draft, "");
    const input = await chat.reopen();
    assert.equal(input.draft, chat.original);
    assert.equal(input.attachments, 1);
    assert.equal(input.annotations, 1);
    assert.ok(reported());
  });
}

test("recovery after switching merges content typed while the send was in flight", async () => {
  const chat = await compose();
  await chat.input.send();
  await chat.input.attach("newer.png");
  await annotate("in mind");
  await chat.input.type("newer");
  const newer = chat.input.draft;
  await chat.leave();
  await chat.reply.resolve(refused());
  const input = await chat.reopen();
  assert.equal(input.draft, `${chat.original}\n\n${newer}`);
  assert.equal(input.attachments, 2);
  assert.equal(input.annotations, 2);
});

test("recovery keeps annotations whose IDs collide but whose text differs", () => {
  const failed = { ...EMPTY_COMPOSER_STASH, draft: "original", annotations: [{ id: "a", text: "old selection" }] };
  const current = { ...EMPTY_COMPOSER_STASH, draft: "newer", annotations: [{ id: "a", text: "new selection" }] };
  const restored = mergeFailedComposer(current, failed);
  assert.equal(restored.draft, "original\n\nnewer");
  assert.deepEqual(restored.annotations.map((item) => item.text), ["old selection", "new selection"]);
  assert.notEqual(restored.annotations[0].id, restored.annotations[1].id);
  assert.deepEqual(mergeFailedComposer(failed, failed).annotations, failed.annotations);
});

test("a side chat created by its first send receives recovery after its draft composer unmounts", async () => {
  const reply = deferred();
  const created = session("created", { sideParentSessionId: "parent" });
  const sessions = [session("parent")];
  const chat = await renderChat(`/projects/p/tasks/parent${sideTab(null)}`, {
    sessions,
    routes: {
      "POST /api/chat/sessions/parent/side": () => { sessions.push(created); return { session: created }; },
      "GET /api/chat/sessions/created/messages": () => ({ messages: [] }),
      "POST /api/chat/sessions/created/message": () => reply.promise,
    },
  });
  await composer(1).type("original");
  await composer(1).send();
  await chat.navigate("/projects");
  await reply.resolve(refused());
  await chat.navigate(`/projects/p/tasks/parent${sideTab("created")}`);
  assert.equal(composer(1).draft, "original");
  await chat.navigate(`/projects/p/tasks/parent${sideTab(null)}`);
  assert.equal(composer(1).draft, "");
});

for (const change of ["deleted", "deleted parent"]) {
  test(`a late failure reports nothing for a ${change} side chat`, async () => {
    const chat = await compose({ side: true });
    await chat.input.send();
    await chat.leave();
    await emit("chat.session.deleted", { sessionId: change === "deleted" ? "s" : "parent" });
    await chat.reply.resolve(refused());
    assert.equal(reported(), false);
  });
}

test("a late failure after the workspace is replaced restores nothing into the new one", async () => {
  const chat = await compose({ side: true, remote: true });
  await chat.input.send();
  await chat.leave();
  await chat.chat.reconnect();
  await chat.reply.resolve(refused());
  assert.equal((await chat.reopen()).draft, "");
});

test("a successful send never restores its draft after switching", async () => {
  const chat = await compose();
  await chat.input.send();
  await chat.leave();
  await chat.reply.resolve({ ok: true });
  assert.equal((await chat.reopen()).draft, "");
});

test("a failed queued side send restores its composer and reports the error", async () => {
  const chat = await compose({ side: true, busy: true });
  await chat.input.send();
  await chat.leave();
  await chat.reply.resolve(refused());
  assert.equal((await chat.reopen()).draft, chat.original);
  assert.ok(reported());
});

for (const update of ["event", "refresh"]) {
  test(`an unconfirmed restored draft survives a server user message via ${update} and remounts`, async () => {
    const chat = await compose();
    await chat.input.send();
    await chat.unmount();
    await chat.reply.reject(lost());
    const accepted = userMessage("accepted", "original", "u2");
    transcripts.s.push(accepted);
    try {
      if (update === "event") await emit("chat.message", { sessionId: "s", message: accepted });
      let input = await chat.reopen();
      if (update === "refresh") await emit("resync.required", {});
      assert.ok(document.querySelector('[data-message-id="accepted"]'));
      assert.equal(input.draft, chat.original);
      assert.ok(input.warning.includes(m.request_connection_lost()), input.warning);

      await input.type("edited");
      await chat.unmount();
      input = await chat.reopen();
      assert.equal(input.draft, `${chat.original}edited`);
      assert.ok(input.warning.includes(m.request_connection_lost()), input.warning);

      await input.dismissWarning();
      await chat.unmount();
      assert.equal((await chat.reopen()).warning, null);
    } finally {
      transcripts.s.pop();
    }
  });
}

test("a failure after returning merges into the open composer without losing newer edits", async () => {
  const chat = await compose();
  await chat.input.send();
  await chat.leave();
  const input = await chat.reopen();
  await input.attach("newer.png");
  await annotate("in mind");
  await input.type("newer");
  const newer = input.draft;
  await chat.reply.resolve(refused());
  assert.equal(input.draft, `${chat.original}\n\n${newer}`);
  assert.equal(input.attachments, 2);
  assert.equal(input.annotations, 2);
});

for (const busy of [false, true]) {
  test(`a refused ${busy ? "queued" : "normal"} send after unmount restores its content and reports the refusal`, async () => {
    const chat = await compose({ busy });
    await chat.input.send();
    await chat.unmount();
    await chat.reply.resolve(refused());
    const input = await chat.reopen();
    assert.equal(input.draft, chat.original);
    assert.equal(input.attachments, 1);
    assert.equal(input.annotations, 1);
    assert.equal(input.warning, null);
    assert.ok(reported());
  });
}

for (const busy of [false, true]) {
  test(`an accepted ${busy ? "queued" : "normal"} send whose response was lost stays cleared`, async () => {
    const lookups = [];
    const chat = await compose({ busy, submission: (_, path) => { lookups.push(path); return { accepted: true }; } });
    await chat.input.send();
    await chat.reply.reject(lost());
    assert.deepEqual(lookups, [`/api/chat/sessions/s/submissions/${chat.chat.sent()[0].clientTurnId}`]);
    assert.match(chat.chat.sent()[0].clientTurnId, /^ct_/);
    assert.equal(chat.input.draft, "");
    assert.equal(chat.input.warning, null);
  });
}

test("an acceptance lookup cannot refresh a deleted chat", async () => {
  const lookup = deferred();
  const chat = await compose({ submission: () => lookup.promise });
  await chat.input.send();
  await chat.reply.reject(lost());
  await emit("chat.session.deleted", { sessionId: "s" });
  const reads = chat.chat.requests.length;
  await lookup.resolve({ accepted: true });
  assert.deepEqual(chat.chat.requests.slice(reads).filter(({ request }) => request.startsWith("GET /api/chat/sessions/s")), []);
});

test("a send confirmed after unmount leaves no retry behind", async () => {
  const chat = await compose({ submission: () => ({ accepted: true }) });
  await chat.input.send();
  await chat.unmount();
  await chat.reply.reject(lost());
  const input = await chat.reopen();
  assert.equal(input.draft, "");
  assert.equal(input.warning, null);
});

test("a refused send does not ask whether it arrived", async () => {
  const lookups = [];
  const chat = await compose({ submission: (_, path) => { lookups.push(path); return { accepted: true }; } });
  await chat.input.send();
  await chat.reply.resolve(refused());
  assert.deepEqual(lookups, []);
});

for (const busy of [false, true]) {
  test(`unconfirmed ${busy ? "steered" : "normal"} sends keep their content and warn`, async () => {
    const chat = await compose({ busy, steering: busy });
    await chat.input.send();
    if (busy) assert.equal(chat.chat.sent()[0].mode, "steer");
    await chat.unmount();
    await chat.reply.reject(lost());
    const input = await chat.reopen();
    assert.equal(input.draft, chat.original);
    assert.ok(input.warning.includes(m.request_connection_lost()), input.warning);
    assert.equal(reported(m.request_connection_lost()), false);
  });
}

test("a failed history read after switching restores the draft and reports it", async () => {
  const history = deferred();
  const chat = await renderChat("/projects/p/tasks/s", { sessions: [session("s"), session("other")], routes: {
    "GET /api/chat/sessions/s/messages": () => history.promise,
  } });
  await composer().type("original");
  await composer().send();
  await chat.navigate("/projects/p/tasks/other");
  await history.resolve(Response.json({ error: "history unavailable" }, { status: 503 }));
  await chat.navigate("/projects/p/tasks/s");
  assert.equal(composer().draft, "original");
  assert.deepEqual(chat.sent(), []);
});

test("a chat that cannot be created restores the unsent draft and reports why", async () => {
  const chat = await renderChat("/projects/p/tasks/new", { sessions: [session("other")], routes: {
    "POST /api/chat/sessions": () => Response.json({ error: "creation failed" }, { status: 500 }),
  } });
  await composer().type("original");
  await composer().send();
  assert.match(document.body.textContent, /creation failed/);
  await chat.navigate("/projects/p/tasks/other");
  await chat.navigate("/projects/p/tasks/new");
  assert.equal(composer().draft, "original");
});

test("an unchanged retry in the same composer keeps its client ID and clears the warning", async () => {
  const chat = await compose();
  await chat.input.send();
  await chat.reply.reject(lost());
  assert.ok(chat.input.warning.includes(m.request_connection_lost()), chat.input.warning);
  await chat.input.send();
  const [first, retry] = chat.chat.sent();
  assert.equal(retry.clientTurnId, first.clientTurnId);
  assert.equal(chat.input.warning, null);
});

test("sending a different message keeps the warning about an earlier uncertain one", async () => {
  const chat = await compose();
  await chat.input.send();
  await chat.reply.reject(lost());
  await chat.input.type("something else");
  await chat.input.send();
  assert.notEqual(chat.chat.sent()[1].clientTurnId, chat.chat.sent()[0].clientTurnId);
  assert.ok(chat.input.warning.includes(m.request_connection_lost()), chat.input.warning);
});

test("an accepted send leaves the composer empty", async () => {
  const chat = await compose();
  await chat.input.send();
  await chat.reply.resolve({ ok: true });
  assert.equal(chat.input.draft, "");
  assert.equal(chat.input.attachments, 0);
  assert.equal(chat.input.annotations, 0);
});

test("deleting the parent while a refused side send refreshes its history reports nothing", async () => {
  let reads = 0;
  const history = deferred();
  const chat = await compose({ side: true, routes: { "GET /api/chat/sessions/s/messages": () => ++reads === 1 ? { messages: transcripts.s } : history.promise } });
  await chat.input.send();
  await chat.reply.resolve(refused());
  assert.equal(reads, 2);
  await emit("chat.session.deleted", { sessionId: "parent" });
  await history.resolve({ messages: transcripts.s });
  assert.equal(reported(), false);
});

test("an uncertain send keeps its client ID after switching chats", async () => {
  const chat = await compose();
  await chat.input.send();
  await chat.reply.reject(lost());
  await chat.leave();
  await (await chat.reopen()).send();
  const [first, retry] = chat.chat.sent();
  assert.equal(retry.clientTurnId, first.clientTurnId);
});

for (const steering of [false, true]) {
  test(`a lost ${steering ? "steered" : "queued"} send keeps its ID and delivery mode after a chat switch`, async () => {
    const chat = await compose({ busy: true, steering });
    await chat.input.send();
    await chat.unmount();
    await chat.reply.reject(lost());
    chat.sessions[0].busy = false;
    await emit("chat.busy", { sessionId: "s", busy: false });
    const input = await chat.reopen();
    await input.send();
    const [first, retry] = chat.chat.sent();
    assert.equal(retry.clientTurnId, first.clientTurnId);
    assert.equal(retry.mode, first.mode);
    assert.equal(retry.planMode, first.planMode);
    assert.equal(retry.model, first.model);
    if (steering) assert.equal(document.querySelectorAll('[data-message-id^="local-"]').length, 0);
  });
}

test("editing a recovered draft sends it as a new message and keeps the warning", async () => {
  const chat = await compose();
  await chat.input.send();
  await chat.reply.reject(lost());
  await chat.input.type("changed request");
  await chat.input.send();
  const [first, next] = chat.chat.sent();
  assert.notEqual(next.clientTurnId, first.clientTurnId);
  assert.ok(chat.input.warning.includes(m.request_connection_lost()), chat.input.warning);
});

test("a session settings refresh does not change the recovered request", async () => {
  const chat = await compose();
  await chat.input.send();
  await chat.unmount();
  await chat.reply.reject(lost());
  await emit("chat.session", { session: { ...chat.sessions[0], model: "gpt-6-astra", reasoningLevel: "low" } });
  await (await chat.reopen()).send();
  const [first, retry] = chat.chat.sent();
  assert.equal(retry.clientTurnId, first.clientTurnId);
  assert.equal(retry.model, first.model);
  assert.equal(retry.reasoningLevel, first.reasoningLevel);
});

for (const [model, name] of [["gpt-6.1-sol", "GPT-6.1 Sol"], ["gpt-6-astra", "GPT-6 Astra"]]) {
  const changed = model !== "gpt-6.1-sol";
  test(`${changed ? "changing" : "re-picking"} the model ${changed ? "makes a recovered retry a new message" : "keeps a recovered retry"} and its warning`, async () => {
    const chat = await compose();
    await chat.input.send();
    await chat.reply.reject(lost());
    await chat.input.pick(name);
    assert.ok(chat.input.warning.includes(m.request_connection_lost()), chat.input.warning);
    await chat.input.send();
    const [first, retry] = chat.chat.sent();
    assert.equal(retry.clientTurnId === first.clientTurnId, !changed);
    assert.equal(retry.model, model);
  });
}

test("a model change while a send is in flight is not undone by its late failure", async () => {
  const chat = await compose();
  await chat.input.send();
  await chat.input.pick("GPT-6 Astra");
  await chat.unmount();
  await chat.reply.reject(lost());
  const input = await chat.reopen();
  assert.ok(input.warning.includes(m.request_connection_lost()), input.warning);
  await input.send();
  const [first, retry] = chat.chat.sent();
  assert.notEqual(retry.clientTurnId, first.clientTurnId);
});

test("retrying a failed plan message does not toggle plan a second time", async () => {
  const chat = await compose();
  await chat.input.type("/plan original");
  await chat.input.send();
  await chat.unmount();
  await chat.reply.reject(lost());
  await (await chat.reopen()).send();
  const [first, retry] = chat.chat.sent();
  assert.equal(first.planMode, true);
  assert.equal(retry.clientTurnId, first.clientTurnId);
  assert.equal(retry.planMode, true);
  assert.equal(chat.chat.requests.filter(({ request }) => request === "PATCH /api/chat/sessions/s").length, 0);
});

test("sending during an agent switch keeps the draft until the new agent is ready", async () => {
  const claude = { ...codex, id: "claude-code", name: "Claude Code", models: [model("claude-model", "Claude Model")] };
  const switched = deferred();
  const chat = await renderChat("/projects/p/tasks/s", {
    harnesses: [codex, claude],
    routes: { "PATCH /api/chat/sessions/s": () => switched.promise },
  });
  const input = composer();
  await input.type("original");
  await input.pick("Claude Model", "Claude Code");
  assert.deepEqual(chat.requests.filter(({ request }) => request === "PATCH /api/chat/sessions/s").map(({ body }) => body.agent.harness), ["claude-code"]);
  await input.send();
  assert.equal(input.draft, "original");
  assert.deepEqual(chat.sent(), []);
  assert.equal(document.querySelector('[data-message-id^="local-"]'), null);
});
