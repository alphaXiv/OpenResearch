import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { timeAgo } from "../src/api.ts";
import { ExperimentsTable } from "../src/components/ExperimentsTable.tsx";
import { m } from "../src/paraglide/messages.js";
import { find, press, render, serve, settle } from "./render.mjs";

const experiment = { id: "e1", projectId: "p", title: "Wider MLP", slug: "wider-mlp", branchName: "orx/wider-mlp", createdAt: 1, parentExperimentId: null, archived: false };
const live = { id: "r1", experimentId: "e1", projectId: "p", status: "running", createdAt: 2, updatedAt: 2, cancelling: false, contact: null, lostReason: null, host: null };
const click = async (element) => { await act(async () => element.click()); await settle(); };

function renderTable(runs, onCancel = async () => {}) {
  const props = { runs, experiments: [experiment], showLineage: false, order: "newest", archiveActions: new Map([["e1", {}]]), onOpen() {}, onOpenLogs() {}, onOpenCode() {}, onArchive() {}, onDelete() {}, onCancel };
  return render(createElement(ExperimentsTable, props));
}

for (const { status, times, seconds } of [
  { status: "running", times: { startedAt: 990_000 }, seconds: 10 },
  { status: "starting", times: { waitingReason: "Waiting for a worker" }, seconds: 15 },
  { status: "done", times: { startedAt: 990_000, endedAt: 998_000 }, seconds: 8 },
  { status: "lost", times: { startedAt: 990_000, updatedAt: 996_000 }, seconds: 6 },
  { status: "failed", times: { endedAt: 998_000, resultMarkdown: "Couldn't start the worker." }, seconds: null },
]) {
  test(`the table shows elapsed time alongside the latest ${status} run's age`, async (t) => {
    t.mock.timers.enable({ apis: ["Date", "setInterval"], now: 1_000_000 });
    const latest = { ...live, id: "r2", status, createdAt: 985_000, updatedAt: 1_000_000, ...times };
    const { container } = await renderTable([live, latest]);
    const row = container.querySelector('[role="listitem"]');
    const duration = () => row.querySelector(`[title="${m.experiment_overview_duration()}"]`).textContent;
    const label = (value) => `${m.experiment_overview_duration()} ${value === null ? "—" : m.duration_seconds({ value: String(value) })}`;
    assert.ok(row.textContent.includes(timeAgo(latest.createdAt)));
    assert.equal(duration(), label(seconds));
    await act(async () => t.mock.timers.tick(1_000));
    const ticking = status === "running" || status === "starting";
    assert.equal(duration(), label(ticking ? seconds + 1 : seconds));
    if (ticking) assert.ok(row.textContent.includes(timeAgo(latest.createdAt)));
  });
}

test("the table's Stop asks first; Escape keeps the run, a refusal stays in the dialog, and Stop run cancels it", async () => {
  serve(() => ({}));
  const cancels = [];
  let refusal = "The run's machine is offline.";
  const onCancel = async (runId) => {
    cancels.push(runId);
    if (refusal) throw new Error(refusal);
  };
  const { container } = await renderTable([live], onCancel);
  const stop = () => container.querySelector(`button[aria-label="${m.common_stop()}"]`);
  const dialog = () => document.querySelector('[role="dialog"]');

  await click(stop());
  assert.ok(dialog().textContent.replace(/[⁦-⁩]/g, "").includes(m.run_stop_title({ name: "Wider MLP" })));
  await press("Escape");
  await settle();
  assert.equal(dialog(), null);
  assert.deepEqual(cancels, []);

  await click(stop());
  await click(find('[role="dialog"] button', m.experiments_stop_run()));
  assert.ok(dialog().textContent.includes(refusal), "the refusal is said where the user confirmed");

  refusal = null;
  await click(find('[role="dialog"] button', m.experiments_stop_run()));
  assert.deepEqual(cancels, ["r1", "r1"]);
  assert.equal(dialog(), null);
});
