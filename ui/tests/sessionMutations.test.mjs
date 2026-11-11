import assert from "node:assert/strict";
import test from "node:test";
import { mutPatchChatSession } from "../src/queries/chat.ts";
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
