import assert from "node:assert/strict";
import test, { afterEach, beforeEach } from "node:test";
import { QueryObserver } from "@tanstack/react-query";
import { act, createElement } from "react";
import { QueryEvents } from "../src/queries/Events.tsx";
import * as live from "../src/queries/live.ts";
import { createWorkspace } from "../src/workspace.tsx";
import { render, serve, streams, settle as settleView } from "./render.mjs";

let workspace, client, queries, invalidation, chatStore;
beforeEach(() => {
  workspace = createWorkspace("cache");
  ({ queryClient: client, queries, invalidation, chat: chatStore } = workspace);
});
afterEach(() => client.clear());
const deferred = () => { let resolve; const promise = new Promise((r) => { resolve = r; }); return { promise, resolve }; };
const settle = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };

/** Serves each request whose path starts with a route's prefix with that route's reply. */
function backend(routes) {
  serve((path, init) => {
    const route = Object.keys(routes).find((prefix) => path.startsWith(prefix));
    return route && routes[route](init);
  });
}

test("components and imperative readers deduplicate and reuse fresh data", async (t) => {
  let calls = 0;
  const pending = deferred();
  backend({ "/api/projects": () => { calls++; return pending.promise.then((projects) => ({ projects })); } });
  const options = queries.listProjectsQuery();
  const observer = new QueryObserver(client, options);
  const unsubscribe = observer.subscribe(() => {});
  const route = client.fetchQuery(options);
  const helper = client.fetchQuery(options);
  pending.resolve([{ id: "p" }]);
  assert.equal(await route, await helper);
  assert.equal(observer.getCurrentResult().data, await client.fetchQuery(options));
  assert.equal(calls, 1);
  unsubscribe();
});

test("a hidden machine keeps its data for an hour, and none once its workspace is retired", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout", "Date"], now: 10_000 });
  backend({ "/api/projects": () => ({ projects: [{ id: "p" }] }) });
  const options = queries.listProjectsQuery();
  const observer = new QueryObserver(client, options);
  let off = observer.subscribe(() => {});
  await settle();
  off();
  t.mock.timers.tick(3_599_000);
  assert.ok(client.getQueryData(options.queryKey));
  off = observer.subscribe(() => {});
  assert.equal(observer.getCurrentResult().data[0].id, "p");
  await settle();
  off();
  t.mock.timers.tick(3_600_000);
  assert.equal(client.getQueryData(options.queryKey), undefined);
  off = observer.subscribe(() => {});
  await settle();
  off();
  workspace.replaceWorkspace();
  await settle();
  assert.equal(client.getQueryData(options.queryKey), undefined);
});

test("hidden transcripts retain stream updates and retired lifetimes reject later events", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout", "Date"], now: 10_000 });
  const snapshot = { messages: [], queued: [], activeLeafId: null };
  backend({ "/api/chat/sessions/s/messages": () => snapshot });
  const options = queries.getChatMessagesQuery("s");
  await client.fetchQuery(options);
  t.mock.timers.tick(300_000);
  client.setQueryData(options.queryKey, (data) => data && { ...data, activeLeafId: "event" });
  t.mock.timers.tick(300_000);
  assert.equal(client.getQueryData(options.queryKey).activeLeafId, "event");
  workspace.replaceWorkspace();
  await settle();
  workspace.setScopedQueryData(options.queryKey, (data) => data && { ...data, activeLeafId: "late-event" });
  assert.equal(client.getQueryCache().find({ queryKey: options.queryKey }), undefined);
});

test("freshness is query-specific and unobserved data lasts an hour", () => {
  assert.equal(client.getDefaultOptions().queries.gcTime, 3_600_000);
  assert.equal(client.getDefaultOptions().queries.retry, false);
  assert.equal(queries.listProjectsQuery().staleTime, 30_000);
  assert.equal(queries.getUiStateQuery().staleTime, 300_000);
  assert.equal(queries.resolvePaperQuery("id").staleTime, 3_600_000);
  assert.equal(queries.getArtifactFileMetadataQuery("p", "x").staleTime, 2_000);
  assert.equal(queries.getProjectFileQuery("p", "x").staleTime, Infinity);
  assert.equal(queries.getProjectFileQuery("p", "x").refetchOnMount, "always");
  assert.equal(queries.getProjectFileQuery("p", "x", { ref: "main" }).staleTime, 30_000);
  assert.equal(queries.getProjectFileQuery("p", "x", { ref: "a".repeat(40) }).staleTime, Infinity);
  assert.equal(queries.getChatMessagesQuery("s").refetchOnWindowFocus, "always");
  assert.equal(queries.getSshMasterStatusQuery("host").staleTime, 5_000);
});

test("replacement cancels reads and prevents late results from repopulating the retired cache", async () => {
  const pending = deferred();
  let signal;
  backend({ "/api/projects": (init) => { signal = init.signal; return pending.promise.then((projects) => ({ projects })); } });
  const old = queries.listProjectsQuery();
  const read = client.fetchQuery(old).catch(() => null);
  assert.equal(signal.aborted, false);
  workspace.replaceWorkspace();
  assert.equal(signal.aborted, true);
  pending.resolve([{ id: "old" }]);
  await read;
  assert.equal(client.getQueryData(old.queryKey), undefined);
  assert.equal(workspace.isCurrentScope(old.queryKey), false);
});

test("snapshot repair is bounded while preserving events during the second read", async (t) => {
  const requests = [deferred(), deferred()];
  let calls = 0;
  backend({ "/api/projects": () => requests[calls++].promise.then((projects) => ({ projects })) });
  const options = queries.listProjectsQuery();
  const read = client.fetchQuery(options);
  live.markLiveUpdate(client, options.queryKey, "p");
  requests[0].resolve([{ id: "p", title: "old" }]);
  await settle();
  assert.equal(calls, 2);
  live.markLiveUpdate(client, options.queryKey, "p");
  client.setQueryData(options.queryKey, [{ id: "p", title: "streamed" }]);
  requests[1].resolve([{ id: "p", title: "intermediate" }]);
  assert.deepEqual(await read, [{ id: "p", title: "streamed" }]);
  assert.equal(calls, 2);
});

test("session deletion during a read cannot resurrect its row", async (t) => {
  const pending = deferred();
  backend({ "/api/chat/sessions?projectId=p": () => pending.promise.then((sessions) => ({ sessions })) });
  const options = queries.listChatSessionsQuery("p");
  const read = client.fetchQuery(options);
  invalidation.removeSession("s");
  pending.resolve([{ id: "s", projectId: "p" }]);
  assert.deepEqual(await read, []);
});

test("live bounded previews refresh their existing source key without retaining versions", async (t) => {
  let calls = 0;
  backend({ "/api/projects/p/file": () => ({ path: "x", content: String(++calls), notFound: false }) });
  const options = queries.resolvedFileQuery("p", "x", "repo");
  assert.equal((await client.fetchQuery(options)).file.content, "1");
  await client.invalidateQueries(options);
  assert.equal((await client.fetchQuery(options)).file.content, "2");
  assert.equal(client.getQueryCache().getAll().length, 2);
});

test("background transcripts retain updates without rerendering the visible chat", async () => {
  const snapshot = { messages: [], queued: [], activeLeafId: null };
  const rows = [{ id: "active", busy: false }, { id: "background", busy: true }];
  client.setQueryData(queries.listChatSessionsQuery("p").queryKey, rows);
  for (const id of ["active", "background"]) client.setQueryData(queries.getChatMessagesQuery(id).queryKey, snapshot);
  backend({
    "/api/chat/sessions?projectId=p": () => ({ sessions: rows }),
    "/api/chat/sessions/active/messages": () => snapshot,
    "/api/chat/sessions/background/messages": () => client.getQueryData(queries.getChatMessagesQuery("background").queryKey),
  });
  let renders = 0;
  let visible;
  function Chat({ id }) {
    [visible] = chatStore.useChatState("p", id);
    renders++;
    return createElement("div", null, visible.messagesBySession[id]?.map((message) => message.id).join(","));
  }
  const view = await render(createElement(Chat, { id: "active" }), workspace);
  await settleView();
  const before = renders;
  await act(() => {
    for (let i = 0; i < 100; i++) chatStore.dispatchChat("p", { type: "upsertMessage", sessionId: "background", message: { id: "reply", role: "assistant", parts: [{ type: "text", text: String(i) }], createdAt: 1 } }, true);
    chatStore.dispatchChat("p", { type: "setQueued", sessionId: "background", items: [{ id: "queued" }] }, true);
  });
  await settleView();
  assert.equal(renders, before);
  assert.deepEqual(Object.keys(visible.messagesBySession), ["active"]);
  const stored = client.getQueryData(queries.getChatMessagesQuery("background").queryKey);
  assert.equal(stored.messages[0].parts[0].text, "99");
  assert.equal(stored.queued[0].id, "queued");
  await view.rerender(createElement(Chat, { id: "background" }));
  await settleView();
  assert.equal(view.container.textContent, "reply");
  assert.equal(visible.messagesBySession.background[0].parts[0].text, "99");
  assert.equal(visible.queuedBySession.background[0].id, "queued");
});

test("resync bursts share a refresh and repair a gap reported during it", async () => {
  const requests = [];
  backend({ "/api/projects": () => { const request = deferred(); requests.push(request); return request.promise; } });
  const options = queries.listProjectsQuery();
  client.setQueryData(options.queryKey, []);
  const observer = new QueryObserver(client, options);
  const off = observer.subscribe(() => {});
  const first = invalidation.repairLiveQueries();
  await settle();
  for (let i = 0; i < 20; i++) assert.equal(invalidation.repairLiveQueries(), first);
  assert.equal(requests.length, 1);
  requests[0].resolve({ projects: [{ id: "before-gap" }] });
  await settle();
  assert.equal(requests.length, 2);
  requests[1].resolve({ projects: [{ id: "after-gap" }] });
  await first;
  assert.equal(requests.length, 2);
  assert.equal(client.getQueryData(options.queryKey)[0].id, "after-gap");
  off();
});

for (const cached of [false, true]) test(`resync repairs a pending ${cached ? "cached" : "cold"} read without rejecting its caller`, async () => {
  const requests = [];
  backend({ "/api/projects": (init) => { const request = deferred(); requests.push({ ...request, signal: init.signal }); return request.promise; } });
  const options = queries.listProjectsQuery();
  if (cached) client.setQueryData(options.queryKey, []);
  const observer = new QueryObserver(client, options);
  const off = observer.subscribe(() => {});
  const read = client.fetchQuery({ ...options, staleTime: 0 });
  await settle();
  const repair = invalidation.repairLiveQueries();
  await settle();
  assert.equal(requests[0].signal.aborted, cached);
  if (!cached) {
    assert.equal(requests.length, 1);
    requests[0].resolve({ projects: [{ id: "before-gap" }] });
    assert.equal((await read)[0].id, "before-gap");
    await settle();
  }
  assert.equal(requests.length, 2);
  requests[1].resolve({ projects: [{ id: "repaired" }] });
  await repair;
  requests[0].resolve({ projects: [{ id: "stale" }] });
  if (cached) assert.equal((await read)[0].id, "repaired");
  assert.equal(client.getQueryData(options.queryKey)[0].id, "repaired");
  off();
});

test("reconnect preserves a side-chat send waiting on its cold history", async () => {
  const history = deferred();
  let sends = 0;
  backend({ "/api/chat/sessions/side/messages": () => history.promise });
  const send = client.fetchQuery(queries.getChatMessagesQuery("side")).then(() => { sends++; });
  await invalidation.repairLiveQueries();
  history.resolve({ messages: [], queued: [], activeLeafId: null });
  await send;
  assert.equal(sends, 1);
});

test("retiring a scope rejects a refresh even when it had cached data", async (t) => {
  const pending = deferred();
  backend({ "/api/projects": () => pending.promise.then((projects) => ({ projects })) });
  const options = queries.listProjectsQuery();
  client.setQueryData(options.queryKey, [{ id: "old" }]);
  const read = client.fetchQuery({ ...options, staleTime: 0 });
  const rejected = assert.rejects(read);
  workspace.replaceWorkspace();
  pending.resolve([{ id: "late" }]);
  await rejected;
  await settle();
  assert.equal(client.getQueryData(options.queryKey), undefined);
});

test("cold chat repair preserves message order, queue, and branch events without a third read", async (t) => {
  const requests = [deferred(), deferred()];
  let calls = 0;
  backend({ "/api/chat/sessions/s/messages": () => requests[calls++].promise });
  const options = queries.getChatMessagesQuery("s");
  const store = chatStore;
  const first = { id: "one", role: "assistant", parts: [], createdAt: 1 };
  const second = { id: "two", role: "assistant", parentId: "one", parts: [], createdAt: 2 };
  const baseline = { messages: [first], queued: [], activeLeafId: "one" };
  const read = client.fetchQuery(options);
  store.dispatchChat("p", { type: "upsertMessage", sessionId: "s", message: first }, true);
  assert.equal(client.getQueryData(options.queryKey), undefined);
  requests[0].resolve(baseline);
  await settle();
  assert.equal(calls, 2);
  store.dispatchChat("p", { type: "upsertMessage", sessionId: "s", message: second }, true);
  store.dispatchChat("p", { type: "setQueued", sessionId: "s", items: [{ id: "queue" }] }, true);
  store.dispatchChat("p", { type: "activeLeaf", sessionId: "s", leafId: "two" }, true);
  requests[1].resolve(baseline);
  const result = await read;
  assert.deepEqual(result.messages.map((message) => message.id), ["one", "two"]);
  assert.deepEqual(result.queued, [{ id: "queue" }]);
  assert.equal(result.activeLeafId, "two");
  assert.equal(calls, 2);
});

test("a pending local turn survives reconciliation regardless of server clock skew", async (t) => {
  const existing = { id: "server", role: "user", parts: [], createdAt: Date.now() + 999999 };
  const snapshot = { messages: [existing], queued: [], activeLeafId: "server" };
  backend({ "/api/chat/sessions/s/messages": () => snapshot });
  const options = queries.getChatMessagesQuery("s");
  client.setQueryData(options.queryKey, snapshot);
  chatStore.dispatchChat("p", { type: "optimisticUser", sessionId: "s", text: "next", attachments: [], annotations: [] });
  const localId = client.getQueryData(options.queryKey).activeLeafId;
  const result = await client.fetchQuery({ ...options, staleTime: 0 });
  assert.deepEqual(result.messages.map((message) => message.id), ["server", localId]);
  assert.equal(result.activeLeafId, localId);
});

test("compaction's initial response cannot overwrite a newer streamed result", () => {
  const store = chatStore;
  for (const status of ["completed", "error"]) {
    const options = queries.getChatMessagesQuery(status);
    const running = {
      id: "compact", role: "assistant", createdAt: 1, parentId: "previous",
      parts: [{ id: "compacted", type: "tool", tool: "compacted", state: { status: "running" } }],
    };
    const finished = {
      ...running, completedAt: 2,
      parts: [{ ...running.parts[0], state: { status, ...(status === "error" ? { error: "Could not compact" } : {}) } }],
    };
    const dispatch = (message, onlyIfMissing = false) => store.dispatchChat("p", {
      type: "upsertMessage", sessionId: status, message, onlyIfMissing,
    });
    dispatch(running, true);
    assert.deepEqual(client.getQueryData(options.queryKey).messages, [running]);
    dispatch(finished);
    store.dispatchChat("p", { type: "activeLeaf", sessionId: status, leafId: "another-branch" });
    dispatch(running, true);
    const result = client.getQueryData(options.queryKey);
    assert.deepEqual(result.messages, [finished]);
    assert.equal(result.activeLeafId, "another-branch");
    // Live updates can still reopen a message when a turn is retried.
    dispatch(running);
    assert.deepEqual(client.getQueryData(options.queryKey).messages, [running]);
  }
});

test("write invalidation stays scoped and excludes immutable commit bodies", async () => {
  const current = workspace.workspaceScope();
  const key = (family, ...args) => [...current, family, ...args];
  const entries = [
    key("getSkills"), key("listUserSkills"), key("getComputeSettings"),
    key("getProjectFile", "p", "x", {}), key("getProjectFile", "other", "x", {}),
    key("getProjectFile", "p", "x", { ref: "a".repeat(40) }),
    key("getRunDiff", "r"), key("getExperimentDiff", "e"),
  ];
  for (const entry of entries) client.setQueryData(entry, {});
  client.setQueryData(key("listRuns", "p"), [{ id: "r" }]);
  client.setQueryData(key("listExperiments", "p"), [{ id: "e" }]);
  const invalid = (index) => client.getQueryState(entries[index]).isInvalidated;
  const { invalidateWrite } = invalidation;
  invalidateWrite("/api/projects/p/open", current);
  assert(entries.every((_, i) => !invalid(i)));
  invalidateWrite("/api/projects/p/file", current);
  await settle();
  assert.equal(invalid(3), true);
  assert.equal(invalid(4), false);
  assert.equal(invalid(5), false);
  assert.equal(invalid(6), true);
  assert.equal(invalid(7), true);
  invalidateWrite("/api/user-skills?name=example", current);
  await settle();
  assert.equal(invalid(0), true);
  assert.equal(invalid(1), true);
  invalidateWrite("/api/settings/ssh/config", current);
  await settle();
  assert.equal(invalid(2), true);
});

test("a settings write cancels a cold imperative read before it can cache old readiness", async (t) => {
  const pending = deferred();
  let signal;
  backend({ "/api/settings/compute": (init) => { signal = init.signal; return pending.promise; } });
  const options = queries.getComputeSettingsQuery();
  const read = client.fetchQuery(options);
  const rejected = assert.rejects(read);
  invalidation.invalidateWrite("/api/settings/hf", workspace.workspaceScope());
  assert.equal(signal.aborted, true);
  pending.resolve({ configured: false });
  await rejected;
  await settle();
  assert.equal(client.getQueryData(options.queryKey), undefined);
});

test("write cancellation preserves the observed success state during refetch", async (t) => {
  const pending = deferred();
  backend({ "/api/settings/compute": () => pending.promise });
  const options = queries.getComputeSettingsQuery();
  client.setQueryData(options.queryKey, { configured: true });
  const observer = new QueryObserver(client, options);
  const errors = [];
  const off = observer.subscribe((result) => { if (result.isError) errors.push(result.error); });
  const read = client.fetchQuery({ ...options, staleTime: 0 });
  invalidation.invalidateWrite("/api/settings/hf", workspace.workspaceScope());
  await settle();
  assert.equal(observer.getCurrentResult().data.configured, true);
  assert.deepEqual(errors, []);
  pending.resolve({ configured: true });
  await read;
  await settle();
  off();
});

test("a turn sent during a cold history load survives older server history", async (t) => {
  const pending = deferred();
  const historical = { id: "history", role: "user", parts: [], createdAt: 1 };
  const snapshot = { messages: [historical], queued: [], activeLeafId: "history" };
  let calls = 0;
  backend({ "/api/chat/sessions/s/messages": () => ++calls === 1 ? pending.promise : snapshot });
  const options = queries.getChatMessagesQuery("s");
  const read = client.fetchQuery(options);
  chatStore.dispatchChat("p", { type: "optimisticUser", sessionId: "s", text: "new", attachments: [], annotations: [] });
  const local = client.getQueryData(options.queryKey).activeLeafId;
  pending.resolve(snapshot);
  const result = await read;
  assert.deepEqual(result.messages.map((message) => message.id), ["history", local]);
  assert.equal(result.activeLeafId, local);
});


test("sidebar pages preserve indicators arriving during cold and subsequent page reads", async () => {
  await render(createElement(function Stream() { workspace.events.useOrxEventStream({ onRun() {} }); return null; }), workspace);
  const emit = (type, data) => streams[0].dispatchEvent(new MessageEvent(type, { data: JSON.stringify(data) }));
  const requests = [];
  backend({ "/api/chat/sessions?scope=sidebar": () => { const request = deferred(); requests.push(request); return request.promise; } });

  for (const cursor of [null, { updatedAt: 10, id: "older" }]) {
    const pending = queries.readSidebarChatPage("active", cursor, new AbortController().signal);
    await settle();
    await act(() => {
      emit("chat.busy", { sessionId: "chat", busy: true });
      emit("chat.busy", { sessionId: "chat", busy: false });
      emit("chat.usage", { sessionId: "chat", usage: { tokens: 42 } });
    });
    requests.at(-1).resolve({ sessions: [{ id: "chat", busy: true }, { id: "other", busy: true }], next: cursor });
    const page = await pending;
    assert.equal(page.sessions[0].busy, false);
    assert.deepEqual(page.sessions[0].contextUsage, { tokens: 42 });
    assert.equal(page.sessions[1].busy, true);
    assert.deepEqual(page.next, cursor);
  }
  const pending = queries.readSidebarChatPage("active", null, new AbortController().signal);
  await settle();
  requests.at(-1).resolve(Promise.reject(new Error("offline")));
  await assert.rejects(pending, /offline/);
});

test("account queries read the owning laptop's cache inside another machine's page", async () => {
  const remote = createWorkspace("remote");
  const key = workspace.workspaceKey("getOpenResearchAccount");
  client.setQueryData(key, { loggedIn: true, user: { id: "owner", firstName: "Laptop account" } });
  remote.queryClient.setQueryData(key, { loggedIn: true, user: { id: "wrong", firstName: "Wrong cache" } });
  const { container } = await render(createElement(function Identity() {
    return createElement("span", null, queries.useOpenResearchAccount().data?.user?.firstName);
  }), remote);
  assert.equal(container.textContent, "Laptop account");
});


test("session updates preserve a background chat's pending input summary", async () => {
  const key = queries.listChatSessionsQuery("p").queryKey;
  client.setQueryData(key, [{ id: "waiting", projectId: "p", updatedAt: 1, awaitingInput: true }]);
  await render(createElement(QueryEvents), workspace);
  await act(() => streams[0].dispatchEvent(new MessageEvent("chat.session", { data: JSON.stringify({ session: { id: "waiting", projectId: "p", title: "New title", updatedAt: 2 } }) })));
  assert.equal(client.getQueryData(key)[0].awaitingInput, true);
  assert.equal(client.getQueryData(key)[0].title, "New title");
});


test("a background input summary survives live events during both list reads", async () => {
  const requests = [];
  backend({ "/api/chat/sessions?projectId=p": () => { const request = deferred(); requests.push(request); return request.promise; } });
  const options = queries.listChatSessionsQuery("p");
  client.setQueryData(options.queryKey, [{ id: "s", projectId: "p", awaitingInput: false }]);
  const read = client.fetchQuery({ ...options, staleTime: 0 });
  live.markLiveUpdate(client, options.queryKey, "s");
  requests[0].resolve({ sessions: [{ id: "s", projectId: "p", awaitingInput: false }] });
  await settle();
  live.markLiveUpdate(client, options.queryKey, "s");
  client.setQueryData(options.queryKey, [{ id: "s", projectId: "p", awaitingInput: false, contextUsage: { usedTokens: 42 } }]);
  requests[1].resolve({ sessions: [{ id: "s", projectId: "p", awaitingInput: true }] });
  const rows = await read;
  assert.equal(rows[0].awaitingInput, true);
  assert.equal(rows[0].contextUsage.usedTokens, 42);
});


for (const event of ["open", "resync.required"]) test(`${event} repairs before listeners start fresh reads`, async () => {
  backend({ "/api/projects": () => ({ projects: [{ id: "post-gap" }] }) });
  const options = queries.listProjectsQuery();
  client.setQueryData(options.queryKey, [{ id: "before-gap" }]);
  await render(createElement(QueryEvents), workspace);
  let listenerRead;
  const off = workspace.events.onChatEvent((event) => {
    if (event.type === "reconnected") listenerRead = client.fetchQuery({ ...options, staleTime: 0 });
  });
  await act(() => {
    if (event === "open") { streams[0].onopen(); streams[0].onopen(); }
    else streams[0].dispatchEvent(new MessageEvent(event, { data: "{}" }));
  });
  assert.equal((await listenerRead)[0].id, "post-gap");
  off();
});
