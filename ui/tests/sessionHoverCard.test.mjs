import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { createElement } from "react";
import { SessionHoverCard } from "../src/components/SessionHoverCard.tsx";
import { m } from "../src/paraglide/messages.js";
import { render, serve, settle } from "./render.mjs";

const running = JSON.parse(readFileSync(new URL("./fixtures/runMonitor/running.json", import.meta.url), "utf8"));
const now = Date.now();
const run = (id, extra = {}) => ({
  id, experimentId: `e-${id}`, projectId: "p", chatSessionId: "s", status: "running", createdAt: now - 60_000, startedAt: now - 60_000, updatedAt: now,
  contact: null, cancelling: false, lostReason: null, host: null, backend: { kind: "local_job" },
  monitor: { tone: "running", whole: 0.248, headline: { name: "loss", label: "Loss", value: 2.31 }, sentence: running.sentence, bindings: { loss: 2.31 }, signals: running.view.signals, estimate: null, flag: null, rev: 1, generation: "g1", asOf: now },
  ...extra,
});
const experiment = (id, title) => ({ id, projectId: "p", title, slug: id, createdAt: 1, parentExperimentId: null, archived: false });
const ignore = () => {};
const plain = (text) => text.replace(/[⁦-⁩]/g, "");

/** Chat s's card over its experiments, with `runs` as its composer shows them. */
async function card(experiments, runs) {
  const requests = [];
  serve((path, init) => {
    const request = `${init.method ?? "GET"} ${path}`;
    requests.push(request);
    if (request === "GET /api/compute/hosts") return [];
    if (request === "GET /api/chat/sessions/s/experiments") return { experiments };
    if (request === "GET /api/runs/r1/monitor") return running;
  });
  const container = document.body.appendChild(document.createElement("div"));
  await render(createElement(SessionHoverCard, {
    session: { id: "s", projectId: "p", title: "Sweep", harness: "codex" },
    runs: runs.map((value) => ({ sessionId: "s", run: value, experimentName: null, experimentSlug: value.experimentId, runNo: 1, runCount: 1 })),
    motion: "enter", modelName: null, anchor: new DOMRect(), cardRef: { current: null }, cardId: "card", container,
    onOpenExperiment: ignore, onReturnToRow: ignore, onContinue: ignore,
  }));
  await settle();
  return { container, requests };
}

test("a chat's card draws the run its composer shows under that run's experiment, and only that one", async (t) => {
  const live = run("r1");
  const ended = run("r2", { experimentId: "e-r2", status: "done", endedAt: now - 1_000, monitor: null });
  const { container, requests } = await card([
    { experiment: experiment("e-r1", "Learning-rate sweep"), projectName: "p", currentRun: live },
    { experiment: experiment("e-r2", "Baseline"), projectName: "p", currentRun: ended },
  ], [live]);
  t.after(() => container.remove());

  const [sweep, baseline] = container.querySelectorAll("li");
  assert.ok(sweep.querySelector('[role="progressbar"]'), "the live run's bar");
  assert.ok(sweep.querySelector('[role="img"][aria-label*="Loss"]'), "the live run's chart");
  assert.match(plain(sweep.textContent), /Loss 2\.31/);
  assert.equal(baseline.querySelector('[role="progressbar"]'), null);
  assert.match(plain(baseline.textContent), new RegExp(m.status_done()));
  assert.deepEqual(requests.filter((request) => request.endsWith("/monitor")), ["GET /api/runs/r1/monitor"]);
});

test("a chat's card over several runs says what they do together and keeps each to its count and bar", async (t) => {
  const runs = ["r1", "r2", "r3"].map((id) => run(id));
  const { container } = await card(runs.map((value) => ({ experiment: experiment(value.experimentId, `LR ${value.id}`), projectName: "p", currentRun: value })), runs);
  t.after(() => container.remove());

  assert.equal(container.querySelectorAll('[role="progressbar"]').length, 3);
  assert.equal(container.querySelector('[role="img"][aria-label*="Loss"]'), null, "no chart for each run");
  assert.equal(container.querySelectorAll("li").length, 3);
  const marks = [...container.querySelectorAll('[role="img"]')].filter((mark) => !mark.closest("ul"));
  assert.equal(marks.length, 3, "a summary over all three, a mark for each");
});

test("an experiment run again shows its newest run alone, without a summary over the two", async (t) => {
  const older = run("r0", { experimentId: "e-r1", createdAt: now - 120_000 });
  const newer = run("r1");
  const { container, requests } = await card([{ experiment: experiment("e-r1", "Sweep"), projectName: "p", currentRun: newer }], [older, newer]);
  t.after(() => container.remove());

  assert.equal(container.querySelectorAll('[role="progressbar"]').length, 1);
  assert.ok(container.querySelector('[role="img"][aria-label*="Loss"]'), "a lone run keeps its chart");
  assert.deepEqual(requests.filter((request) => request.endsWith("/monitor")), ["GET /api/runs/r1/monitor"]);
});
