import assert from "node:assert/strict";
import test from "node:test";
import { applySidebarCards, mutDeleteChatSession, mutPatchChatSession } from "../src/queries/chat.ts";
import { createWorkspace } from "../src/workspace.tsx";
import { serve } from "./render.mjs";

const deferred = () => { let resolve; const promise = new Promise((r) => { resolve = r; }); return { promise, resolve }; };
const settle = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };
const row = (id, title) => ({ id, projectId: "p", title, archived: false, autonomy: "agentic", planMode: false, updatedAt: 1 });

function setup() {
  const workspace = createWorkspace("mutations");
  const options = workspace.queries.listChatSessionsQuery("p");
  workspace.queryClient.setQueryData(options.queryKey, [row("s", "Old"), row("t", "Other")]);
  const reply = deferred();
  const requests = [];
  serve((path, init) => {
    requests.push(`${init.method ?? "GET"} ${path}`);
    if (init.method === "PATCH") return reply.promise;
    if (path.startsWith("/api/chat/sessions?")) return { sessions: [row("s", "Old"), row("t", "Other")] };
  });
  const titles = () => workspace.queryClient.getQueryData(options.queryKey).map((session) => session.title);
  return { workspace, reply, requests, titles };
}

test("a chat setting shows at once and keeps the server's row", async () => {
  const { workspace, reply, titles } = setup();
  const saved = mutPatchChatSession.runAsHeadlessPromise(workspace, row("s", "Old"), { title: "New" }, {});
  assert.deepEqual(titles(), ["New", "Other"]);
  reply.resolve({ session: row("s", "New (trimmed)") });
  await saved;
  assert.deepEqual(titles(), ["New (trimmed)", "Other"]);
});

test("a refused chat setting rolls back and reports the server's message", async () => {
  const { workspace, reply, titles } = setup();
  const saved = mutPatchChatSession.runAsHeadlessPromise(workspace, row("s", "Old"), { title: " " }, {});
  reply.resolve(Response.json({ error: "A chat needs a title." }, { status: 400 }));
  await assert.rejects(saved, { message: "A chat needs a title." });
  assert.deepEqual(titles(), ["Old", "Other"]);
});

test("a message sent after a setting waits for it", async () => {
  const { workspace, reply, requests } = setup();
  const saved = mutPatchChatSession.runAsHeadlessPromise(workspace, row("s", "Old"), { permissionMode: "auto" }, {});
  const sent = workspace.chat.serialize(async () => { requests.push("send"); });
  await settle();
  assert.deepEqual(requests, ["PATCH /api/chat/sessions/s"]);
  reply.resolve({ session: { ...row("s", "Old"), permissionMode: "auto" } });
  await Promise.all([saved, sent]);
  assert.deepEqual(requests, ["PATCH /api/chat/sessions/s", "send"]);
});

test("a message sent while two settings are pending waits for both", async () => {
  const { workspace, reply, requests } = setup();
  const first = mutPatchChatSession.runAsHeadlessPromise(workspace, row("s", "Old"), { permissionMode: "auto" }, {});
  const second = mutPatchChatSession.runAsHeadlessPromise(workspace, row("s", "Old"), { autonomy: "copilot" }, {});
  const sent = workspace.chat.serialize(async () => { requests.push("send"); });
  reply.resolve({ session: row("s", "Old") });
  await Promise.all([first, second, sent]);
  assert.deepEqual(requests, ["PATCH /api/chat/sessions/s", "PATCH /api/chat/sessions/s", "send"]);
});

test("a refused setting still lets the next message through", async () => {
  const { workspace, reply, requests } = setup();
  const first = mutPatchChatSession.runAsHeadlessPromise(workspace, row("s", "Old"), { title: " " }, {});
  const second = mutPatchChatSession.runAsHeadlessPromise(workspace, row("s", "Old"), { autonomy: "copilot" }, {});
  const sent = workspace.chat.serialize(async () => { requests.push("send"); });
  reply.resolve(Response.json({ error: "A chat needs a title." }, { status: 400 }));
  await assert.rejects(first);
  await assert.rejects(second);
  await sent;
  assert.deepEqual(requests, ["PATCH /api/chat/sessions/s", "send"]);
});

function sidebar(workspace, done) {
  const lists = { active: [row("s", "Old"), { ...row("t", "Other"), updatedAt: 0 }], archived: done };
  for (const [filter, sessions] of Object.entries(lists)) {
    const { queryKey } = workspace.queries.listSidebarChatSessionsQuery(filter);
    workspace.queryClient.setQueryData(queryKey, { pages: [{ sessions, next: null, total: sessions.length }], pageParams: [null] });
  }
  return (filter) => {
    const [page] = workspace.queryClient.getQueryData(workspace.queries.listSidebarChatSessionsQuery(filter).queryKey).pages;
    return { ids: page.sessions.map((session) => session.id), total: page.total };
  };
}

test("marking a chat done moves it from the inbox to Done at once", async () => {
  const { workspace, reply } = setup();
  const list = sidebar(workspace, [{ ...row("newer", "Newer"), archived: true, updatedAt: 2 }, { ...row("older", "Older"), archived: true, updatedAt: 0 }]);
  const saved = mutPatchChatSession.runAsHeadlessPromise(workspace, row("s", "Old"), { archived: true }, {});
  assert.deepEqual(list("active"), { ids: ["t"], total: 1 });
  assert.deepEqual(list("archived"), { ids: ["newer", "s", "older"], total: 3 });
  reply.resolve({ session: { ...row("s", "Old"), archived: true } });
  await saved;
});

test("a refused mark-done puts the chat back in the inbox", async () => {
  const { workspace, reply } = setup();
  const list = sidebar(workspace, []);
  const saved = mutPatchChatSession.runAsHeadlessPromise(workspace, row("s", "Old"), { archived: true }, {});
  reply.resolve(Response.json({ error: "The chat is gone." }, { status: 404 }));
  await assert.rejects(saved, { message: "The chat is gone." });
  assert.deepEqual(list("active"), { ids: ["s", "t"], total: 2 });
  assert.deepEqual(list("archived"), { ids: [], total: 0 });
});

test("a chat marked done that sorts past Done's loaded pages counts without showing", async () => {
  const { workspace, reply } = setup();
  const list = sidebar(workspace, [{ ...row("newer", "Newer"), archived: true, updatedAt: 2 }]);
  const { queryKey } = workspace.queries.listSidebarChatSessionsQuery("archived");
  workspace.queryClient.setQueryData(queryKey, (data) => ({ ...data, pages: [{ ...data.pages[0], next: { updatedAt: 2, id: "newer" }, total: 5 }] }));
  const saved = mutPatchChatSession.runAsHeadlessPromise(workspace, row("s", "Old"), { archived: true }, {});
  assert.deepEqual(list("archived"), { ids: ["newer"], total: 6 });
  reply.resolve({ session: { ...row("s", "Old"), archived: true } });
  await saved;
});

test("a setting on a chat the sidebar does not list leaves the sidebar alone, also when refused", async () => {
  const { workspace, reply } = setup();
  const list = sidebar(workspace, []);
  const { queryKey } = workspace.queries.listSidebarChatSessionsQuery("active");
  // A side chat sits in its project's list but not the sidebar's.
  workspace.queryClient.setQueryData(queryKey, (data) => ({ ...data, pages: [{ ...data.pages[0], sessions: data.pages[0].sessions.filter((session) => session.id !== "t") }] }));
  const saved = mutPatchChatSession.runAsHeadlessPromise(workspace, row("t", "Other"), { archived: true }, {});
  assert.deepEqual(list("active").ids, ["s"]);
  reply.resolve(Response.json({ error: "Nope." }, { status: 400 }));
  await assert.rejects(saved);
  assert.deepEqual(list("active").ids, ["s"]);
  assert.deepEqual(list("archived").ids, []);
});

test("a move in flight on one machine leaves another machine's counts alone", async () => {
  const { workspace, reply } = setup();
  sidebar(workspace, []);
  const other = createWorkspace("other");
  const otherList = sidebar(other, []);
  const saved = mutPatchChatSession.runAsHeadlessPromise(workspace, row("s", "Old"), { archived: true }, {});
  const scope = other.queries.listSidebarChatSessionsQuery("active").queryKey.slice(0, -2);
  applySidebarCards(other.queryClient, scope, { type: "sidebar", cards: [{ sessionId: "t", session: row("t", "Other"), atWork: false }], totals: { active: 2, archived: 0 } });
  assert.deepEqual(otherList("archived").total, 0);
  reply.resolve({ session: { ...row("s", "Old"), archived: true } });
  await saved;
});

test("deleting a chat re-reads neither list, and a list a resync invalidated still waits for its read", async () => {
  const { workspace } = setup();
  const list = sidebar(workspace, [{ ...row("old", "Old"), archived: true }]);
  serve((_, init) => init.method === "DELETE" ? { ok: true } : undefined);
  const state = (filter) => workspace.queryClient.getQueryState(workspace.queries.listSidebarChatSessionsQuery(filter).queryKey);
  // A server that streams cards counts both sections on every page.
  for (const filter of ["active", "archived"]) {
    workspace.queryClient.setQueryData(workspace.queries.listSidebarChatSessionsQuery(filter).queryKey, (data) => ({ ...data, pages: [{ ...data.pages[0], totals: { active: 2, archived: 1 } }] }));
  }
  await workspace.queryClient.invalidateQueries({ queryKey: workspace.queries.listSidebarChatSessionsQuery("archived").queryKey, refetchType: "none" });
  await mutDeleteChatSession.runAsHeadlessPromise(workspace, row("s", "Old"), {});
  await settle();
  assert.deepEqual(list("active").ids, ["t"]);
  assert.equal(state("active").isInvalidated, false);
  assert.equal(state("archived").isInvalidated, true);
});
