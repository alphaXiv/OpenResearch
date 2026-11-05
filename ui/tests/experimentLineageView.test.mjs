import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { ExperimentLineage } from "../src/components/ExperimentLineage.tsx";
import { buildHistoryNodes } from "../src/components/experimentLineageModel.ts";
import { statusLabel } from "../src/components/StatusBadge.tsx";
import { m } from "../src/paraglide/messages.js";
import { render } from "./render.mjs";

const experiments = Array.from({ length: 8 }, (_, index) => ({
  id: String(index),
  title: `Experiment ${index}`,
  slug: `experiment-${index}`,
  createdAt: index,
  parentExperimentId: index ? String(index - 1) : null,
}));
const nodes = buildHistoryNodes(experiments, []);

async function lineage(node, onSelect = () => {}) {
  const { container } = await render(createElement(ExperimentLineage, { node, onSelect }));
  const steps = () => [...container.querySelectorAll("button:not([aria-expanded])")];
  return {
    container,
    expander: () => container.querySelector("button[aria-expanded]"),
    steps,
    labels: () => steps().map((step) => step.textContent.match(/Experiment \d/)[0]),
  };
}

test("lineage shows four ancestors and the current experiment before folding earlier ancestors", async () => {
  const { expander, steps, labels } = await lineage(nodes[6]);
  assert.equal(expander().getAttribute("aria-expanded"), "false");
  assert.deepEqual(labels(), ["Experiment 2", "Experiment 3", "Experiment 4", "Experiment 5", "Experiment 6", "Experiment 7"]);
  assert.equal(steps().find((step) => step.getAttribute("aria-current") === "true").textContent.includes("Experiment 6"), true);
});

test("expanding the lineage keeps a collapse control and shows every ancestor", async () => {
  const { expander, labels } = await lineage(nodes[6]);
  await act(() => expander().click());
  assert.equal(expander().getAttribute("aria-expanded"), "true");
  assert.deepEqual(labels(), ["Experiment 0", "Experiment 1", "Experiment 2", "Experiment 3", "Experiment 4", "Experiment 5", "Experiment 6", "Experiment 7"]);

  await act(() => expander().click());
  assert.equal(labels().length, 6);
});

test("short lineage has no unnecessary expand control", async () => {
  const { expander, labels } = await lineage(nodes[4]);
  assert.equal(expander(), null);
  assert.deepEqual(labels(), ["Experiment 0", "Experiment 1", "Experiment 2", "Experiment 3", "Experiment 4", "Experiment 5"]);
});

test("ancestor and child rows preserve preview and keep-open gestures", async () => {
  const opened = [];
  const { steps } = await lineage(nodes[6], (...args) => opened.push(args));
  for (const id of ["5", "7"]) {
    const row = steps().find((step) => step.textContent.includes(`Experiment ${id}`));
    await act(() => {
      row.dispatchEvent(new MouseEvent("click", { bubbles: true, detail: 1 }));
      row.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, detail: 2 }));
      row.dispatchEvent(new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 }));
      row.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Enter" }));
      row.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: " " }));
    });
    assert.deepEqual(opened.splice(0), [
      [id, "preview"], [id, "keepOpen"], [id, "keepOpen"], [id, "keepOpen"], [id, "preview"],
    ]);
  }
});

test("lineage never announces Cancelled while its latest run still has survivors", async () => {
  const [node] = buildHistoryNodes([experiments[0]], [{
    id: "run", experimentId: experiments[0].id, status: "cancelled", createdAt: 1, host: "gpu-box", contact: { type: "stopping", since: 1, count: 2 },
  }]);
  const { container } = await lineage(node);
  const text = container.textContent.replace(/[⁦-⁩]/g, "");
  assert.ok(text.includes(statusLabel("not_fully_stopped", { count: 2, host: "gpu-box" }).replace(/[⁦-⁩]/g, "")), text);
  assert.ok(![...container.querySelectorAll("*")].some((element) => element.textContent === m.status_cancelled()));
});
