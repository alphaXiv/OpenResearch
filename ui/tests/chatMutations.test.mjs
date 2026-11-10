import assert from "node:assert/strict";
import test from "node:test";
import {
  mutCancelQueuedMessage,
  mutDeleteChatSession,
  mutRespondChat,
  mutRetryQueuedMessage,
  mutSelectChatBranch,
  mutSetChatConfiguration,
} from "../src/queries/chat.ts";
import { createWorkspace } from "../src/workspace.tsx";
import { serve } from "./render.mjs";

const deferred = () => { let resolve; const promise = new Promise((r) => { resolve = r; }); return { promise, resolve }; };
const refused = (error) => Response.json({ error }, { status: 409 });
const row = (id, extra = {}) => ({ id, projectId: "p", title: id, archived: false, busy: false, updatedAt: 1, ...extra });
const blocked = { id: "q1", text: "later", dispatchState: "blocked", error: "agent offline", nextRetryAt: null };

function setup() {
  const workspace = createWorkspace("chat-mutations");
  const { queryClient, queries } = workspace;
  const sessions = queries.listChatSessionsQuery("p");
  const sidebar = queries.listSidebarChatSessionsQuery("active");
  const messages = queries.getChatMessagesQuery("s");
  queryClient.setQueryData(sessions.queryKey, [row("s"), row("t")]);
  queryClient.setQueryData(sidebar.queryKey, { pages: [{ sessions: [row("s"), row("t")], next: null }], pageParams: [null] });
  queryClient.setQueryData(messages.queryKey, {
    messages: [{ id: "m1", role: "assistant", parts: [] }, { id: "m2", role: "assistant", parts: [] }],
    queued: [blocked],
    activeLeafId: "m2",
  });
  const reply = deferred();
  const requests = [];
  serve((path, init) => {
    const method = init.method ?? "GET";
    if (method !== "GET") { requests.push(`${method} ${path}`); return reply.promise.then((body) => body instanceof Response ? body.clone() : body); }
    if (path.startsWith("/api/chat/sessions?")) return { sessions: queryClient.getQueryData(sessions.queryKey) };
    if (path === "/api/chat/sessions/s/messages") return queryClient.getQueryData(messages.queryKey);
  });
  return {
    workspace,
    reply,
    requests,
    sessionIds: () => queryClient.getQueryData(sessions.queryKey).map((session) => session.id),
    sidebarIds: () => queryClient.getQueryData(sidebar.queryKey).pages.flatMap((page) => page.sessions.map((session) => session.id)),
    busy: () => queryClient.getQueryData(sessions.queryKey).find((session) => session.id === "s").busy,
    snapshot: () => queryClient.getQueryData(messages.queryKey),
  };
}

test("a deleted chat leaves the project list and the sidebar at once", async () => {
  const { workspace, reply, sessionIds, sidebarIds } = setup();
  const deleted = mutDeleteChatSession.runAsHeadlessPromise(workspace, row("s"), {});
  assert.deepEqual(sessionIds(), ["t"]);
  assert.deepEqual(sidebarIds(), ["t"]);
  reply.resolve({ ok: true });
  await deleted;
  assert.deepEqual(sessionIds(), ["t"]);
  assert.ok(workspace.deletedSessionIds.has("s"));
});

test("a refused delete brings the chat back and reports why", async () => {
  const { workspace, reply, sessionIds, sidebarIds } = setup();
  const deleted = mutDeleteChatSession.runAsHeadlessPromise(workspace, row("s"), {});
  reply.resolve(refused("This chat is still running."));
  await assert.rejects(deleted, { message: "This chat is still running." });
  assert.deepEqual(sessionIds(), ["s", "t"]);
  assert.deepEqual(sidebarIds(), ["s", "t"]);
  assert.ok(!workspace.deletedSessionIds.has("s"));
});

test("a removed queued message disappears at once and returns if the server keeps it", async () => {
  const { workspace, reply, snapshot } = setup();
  const removed = mutCancelQueuedMessage.runAsHeadlessPromise(workspace, "s", "q1", {});
  assert.deepEqual(snapshot().queued, []);
  reply.resolve(refused("That message is already being sent."));
  await assert.rejects(removed, { message: "That message is already being sent." });
  assert.deepEqual(snapshot().queued, [blocked]);
});

test("retrying a blocked message shows it queued, and blocked again if refused", async () => {
  const { workspace, reply, snapshot } = setup();
  const retried = mutRetryQueuedMessage.runAsHeadlessPromise(workspace, "s", "q1", {});
  assert.equal(snapshot().queued[0].dispatchState, "queued");
  assert.equal(snapshot().queued[0].error, null);
  reply.resolve(refused("The agent is not signed in."));
  await assert.rejects(retried, { message: "The agent is not signed in." });
  assert.deepEqual(snapshot().queued, [blocked]);
});

test("an answer turns the chat busy at once and idle again if refused", async () => {
  const { workspace, reply, busy } = setup();
  const answered = mutRespondChat.runAsHeadlessPromise(workspace, row("s"), { promptId: "ask", answers: ["A"] }, {});
  assert.equal(busy(), true);
  reply.resolve(refused("This question was already answered."));
  await assert.rejects(answered, { message: "This question was already answered." });
  assert.equal(busy(), false);
});

test("a fork switch shows at once and returns to the shown branch if refused", async () => {
  const { workspace, reply, snapshot } = setup();
  const switched = mutSelectChatBranch.runAsHeadlessPromise(workspace, "s", "m1", {});
  assert.equal(snapshot().activeLeafId, "m1");
  reply.resolve(refused("A turn is running."));
  await assert.rejects(switched, { message: "A turn is running." });
  assert.equal(snapshot().activeLeafId, "m2");
});

test("an agent setting shows at once in either configuration shape and rolls back if refused", async () => {
  const configOptions = { configOptions: [{ id: "effort", type: "select", name: "Effort", currentValue: "low", options: [] }] };
  const legacy = { models: { currentModelId: "small", availableModels: [] } };
  for (const [nativeConfiguration, optionId, value, shown] of [
    [configOptions, "effort", "high", (configuration) => configuration.configOptions[0].currentValue],
    [legacy, "model", "large", (configuration) => configuration.models.currentModelId],
  ]) {
    const { workspace, reply } = setup();
    const { queryClient, queries } = workspace;
    const options = queries.listChatSessionsQuery("p");
    queryClient.setQueryData(options.queryKey, [row("s", { nativeConfiguration })]);
    const configuration = () => queryClient.getQueryData(options.queryKey)[0].nativeConfiguration;

    const saved = mutSetChatConfiguration.runAsHeadlessPromise(workspace, row("s"), optionId, value, {});
    assert.equal(shown(configuration()), value);
    reply.resolve(refused("That choice is not available."));
    await assert.rejects(saved, { message: "That choice is not available." });
    assert.deepEqual(configuration(), nativeConfiguration);
  }
});
