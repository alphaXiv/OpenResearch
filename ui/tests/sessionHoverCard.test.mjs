import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { SessionHoverCard } from "../src/components/SessionHoverCard.tsx";
import { m } from "../src/paraglide/messages.js";
import { press, render, serve, settle } from "./render.mjs";

const plain = (text) => text.replace(/[⁦-⁩]/g, "").replace(/ /g, " ");
const ignore = () => {};
const signals = { loss: { label: "Loss", digits: 2, better: "lower" } };

const summary = (at, extra = {}) => ({
  tone: "running", whole: 0.2, stage: { name: "train", done: 120, of: 5000, unit: "steps" }, stageOrder: { index: 1, count: 2, first: 1 },
  headline: { name: "loss", label: "Loss", value: 2.31 }, sentence: null, bindings: { loss: 2.31 }, signals,
  estimate: { loMs: 600_000, midMs: 750_000, hiMs: 900_000, at, pending: false, floor: false },
  limitEndsAt: null, flag: null, setup: null, logError: null, rev: 1, generation: "g", asOf: at, ...extra,
});
const run = (at, id, extra = {}) => ({
  id, experimentId: id, projectId: "p", status: "running", createdAt: at - 60_000, startedAt: at - 60_000, updatedAt: at,
  contact: null, cancelling: false, lostReason: null, host: null, monitor: summary(at), ...extra,
});
const ended = (at, id, status, extra = {}) => run(at, id, { status, startedAt: at - 360_000, endedAt: at - 300_000, updatedAt: at - 300_000, monitor: summary(at, { tone: status, estimate: null }), ...extra });
const row = (id, title, currentRun) => ({ experiment: { id, projectId: "p", slug: id, title, archived: false, createdAt: 1 }, projectName: "Research", currentRun });

/** Chat "chat"'s hover card over `experiments` as the server lists them: each row's text, and the experiments opened. */
async function card(experiments) {
  serve((path) => {
    if (path === "/api/chat/sessions/chat/experiments") return { experiments };
    if (path === "/api/compute/hosts") return [];
  });
  const container = document.body.appendChild(document.createElement("div"));
  const opened = [];
  const view = await render(createElement(SessionHoverCard, {
    session: { id: "chat", projectId: "p", title: "Sweep", harness: "codex" },
    modelName: null, anchor: new DOMRect(), cardRef: { current: null }, cardId: "card", container,
    onEnter: ignore, onLeave: ignore, onOpenExperiment: (_project, id) => opened.push(id), onReturnToRow: ignore, onContinue: ignore,
  }));
  await settle();
  const rows = () => [...container.querySelectorAll("[data-experiment-row]")];
  return {
    container, opened, rows,
    text: (index) => plain(rows()[index].textContent),
    unmount: () => { view.unmount(); container.remove(); },
  };
}

test("a live run says its stage and count, its time left and its headline", async () => {
  const now = Date.now();
  const shown = await card([row("wide", "Wider layers", run(now, "wide"))]);
  const text = shown.text(0);
  for (const fact of ["Wider layers", "train (2/2)", "Step 120 of 5,000", "12m left", "Loss 2.31"]) assert.ok(text.includes(fact), `${fact}: ${text}`);
  assert.ok(shown.rows()[0].querySelector(`[role="img"][aria-label="${m.status_running()}"]`));
  shown.unmount();
});

test("a flag waiting on the user leads the card with its warning, in amber", async () => {
  const now = Date.now();
  const flag = { id: "f1", state: "waiting", template: "Loss is **{loss}**.", bindings: { loss: "NaN" } };
  const shown = await card([
    row("wide", "Wider layers", run(now, "wide")),
    row("high", "Higher learning rate", run(now, "high", { monitor: summary(now, { tone: "flag", flag }) })),
  ]);
  const text = shown.text(0);
  assert.ok(text.startsWith("Higher learning rate"), text);
  for (const fact of ["Loss is NaN.", m.chat_hover_waiting_for_choice()]) assert.ok(text.includes(fact), `${fact}: ${text}`);
  assert.ok(shown.rows()[0].querySelector(`[role="img"][aria-label="${m.status_flagged()}"]`));
  assert.ok(shown.rows()[0].className.includes("bg-accent-amber-subtle"));
  assert.ok(!shown.text(1).includes(m.chat_hover_waiting_for_choice()));
  shown.unmount();
});

test("a finished run says how long it took, when it ended and its result; a failed one says why", async () => {
  const now = Date.now();
  const shown = await card([
    row("base", "Baseline", ended(now, "base", "done")),
    row("small", "Smaller batch", ended(now, "small", "failed", { resultMarkdown: "CUDA out of memory." })),
  ]);
  const done = shown.text(0);
  for (const fact of ["Baseline", "Done in 1m", "5m ago", "Loss 2.31"]) assert.ok(done.includes(fact), `${fact}: ${done}`);
  const failed = shown.text(1);
  for (const fact of ["Smaller batch", m.status_failed(), "CUDA out of memory.", "5m ago"]) assert.ok(failed.includes(fact), `${fact}: ${failed}`);
  assert.ok(!failed.includes("Loss"), failed);
  shown.unmount();
});

test("live experiments come first, then the ended ones, then those never run; each opens by click", async () => {
  const now = Date.now();
  const shown = await card([
    row("idea", "Dropout ablation", null),
    row("base", "Baseline", ended(now, "base", "done")),
    row("wide", "Wider layers", run(now, "wide")),
  ]);
  const titles = ["Wider layers", "Baseline", "Dropout ablation"];
  assert.deepEqual(titles.map((title, index) => shown.text(index).startsWith(title)), titles.map(() => true));
  assert.ok(shown.text(2).includes(m.experiments_not_run_yet()));
  assert.equal(shown.rows()[0].tabIndex, 0, "the first row is the one Tab reaches");
  await act(async () => shown.rows()[0].focus());
  await press("ArrowDown");
  assert.equal(document.activeElement, shown.rows()[1]);
  await act(async () => shown.rows()[2].click());
  assert.deepEqual(shown.opened, ["idea"]);
  shown.unmount();
});

test("many experiments all stay listed, under their count", async () => {
  const now = Date.now();
  const shown = await card(Array.from({ length: 14 }, (_, index) => row(`seed-${index}`, `Seed ${index + 1}`, ended(now, `seed-${index}`, "done"))));
  assert.equal(shown.rows().length, 14);
  assert.ok(plain(shown.container.textContent).includes(`${m.app_experiments()}14`));
  shown.unmount();
});

test("a chat with no experiments shows no list", async () => {
  const shown = await card([]);
  assert.equal(shown.rows().length, 0);
  assert.ok(!shown.container.textContent.includes(m.app_experiments()));
  shown.unmount();
});

test("time left counts down while the card is open and stops when it closes", async (t) => {
  const now = Date.now();
  const ticks = [];
  t.mock.method(window, "setInterval", (tick, ms) => { if (ms === 1000) ticks.push(tick); return ticks.length; });
  const cleared = t.mock.method(window, "clearInterval", () => {});
  t.mock.timers.enable({ apis: ["Date"], now });
  const shown = await card([row("wide", "Wider layers", run(now, "wide"))]);
  assert.ok(shown.text(0).includes("12m left"), shown.text(0));
  assert.equal(ticks.length, 1);
  t.mock.timers.setTime(now + 60_000);
  await act(async () => ticks[0]());
  assert.ok(shown.text(0).includes("11m left"), shown.text(0));
  shown.unmount();
  assert.deepEqual(cleared.mock.calls.map((call) => call.arguments[0]), [1]);
});
