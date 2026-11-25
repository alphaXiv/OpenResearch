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
  localStorage.setItem("sidebar-grouping", "projects");
  t.after(() => localStorage.removeItem("sidebar-grouping"));
  await renderChat("/projects/p/tasks/a", {
    sessions: [session("a"), session("b"), session("c"), session("d")],
    routes: {
      "GET /api/chat/shown-runs": () => [
        { sessionId: "a", run: run("r1"), experimentName: null, experimentSlug: "one", runNo: 1, runCount: 1 },
        { sessionId: "b", run: run("r2", { id: "f", state: "waiting" }), experimentName: null, experimentSlug: "two", runNo: 1, runCount: 1 },
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

test("in the inbox, a chat whose run flag waits keeps its warning in Working, a monitored run adds its summary line, and an ended run leaves its chat in the inbox", async () => {
  const monitored = (id, monitor, extra = {}) => ({ ...run(id), ...extra, monitor: { ...run(id, {}).monitor, flag: null, ...monitor } });
  // Chat h's run was answered, so only the chat's own record of its newest run still holds it.
  const answered = { ...session("h"), lastRun: monitored("r8", { tone: "done", headline: { name: "acc", label: "Accuracy", value: 0.91 } }, { status: "done", endedAt: 2 }) };
  // Chat i's record predates its run ending: a live run would be among the shown ones.
  const stale = { ...session("i"), lastRun: monitored("r9", { tone: "running", sentence: { template: "Step **{step}**.", t: Date.now() }, bindings: { step: 3 } }) };
  const sessions = [session("a"), session("b"), session("d"), session("e"), session("f"), session("g"), answered, stale];
  localStorage.setItem("sidebar-grouping", "inbox");
  try {
    await renderChat("/projects/p/tasks/a", {
      sessions,
      routes: {
        "GET /api/chat/sessions": (_, url) => ({
          sessions: url.includes("scope=sidebar") ? url.includes("archived=true") ? [] : sessions : sessions.filter((row) => url.includes(`projectId=${row.projectId}`)),
        }),
        "GET /api/chat/shown-runs": () => [
          { sessionId: "a", run: run("r1"), experimentName: null, experimentSlug: "one", runNo: 1, runCount: 1 },
          { sessionId: "b", run: run("r2", { id: "f", state: "waiting" }), experimentName: null, experimentSlug: "two", runNo: 1, runCount: 1 },
          { sessionId: "d", run: { ...run("r4"), status: "done" }, experimentName: null, experimentSlug: "four", runNo: 1, runCount: 1 },
          { sessionId: "e", run: monitored("r5", { tone: "running", bindings: { loss: 2.31 }, sentence: { template: "Loss **{loss}**, falling steadily.", t: Date.now() } }), experimentName: null, experimentSlug: "five", runNo: 1, runCount: 1 },
          { sessionId: "f", run: monitored("r6", { tone: "done", headline: { name: "loss", label: "Loss", value: 1.87 } }, { status: "done", endedAt: 2 }), experimentName: null, experimentSlug: "six", runNo: 1, runCount: 1 },
          { sessionId: "g", run: monitored("r7", { tone: "running", stage: { name: "Train", done: 13, of: 24, unit: "epochs" }, stageOrder: { index: 1, count: 3, first: 0 } }), experimentName: null, experimentSlug: "seven", runNo: 1, runCount: 1 },
        ],
        "GET /api/compute/hosts": () => [],
      },
    });
    const working = [...document.querySelectorAll("button[aria-expanded]")].find((button) => button.textContent.startsWith(m.chat_inbox_working()));
    assert.ok(working, "a Working section");
    const inbox = [...document.querySelectorAll("button[aria-expanded]")].find((button) => button.textContent.startsWith(m.sidebar_inbox()));
    const after = (header, title) => Boolean(header.compareDocumentPosition(row(title)) & Node.DOCUMENT_POSITION_FOLLOWING);
    const inWorking = (title) => after(working, title) && !after(inbox, title);

    assert.ok(inWorking("Chat a") && inWorking("Chat b"));
    assert.equal(inWorking("Chat d"), false);
    assert.deepEqual(marks("Chat b"), [m.chat_run_needs_attention(), m.chat_experiment_running()]);
    assert.ok(row("Chat b").getAttribute("aria-label").includes(m.chat_run_needs_attention()));
    assert.ok(row("Chat b").querySelector(".session-title").classList.contains("text-ink-amber"));
    assert.equal(row("Chat a").querySelector(".session-title").classList.contains("text-ink-amber"), false);
    assert.equal(row("Chat d").querySelector(".session-title").classList.contains("text-ink-amber"), false);

    // A monitored run gets the card a middle line, figures bold: the watcher's words, the stage's count without its
    // name, or a finished run's result. A chat without one keeps two lines.
    const line = (title) => row(title).textContent.replace(/[\u2066-\u2069]/g, "");
    const bold = (title) => [...row(title).querySelectorAll("strong")].map((figure) => figure.textContent.replace(/[\u2066-\u2069]/g, ""));
    const height = (title) => row(title).closest("[data-index]").style.height;
    assert.match(line("Chat e"), /Loss 2\.31, falling steadily\./);
    assert.deepEqual(bold("Chat e"), ["2.31"]);
    assert.match(line("Chat f"), /Loss 1\.87/);
    assert.deepEqual(bold("Chat f"), ["1.87"]);
    assert.match(line("Chat g"), /13 of 24 epochs/);
    assert.doesNotMatch(line("Chat g"), /Train/);
    assert.match(line("Chat h"), /Accuracy 0\.91/);
    assert.equal(height("Chat a"), "50px");
    assert.equal(height("Chat i"), "50px");
    assert.equal(height("Chat e"), "70px");
  } finally {
    localStorage.removeItem("sidebar-grouping");
  }
});
