import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import ts from "typescript";
import { ChatCompletionTracker } from "../src/chatCompletion.ts";
import * as tracking from "../src/notifications.ts";

const settle = async () => { for (let i = 0; i < 30; i++) await Promise.resolve(); };
const row = (id, extra = {}) => ({ id, projectId: "p", title: id, busy: false, activeLeafId: `${id}-old`, createdAt: 1, updatedAt: 10, ...extra });
const reply = (id, extra = {}) => ({ id, role: "assistant", completedAt: 100, createdAt: 20, parts: [{ id: "text", type: "text", text: "Done" }], ...extra });

function setup(t, initial, { initialRead } = {}) {
  t.mock.timers.enable({ apis: ["setTimeout", "setInterval", "Date"], now: 1_000_000 });
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "window", { configurable: true, value: { setTimeout, setInterval } });
  const sessions = new Map(initial.map(s => [s.id, s]));
  const messages = new Map(); const sent = []; const visited = []; const cleanups = [];
  let listener; let runListener; let current = true; let failReads = 0; let sessionReads = 0;
  const api = {
    listAllChatSessions: () => initialRead ?? Promise.resolve([...sessions.values()]),
    getChatSession: async id => { sessionReads++; if (failReads-- > 0) throw new Error("temporary disconnect"); return sessions.get(id); },
    getChatMessages: async () => ({ messages: [...messages.values()] }),
  };
  const module = { exports: {} };
  const source = readFileSync(new URL("../src/useBrowserNotifications.ts", import.meta.url), "utf8");
  const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
  const dependencies = {
    react: { useEffect: fn => { const cleanup = fn(); if (cleanup) cleanups.push(cleanup); }, useSyncExternalStore: () => "granted:turns,prompts,runs" },
    "@tanstack/react-router": { useRouter: () => ({ navigate: location => visited.push(location) }) },
    "./api": api,
    "./events": { onChatEvent: fn => { listener = fn; return () => { listener = undefined; }; }, useOrxEvents: handlers => { runListener = handlers.onRun; } },
    "./chatCompletion": { ChatCompletionTracker },
    "./notifications": { ...tracking, notificationsEnabled: () => true, notify: async (kind, tag, title, body, onClick, valid) => {
      if (!valid() || sent.some(item => item.tag === tag)) return false;
      sent.push({ kind, tag, title, body, onClick }); return true;
    } },
    "./queries/client": { workspaceScope: () => ["workspace", 1], isCurrentScope: () => current, queryClient: { getQueryData: () => undefined } },
    "./queries/projects": { listExperimentsQuery: () => ({ queryKey: [] }) },
    "./workspaceState": { taskLocation: (project, session) => `/projects/${project}/tasks/${session}` },
    "./paraglide/messages.js": { m: new Proxy({}, { get: (_target, name) => () => name }) },
  };
  new Function("require", "exports", code)(id => { assert.ok(id in dependencies, id); return dependencies[id]; }, module.exports);
  tracking.resetNotificationTracking();
  module.exports.useBrowserNotifications();
  const stop = () => { for (const cleanup of cleanups.splice(0)) cleanup(); };
  t.after(() => { stop(); if (previousWindow) Object.defineProperty(globalThis, "window", previousWindow); else delete globalThis.window; });
  const emit = event => listener?.(event);
  return { sessions, messages, sent, visited, emit, stop,
    failNextRead: () => { failReads = 1; }, reads: () => sessionReads,
    retire: () => { current = false; }, run: run => runListener(run),
    finish(id, message = reply(`${id}-new`)) {
      messages.set(message.id, message);
      const session = { ...sessions.get(id), busy: false, activeLeafId: message.id, updatedAt: 101 };
      sessions.set(id, session);
      emit({ type: "message", sessionId: id, message });
      emit({ type: "session", session });
      emit({ type: "busy", sessionId: id, busy: false });
    },
  };
}

test("a globally tracked chat finishes even if its project was never opened", async t => {
  const h = setup(t, [row("old"), row("live", { projectId: "other", busy: true })]);
  await settle(); assert.equal(h.sent.length, 0);
  h.finish("live"); t.mock.timers.tick(350); await settle();
  assert.equal(h.sent.length, 1); assert.equal(h.sent[0].tag, "turn:live:live-new");
  h.sent[0].onClick(); assert.deepEqual(h.visited, [{ href: "/projects/other/tasks/live" }]);
  h.emit({ type: "busy", sessionId: "live", busy: false }); t.mock.timers.tick(350); await settle();
  assert.equal(h.sent.length, 1);
});

test("reconnect catches completed work from snapshots using server timestamps", async t => {
  const h = setup(t, [row("live", { busy: true })]);
  await settle();
  h.sessions.set("live", row("live", { activeLeafId: "missed", updatedAt: 101 }));
  h.messages.set("missed", reply("missed"));
  h.emit({ type: "reconnected" }); await settle(); t.mock.timers.tick(350); await settle();
  assert.equal(h.sent.length, 1, "browser time is deliberately far ahead of server time");
});

test("completion edges arriving during the initial catalog read are replayed", async t => {
  let resolve; const initialRead = new Promise(r => { resolve = r; });
  const h = setup(t, [row("live", { busy: true })], { initialRead });
  h.finish("live");
  resolve([...h.sessions.values()]); await settle(); t.mock.timers.tick(350); await settle();
  assert.equal(h.sent.length, 1);
});

test("old branch navigation and pending questions are not completed turns", async t => {
  const h = setup(t, [row("s", { busy: true })]);
  await settle();
  h.finish("s"); h.emit({ type: "branch", sessionId: "s", activeLeafId: "s-new" });
  h.emit({ type: "busy", sessionId: "s", busy: false });
  t.mock.timers.tick(350); await settle(); assert.equal(h.sent.length, 0);
  h.finish("s", reply("prompt", { parts: [{ id: "question", type: "prompt", prompt: { kind: "question", resolved: false } }] }));
  t.mock.timers.tick(350); await settle();
  assert.deepEqual(h.sent.map(item => item.kind), ["prompts"]);
});

test("a queued follow-up and a retired workspace cancel pending completion checks", async t => {
  const h = setup(t, [row("s", { busy: true })]); await settle();
  h.finish("s"); h.emit({ type: "busy", sessionId: "s", busy: true });
  t.mock.timers.tick(350); await settle(); assert.equal(h.sent.length, 0);
  h.finish("s", reply("next")); h.retire();
  t.mock.timers.tick(350); await settle(); assert.equal(h.sent.length, 0);
});

test("a transient detail read failure retries the unconsumed completion", async t => {
  const h = setup(t, [row("s", { busy: true })]); await settle();
  h.failNextRead(); h.finish("s"); t.mock.timers.tick(350); await settle();
  assert.equal(h.sent.length, 0);
  t.mock.timers.tick(3000); await settle();
  assert.equal(h.sent.length, 1); assert.equal(h.reads(), 2);
});

test("running experiments outside the selected project also notify", async t => {
  const h = setup(t, []); await settle();
  h.run({ id: "run", projectId: "other", experimentId: "exp", status: "done" });
  assert.equal(h.sent.length, 0);
  h.run({ id: "new-run", projectId: "other", experimentId: "exp", status: "running" });
  h.run({ id: "new-run", projectId: "other", experimentId: "exp", status: "failed" });
  await settle(); assert.equal(h.sent.length, 1); assert.equal(h.sent[0].kind, "runs");
  h.sent[0].onClick(); assert.deepEqual(h.visited, [{ href: "/projects/other" }]);
});
