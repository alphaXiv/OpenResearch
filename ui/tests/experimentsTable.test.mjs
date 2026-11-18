import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { ExperimentsTable } from "../src/components/ExperimentsTable.tsx";
import { m } from "../src/paraglide/messages.js";
import { find, press, render, serve, settle } from "./render.mjs";

const experiment = { id: "e1", projectId: "p", title: "Wider MLP", slug: "wider-mlp", branchName: "orx/wider-mlp", createdAt: 1, parentExperimentId: null, archived: false };
const live = { id: "r1", experimentId: "e1", projectId: "p", status: "running", createdAt: 2, updatedAt: 2, cancelling: false, contact: null, lostReason: null, host: null };
const click = async (element) => { await act(async () => element.click()); await settle(); };

test("the table's Stop asks first; Escape keeps the run, a refusal stays in the dialog, and Stop run cancels it", async () => {
  serve(() => ({}));
  const cancels = [];
  let refusal = "The run's machine is offline.";
  const onCancel = async (runId) => {
    cancels.push(runId);
    if (refusal) throw new Error(refusal);
  };
  const props = { runs: [live], experiments: [experiment], showLineage: false, order: "newest", archiveActions: new Map([["e1", {}]]), onOpen() {}, onOpenLogs() {}, onOpenCode() {}, onArchive() {}, onDelete() {}, onCancel };
  const { container } = await render(createElement(ExperimentsTable, props));
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
