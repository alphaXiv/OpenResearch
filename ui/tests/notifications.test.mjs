import assert from "node:assert/strict";
import test from "node:test";
import { newPendingPrompts, runJustFinished } from "../src/notifications.ts";
import { ChatCompletionTracker } from "../src/chatCompletion.ts";
import { notify, requestNotificationPermissionOnce } from "../src/notifications.ts";

const run = (id, status, cancelRequested = false) => ({ id, status, cancelRequested });
const prompt = (id, resolved = false) => ({ id, type: "prompt", prompt: { kind: "permission", resolved } });
const message = (id, ...parts) => ({ id, role: "assistant", parts, createdAt: 0 });

test("a run counts as finished only on a live to done or failed transition", () => {
  assert.equal(runJustFinished(run("a", "done")), false, "first sighting is the connect snapshot");
  assert.equal(runJustFinished(run("b", "starting")), false);
  assert.equal(runJustFinished(run("b", "running")), false);
  assert.equal(runJustFinished(run("b", "done")), true);
  assert.equal(runJustFinished(run("b", "done")), false, "an unchanged status never repeats");
  assert.equal(runJustFinished(run("c", "running")), false);
  assert.equal(runJustFinished(run("c", "failed")), true);
});

test("a cancelled run never counts as finished", () => {
  assert.equal(runJustFinished(run("d", "running")), false);
  assert.equal(runJustFinished(run("d", "cancelled", true)), false);
  assert.equal(runJustFinished(run("e", "running", true)), false);
  assert.equal(runJustFinished(run("e", "failed", true)), false);
});

test("pending prompts are reported once and skip resolved cards and other parts", () => {
  const text = { id: "t", type: "text", text: "hello" };
  assert.deepEqual(newPendingPrompts("s1", message("m1", text, prompt("p1", true))), []);
  assert.deepEqual(newPendingPrompts("s1", message("m1", text, prompt("p2"))).map((part) => part.id), ["p2"]);
  assert.deepEqual(newPendingPrompts("s1", message("m1", text, prompt("p2"))), [], "streaming re-sends the same card");
  assert.deepEqual(newPendingPrompts("s1", message("m2", prompt("p2"))).map((part) => part.id), ["p2"], "keyed per message");
});

const session = (id, extra = {}) => ({ id, projectId: "p", busy: false, activeLeafId: `${id}-reply`, createdAt: 10, updatedAt: 20, ...extra });

test("initial history is quiet; a running chat in another project is tracked", () => {
  const tracker = new ChatCompletionTracker();
  assert.deepEqual(tracker.snapshot([session("old"), session("live", { busy: true, projectId: "other" })]), []);
  assert.deepEqual(tracker.snapshot([session("old"), session("live", { projectId: "other", updatedAt: 30 })]).map(s => s.id), ["live"]);
  assert.equal(tracker.completionAfter.get("live"), 10, "use server time, not the browser's clock");
  assert.deepEqual(tracker.snapshot([session("old"), session("live", { projectId: "other", updatedAt: 30 })]), []);
});

test("reconnect finds changed replies and new sessions without announcing renames", () => {
  const tracker = new ChatCompletionTracker();
  tracker.snapshot([session("a")]);
  assert.equal(tracker.update(session("a", { title: "renamed" })), false);
  assert.equal(tracker.update(session("a", { activeLeafId: "second", updatedAt: 40 })), true);
  assert.equal(tracker.update(session("new", { createdAt: 30, updatedAt: 50 })), true);
  assert.equal(tracker.completionAfter.get("new"), 30);
});

test("branch selection is a new baseline, while a subsequent real turn still completes", () => {
  const tracker = new ChatCompletionTracker();
  tracker.snapshot([session("a")]);
  tracker.branch("a", "historical");
  assert.equal(tracker.update(session("a", { activeLeafId: "historical" })), false);
  assert.equal(tracker.completionAfter.get("a"), Infinity);
  tracker.busy("a");
  assert.equal(tracker.update(session("a", { activeLeafId: "fresh", updatedAt: 50 })), true);
  assert.equal(tracker.completionAfter.get("a"), 10);
});

function browser(t) {
  const old = new Map(["window", "document", "navigator", "Notification", "localStorage"].map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  const values = new Map(); const shown = [];
  let queue = Promise.resolve();
  const state = { fail: false };
  class Notification {
    static permission = "granted";
    constructor(title, options) { if (state.fail) throw new Error("blocked"); shown.push({ title, options, notification: this }); }
    close() {}
  }
  const globals = {
    window: Object.assign(new EventTarget(), { isSecureContext: true, focus() {} }),
    document: { visibilityState: "visible", hasFocus: () => true },
    navigator: { locks: { request: (_key, fn) => (queue = queue.then(fn)) } },
    Notification,
    localStorage: { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value) },
  };
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, writable: true, value });
  t.after(() => { for (const [key, descriptor] of old) { if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key]; } });
  return { shown, values, state };
}

test("enabled notifications fire even while the dashboard is focused", async t => {
  const { shown } = browser(t);
  assert.equal(await notify("turns", "focused", "Done", "Chat", () => {}), true);
  assert.equal(shown.length, 1);
});

test("two independent tabs and a later reload share one delivery ledger", async t => {
  const { shown } = browser(t);
  const secondTab = await import("../src/notifications.ts?tab=second");
  const args = ["turns", "cross-tab", "Done", "Chat", () => {}];
  const result = await Promise.all([notify(...args), secondTab.notify(...args)]);
  assert.deepEqual(result.sort(), [false, true]);
  const reloaded = await import("../src/notifications.ts?tab=reloaded");
  assert.equal(await reloaded.notify(...args), false);
  assert.equal(shown.length, 1);
});

test("permission denial and failed construction do not consume a completion", async t => {
  const { shown, state } = browser(t);
  Notification.permission = "denied";
  assert.equal(await notify("turns", "retryable", "Done", "", () => {}), false);
  Notification.permission = "granted";
  state.fail = true;
  assert.equal(await notify("turns", "retryable", "Done", "", () => {}), false);
  state.fail = false;
  assert.equal(await notify("turns", "retryable", "Done", "", () => {}), true);
  assert.equal(shown.length, 1);
});

test("a delivery waiting for another tab's lock rechecks its workspace", async t => {
  const { shown } = browser(t);
  let release;
  const blocked = new Promise(resolve => { release = resolve; });
  navigator.locks.request = async (_key, fn) => { await blocked; return fn(); };
  let valid = true;
  const result = notify("turns", "old-workspace", "Done", "", () => {}, () => valid);
  valid = false; release();
  assert.equal(await result, false);
  assert.equal(shown.length, 0);
});

test("permission is requested once from the next trusted gesture", async t => {
  const old = new Map(["window", "Notification", "localStorage"].map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  const values = new Map();
  const listeners = new Map();
  let requests = 0;
  class Notification {
    static permission = "default";
    static requestPermission() { requests += 1; this.permission = "granted"; return Promise.resolve(this.permission); }
  }
  const target = new EventTarget();
  const fakeWindow = Object.assign(target, {
    isSecureContext: true,
    addEventListener(type, fn, capture) { listeners.set(type, fn); EventTarget.prototype.addEventListener.call(this, type, fn, capture); },
    removeEventListener(type, fn, capture) { listeners.delete(type); EventTarget.prototype.removeEventListener.call(this, type, fn, capture); },
  });
  const globals = {
    window: fakeWindow,
    Notification,
    localStorage: { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value) },
  };
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, writable: true, value });
  t.after(() => { for (const [key, descriptor] of old) { if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key]; } });
  requestNotificationPermissionOnce();
  assert.equal(requests, 0, "loading does not prompt");
  listeners.get("pointerdown")({ isTrusted: false });
  assert.equal(requests, 0, "a synthetic event does not prompt");
  listeners.get("pointerdown")({ isTrusted: true });
  await Promise.resolve();
  assert.equal(requests, 1);
  assert.equal(values.get("orx:notification-permission-asked"), "1");
  assert.equal(listeners.has("keydown"), false, "one gesture retires both listeners");
  requestNotificationPermissionOnce();
  assert.equal(requests, 1, "a second call does not ask again");
});

test("an already delivered notification can be clicked after a follow-up starts", async t => {
  const { shown } = browser(t);
  let idle = true; let opened = false;
  await notify("turns", "click-after-followup", "Done", "", () => { opened = true; }, () => idle);
  idle = false;
  shown[0].notification.onclick();
  assert.equal(opened, true);
});
