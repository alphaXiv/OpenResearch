import test from "node:test";
import assert from "node:assert/strict";
import { QueryObserver } from "@tanstack/react-query";
import { queryModules } from "./queryModules.mjs";

const deferred = () => { let resolve; const promise = new Promise((r) => { resolve = r; }); return { promise, resolve }; };
const settle = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };

function setup(t, api) {
  const modules = queryModules(api, { nativeClient: true });
  t.after(() => modules.client.clear());
  return modules;
}

test("components and imperative readers deduplicate and reuse fresh data", async (t) => {
  let calls = 0;
  const pending = deferred();
  const { client, load } = setup(t, { listProjects: () => { calls++; return pending.promise; } });
  const options = load("projects").listProjectsQuery();
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

test("ordinary data survives revisits and expires ten minutes after its last observer", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout", "Date"], now: 10_000 });
  const { client, load } = setup(t, { listProjects: async () => [{ id: "p" }] });
  const options = load("projects").listProjectsQuery();
  const observer = new QueryObserver(client, options);
  let off = observer.subscribe(() => {});
  await settle();
  off();
  t.mock.timers.tick(599_999);
  assert.ok(client.getQueryData(options.queryKey));
  off = observer.subscribe(() => {});
  assert.equal(observer.getCurrentResult().data[0].id, "p");
  await settle();
  off();
  t.mock.timers.tick(599_999);
  assert.ok(client.getQueryData(options.queryKey));
  t.mock.timers.tick(1);
  assert.equal(client.getQueryData(options.queryKey), undefined);
});

test("stream updates do not pin inactive transcripts or recreate expired snapshots", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout", "Date"], now: 10_000 });
  const snapshot = { messages: [], queued: [], activeLeafId: null };
  const { client, load } = setup(t, { getChatMessages: async () => snapshot });
  const options = load("chat").getChatMessagesQuery("s");
  await client.fetchQuery(options);
  t.mock.timers.tick(300_000);
  client.setQueryData(options.queryKey, (data) => data && { ...data, activeLeafId: "event" });
  t.mock.timers.tick(300_000);
  assert.equal(client.getQueryData(options.queryKey), undefined);
  client.setQueryData(options.queryKey, (data) => data && { ...data, activeLeafId: "late-event" });
  assert.equal(client.getQueryCache().find({ queryKey: options.queryKey }), undefined);
});

test("freshness is query-specific and immutable bodies still use finite retention", (t) => {
  const { client, load } = setup(t, {});
  const projects = load("projects");
  const files = load("files");
  assert.equal(client.getDefaultOptions().queries.gcTime, 600_000);
  assert.equal(client.getDefaultOptions().queries.retry, false);
  assert.equal(projects.listProjectsQuery().staleTime, 30_000);
  assert.equal(projects.getUiStateQuery().staleTime, 300_000);
  assert.equal(projects.resolvePaperQuery("id").staleTime, 3_600_000);
  assert.equal(files.getArtifactFileMetadataQuery("p", "x").staleTime, 2_000);
  assert.equal(files.getProjectFileQuery("p", "x").staleTime, Infinity);
  assert.equal(files.getProjectFileQuery("p", "x").refetchOnMount, "always");
  assert.equal(files.getProjectFileQuery("p", "x", { ref: "main" }).staleTime, 30_000);
  assert.equal(files.getProjectFileQuery("p", "x", { ref: "a".repeat(40) }).staleTime, Infinity);
  assert.equal(load("chat").getChatMessagesQuery("s").refetchOnWindowFocus, "always");
  assert.equal(load("settings").getSshMasterStatusQuery("host").staleTime, 5_000);
});

test("replacement cancels reads, clears old data, and preserves temporary reconnect identity", async (t) => {
  const pending = deferred();
  let signal;
  const { client, load } = setup(t, { listProjects: (s) => { signal = s; return pending.promise; } });
  const scope = load("client");
  scope.activateWorkspace("ssh:one");
  const old = load("projects").listProjectsQuery();
  const read = client.fetchQuery(old).catch(() => null);
  assert.equal(scope.activateWorkspace("ssh:one"), false);
  assert.equal(signal.aborted, false);
  scope.activateWorkspace("local");
  assert.equal(signal.aborted, true);
  pending.resolve([{ id: "old" }]);
  await read;
  assert.equal(client.getQueryData(old.queryKey), undefined);
  assert.notDeepEqual(old.queryKey, load("projects").listProjectsQuery().queryKey);
  const generation = scope.getWorkspaceGeneration();
  scope.replaceWorkspace();
  assert.equal(scope.getWorkspaceGeneration(), generation + 1);
});

test("snapshot repair is bounded while preserving events during the second read", async (t) => {
  const requests = [deferred(), deferred()];
  let calls = 0;
  const { client, load } = setup(t, { listProjects: () => requests[calls++].promise });
  const options = load("projects").listProjectsQuery();
  const read = client.fetchQuery(options);
  load("live").markLiveUpdate(client, options.queryKey, "p");
  requests[0].resolve([{ id: "p", title: "old" }]);
  await settle();
  assert.equal(calls, 2);
  load("live").markLiveUpdate(client, options.queryKey, "p");
  client.setQueryData(options.queryKey, [{ id: "p", title: "streamed" }]);
  requests[1].resolve([{ id: "p", title: "intermediate" }]);
  assert.deepEqual(await read, [{ id: "p", title: "streamed" }]);
  assert.equal(calls, 2);
});

test("session deletion during a read cannot resurrect its row", async (t) => {
  const pending = deferred();
  const { client, load } = setup(t, { listChatSessions: () => pending.promise });
  const options = load("chat").listChatSessionsQuery("p");
  const read = client.fetchQuery(options);
  load("invalidation").removeSession("s");
  pending.resolve([{ id: "s", projectId: "p" }]);
  assert.deepEqual(await read, []);
});

test("live bounded previews refresh their existing source key without retaining versions", async (t) => {
  let calls = 0;
  const { client, load } = setup(t, { getProjectFile: async () => ({ path: "x", content: String(++calls), notFound: false }) });
  const files = load("files");
  const options = files.resolvedFileQuery("p", "x", "repo");
  assert.equal((await client.fetchQuery(options)).file.content, "1");
  await client.invalidateQueries(options);
  assert.equal((await client.fetchQuery(options)).file.content, "2");
  assert.equal(client.getQueryCache().getAll().length, 2);
});

test("chat projection renders a cold route without waiting for the session catalog", (t) => {
  const { client, load } = setup(t, {});
  const snapshot = { messages: [{ id: "m" }], queued: [], activeLeafId: "m" };
  client.setQueryData(load("chat").getChatMessagesQuery("s").queryKey, snapshot);
  client.setQueryData(load("chat").getChatMessagesQuery("hidden").queryKey, snapshot);
  const state = load("chatStore").readChatState("p", "s");
  assert.deepEqual(Object.keys(state.messagesBySession), ["s"]);
  assert.equal(state.messagesBySession.s, snapshot.messages);
});

test("chat subscriptions ignore observer option changes and pending reads", (t) => {
  const { client, load } = setup(t, {});
  const options = load("chat").getChatMessagesQuery("s");
  const store = load("chatStore");
  let notifications = 0;
  const off = store.subscribeChat(() => notifications++);
  const observer = new QueryObserver(client, { ...options, enabled: false });
  observer.setOptions({ ...options, enabled: false });
  assert.equal(notifications, 0);
  client.setQueryData(options.queryKey, { messages: [], queued: [], activeLeafId: null });
  assert.equal(notifications, 1);
  off();
});

test("retiring a scope rejects a refresh even when it had cached data", async (t) => {
  const pending = deferred();
  const { client, load } = setup(t, { listProjects: () => pending.promise });
  const options = load("projects").listProjectsQuery();
  client.setQueryData(options.queryKey, [{ id: "old" }]);
  const read = client.fetchQuery({ ...options, staleTime: 0 });
  const rejected = assert.rejects(read);
  load("client").activateWorkspace("other");
  pending.resolve([{ id: "late" }]);
  await rejected;
  await settle();
  assert.equal(client.getQueryData(options.queryKey), undefined);
});

test("cold chat repair preserves message order, queue, and branch events without a third read", async (t) => {
  const requests = [deferred(), deferred()];
  let calls = 0;
  const { client, load } = setup(t, { getChatMessages: () => requests[calls++].promise });
  const options = load("chat").getChatMessagesQuery("s");
  const store = load("chatStore");
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
  const { client, load } = setup(t, { getChatMessages: async () => snapshot });
  const options = load("chat").getChatMessagesQuery("s");
  client.setQueryData(options.queryKey, snapshot);
  load("chatStore").dispatchChat("p", { type: "optimisticUser", sessionId: "s", text: "next", attachments: [], annotations: [] });
  const localId = client.getQueryData(options.queryKey).activeLeafId;
  const result = await client.fetchQuery({ ...options, staleTime: 0 });
  assert.deepEqual(result.messages.map((message) => message.id), ["server", localId]);
  assert.equal(result.activeLeafId, localId);
});

test("write invalidation stays scoped and excludes immutable commit bodies", async (t) => {
  const { client, load } = setup(t, {});
  const scope = load("client").workspaceScope();
  const key = (family, ...args) => [...scope, family, ...args];
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
  const { invalidateWrite } = load("invalidation");
  invalidateWrite("/api/projects/p/open", scope);
  assert(entries.every((_, i) => !invalid(i)));
  invalidateWrite("/api/projects/p/file", scope);
  await settle();
  assert.equal(invalid(3), true);
  assert.equal(invalid(4), false);
  assert.equal(invalid(5), false);
  assert.equal(invalid(6), true);
  assert.equal(invalid(7), true);
  invalidateWrite("/api/user-skills?name=example", scope);
  await settle();
  assert.equal(invalid(0), true);
  assert.equal(invalid(1), true);
  invalidateWrite("/api/settings/ssh/config", scope);
  await settle();
  assert.equal(invalid(2), true);
});

test("a settings write cancels a cold imperative read before it can cache old readiness", async (t) => {
  const pending = deferred();
  let signal;
  const { client, load } = setup(t, { getComputeSettings: (_project, s) => { signal = s; return pending.promise; } });
  const options = load("settings").getComputeSettingsQuery();
  const read = client.fetchQuery(options);
  const rejected = assert.rejects(read);
  load("invalidation").invalidateWrite("/api/settings/hf", load("client").workspaceScope());
  assert.equal(signal.aborted, true);
  pending.resolve({ configured: false });
  await rejected;
  await settle();
  assert.equal(client.getQueryData(options.queryKey), undefined);
});

test("write cancellation preserves the observed success state during refetch", async (t) => {
  const pending = deferred();
  const { client, load } = setup(t, { getComputeSettings: () => pending.promise });
  const options = load("settings").getComputeSettingsQuery();
  client.setQueryData(options.queryKey, { configured: true });
  const observer = new QueryObserver(client, options);
  const errors = [];
  const off = observer.subscribe((result) => { if (result.isError) errors.push(result.error); });
  const read = client.fetchQuery({ ...options, staleTime: 0 });
  load("invalidation").invalidateWrite("/api/settings/hf", load("client").workspaceScope());
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
  const { client, load } = setup(t, { getChatMessages: () => ++calls === 1 ? pending.promise : Promise.resolve(snapshot) });
  const options = load("chat").getChatMessagesQuery("s");
  const read = client.fetchQuery(options);
  load("chatStore").dispatchChat("p", { type: "optimisticUser", sessionId: "s", text: "new", attachments: [], annotations: [] });
  const local = client.getQueryData(options.queryKey).activeLeafId;
  pending.resolve(snapshot);
  const result = await read;
  assert.deepEqual(result.messages.map((message) => message.id), ["history", local]);
  assert.equal(result.activeLeafId, local);
});
