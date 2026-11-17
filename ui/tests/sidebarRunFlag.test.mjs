import assert from "node:assert/strict";
import test from "node:test";
import { m } from "../src/paraglide/messages.js";
import { renderChat, session } from "./chat.mjs";

const run = (id, monitor) => ({
  id, experimentId: `e-${id}`, projectId: "p", status: "running", createdAt: 1, updatedAt: 1, contact: null, cancelling: false, lostReason: null, host: null, backend: null,
  monitor: monitor && { tone: "flag", whole: 0.4, headline: null, sentence: null, bindings: {}, signals: {}, estimate: null, flag: monitor, rev: 1, generation: "g", asOf: 1 },
});
const row = (title) => [...document.querySelectorAll(".session-row")].find((element) => element.textContent.includes(title));
const marks = (title) => [...row(title).querySelectorAll('[role="img"]')].map((mark) => mark.getAttribute("aria-label"));

test("a chat's sidebar row shows the flask while a run runs and the warning while a flag waits", async (t) => {
  t.mock.getter(HTMLElement.prototype, "scrollWidth", function () {
    return this.textContent.includes("Chat c") ? 900 : 50;
  });
  await renderChat("/projects/p/tasks/a", {
    sessions: [session("a"), session("b"), session("c"), session("d")],
    routes: {
      "GET /api/chat/shown-runs": () => [
        { sessionId: "a", run: run("r1"), experimentName: null, experimentSlug: "one", runNo: 1, runCount: 1 },
        { sessionId: "b", run: run("r2", { id: "f", state: "waiting", mode: "copilot" }), experimentName: null, experimentSlug: "two", runNo: 1, runCount: 1 },
        { sessionId: "c", run: run("r3", { id: "f", state: "agent_looking", mode: "agentic" }), experimentName: null, experimentSlug: "three", runNo: 1, runCount: 1 },
        { sessionId: "d", run: { ...run("r4"), status: "done" }, experimentName: null, experimentSlug: "four", runNo: 1, runCount: 1 },
      ],
      "GET /api/compute/hosts": () => [],
    },
  });
  assert.deepEqual(marks("Chat a"), [m.chat_experiment_running()]);
  assert.deepEqual(marks("Chat b"), [m.chat_run_needs_attention()]);
  assert.deepEqual(marks("Chat c"), [m.chat_run_flagged()]);
  assert.deepEqual(marks("Chat d"), []);
  for (const [title, state] of [["Chat b", m.chat_run_needs_attention()], ["Chat c", m.chat_run_flagged()]]) {
    assert.ok(row(title).getAttribute("aria-label").includes(state));
    assert.ok(row(title).querySelector(".session-title").classList.contains("text-ink-amber"));
    assert.ok(row(title).querySelector('[role="img"]').classList.contains("text-ink-amber"));
  }
  assert.equal(row("Chat a").querySelector(".session-title").classList.contains("text-ink-amber"), false);
  assert.equal(row("Chat b").querySelector(".session-title").hasAttribute("data-overflow"), false);
  assert.equal(row("Chat c").querySelector(".session-title").hasAttribute("data-overflow"), true);
});
