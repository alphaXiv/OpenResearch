import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { SubagentTab } from "../src/components/SubagentTab.tsx";
import { render, serve, settle } from "./render.mjs";

test("a transcript that finishes loading after its messages attaches its growth observer", async (t) => {
  const observers = [];
  t.mock.property(globalThis, "ResizeObserver", class {
    constructor(callback) { this.callback = callback; this.observed = []; observers.push(this); }
    observe(element) { this.observed.push(element); }
    disconnect() {}
  });
  let finishTranscript;
  serve((path) => {
    if (path === "/api/chat/sessions/s/messages") return { messages: [], queued: [], activeLeafId: null };
    if (path.startsWith("/api/chat/sessions/s/subagent?")) return new Promise((resolve) => { finishTranscript = resolve; });
  });
  await render(createElement(SubagentTab, { projectId: "p", sessionId: "s", spawnPartId: "spawn", expansionState: new Map() }));
  await settle();
  assert.deepEqual(observers, []);

  finishTranscript({ messages: [], state: { status: "completed" } });
  await settle();
  assert.equal(observers.length, 1);
  const [scroll] = observers[0].observed.slice(-1);
  Object.defineProperty(scroll, "scrollHeight", { value: 1000, configurable: true });
  await act(() => observers[0].callback());
  assert.equal(scroll.scrollTop, 1000);
});
