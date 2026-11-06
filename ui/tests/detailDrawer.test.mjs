import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { DetailDrawer } from "../src/components/DetailDrawer.tsx";
import { m } from "../src/paraglide/messages.js";
import { render, serve, settle } from "./render.mjs";

const runs = [
  { id: "run-old", experimentId: "experiment-a", createdAt: 1, status: "completed" },
  { id: "run-new", experimentId: "experiment-a", createdAt: 2, status: "completed" },
  { id: "foreign-run", experimentId: "experiment-b", createdAt: 3, status: "completed" },
];
const props = {
  experiment: { id: "experiment-a", projectId: "project-a", slug: "example" },
  project: {},
  view: "terminal",
  runs,
  onSelectRun: () => assert.fail("render must not navigate"),
};

/** Renders the drawer against a backend whose Trackio store holds `trackioRuns`; returns the runs whose logs it read. */
async function show(overrides, trackioRuns = []) {
  const logs = [];
  serve((path) => {
    const log = path.match(/^\/api\/runs\/([^/]+)\/log/);
    if (log) {
      logs.push(log[1]);
      return { generation: "g", offset: 0, dataBase64: "", nextOffset: 0, eof: true };
    }
    if (path === "/api/compute/hosts") return [];
    if (path === "/api/trackio/projects/project-a/runs") {
      return { dashboardUrl: trackioRuns.length ? "http://127.0.0.1:7860" : null, project: "p", runs: trackioRuns };
    }
  });
  const { container } = await render(createElement(DetailDrawer, { selectedRunId: null, ...props, ...overrides }));
  await settle();
  return { container, logs };
}

test("terminal uses newest only when the URL omits a run", async () => {
  assert.deepEqual((await show({})).logs, ["run-new"]);
  assert.deepEqual((await show({ selectedRunId: "run-old" })).logs, ["run-old"]);
});

test("missing and foreign explicit runs render unavailable without substituting another run", async () => {
  for (const id of ["missing-run", "foreign-run"]) {
    const { container, logs } = await show({ selectedRunId: id });
    assert.deepEqual(logs, []);
    assert.match(container.textContent, new RegExp(m.model_picker_unavailable()));
  }
});

const buttonText = (container) => [...container.querySelectorAll("button")].map((button) => button.textContent);

test("the header offers no Trackio before the run has logged there", async () => {
  assert.ok(!buttonText((await show({})).container).includes(m.experiment_overview_visualize()));
});

test("the header offers Trackio once the run has logged there", async () => {
  const { container, logs } = await show({}, ["run-new"]);
  assert.ok(buttonText(container).includes(m.experiment_overview_visualize()));
  assert.deepEqual(logs, ["run-new"]);
});

test("the trackio view says so when the run has not logged there", async () => {
  const { container, logs } = await show({ view: "trackio" });
  assert.deepEqual(logs, []);
  assert.match(container.textContent, new RegExp(m.detail_drawer_trackio_no_data()));
});

test("the trackio view embeds the run's charts in place of its logs", async () => {
  const { container, logs } = await show({ view: "trackio" }, ["run-new"]);
  const chart = new URL(container.querySelector("iframe").src);
  assert.equal(chart.origin, "http://127.0.0.1:7860");
  assert.equal(chart.searchParams.get("runs"), "run-new");
  assert.equal(chart.searchParams.get("sidebar"), "hidden");
  assert.match(container.textContent, new RegExp(m.experiment_overview_logs()));
  assert.deepEqual(logs, []);
});

test("monitoring diagnostic follows the selected run and clears after recovery", async () => {
  const diagnostic = "Monitoring unavailable: no exit status or scheduler record.";
  const status = async (overrides, newest) => {
    const { container } = await show({ runs: [runs[0], { ...runs[1], ...newest }], ...overrides });
    return container.querySelector('[role="status"]')?.textContent ?? null;
  };
  assert.equal(await status({}, { status: "running", backend: { monitoringError: diagnostic } }), diagnostic);
  assert.equal(await status({ selectedRunId: "run-old" }, { status: "running", backend: { monitoringError: diagnostic } }), null);
  assert.equal(await status({}, { status: "running", backend: {} }), null);
  assert.equal(await status({}, { status: "completed", backend: { monitoringError: diagnostic } }), null);
});

test("the contact notice follows the selected run", async () => {
  const lost = (run, cause) => ({ ...run, status: "running", host: "gpu-box", contact: { type: "unreachable", since: 1, cause } });
  const drawer = { runs: [lost(runs[0], "old cause"), lost(runs[1], "new cause"), runs[2]] };
  const notice = async (selectedRunId) => (await show({ ...drawer, selectedRunId })).container.querySelector('[role="status"]')?.textContent ?? "";
  assert.ok((await notice(null)).includes("new cause"));
  assert.ok((await notice("run-old")).includes("old cause"));
  assert.equal(await notice("missing-run"), "");
});

test("stop distinguishes a pending cancel from a delivered one", async () => {
  const live = { id: "run-live", experimentId: "experiment-a", createdAt: 4, status: "running", cancelling: false, contact: null, host: "gpu-box" };
  const text = async (run) => (await show({ runs: [run] })).container.textContent;
  assert.ok((await text(live)).includes(m.common_stop()));
  for (const [contact, label] of [
    [null, m.status_cancelling()],
    [{ type: "unreachable", since: 1, cause: "Offline" }, m.status_cancellation_pending()],
    [{ type: "needs_auth", since: 1, cause: "Connect" }, m.status_cancellation_pending()],
  ]) {
    const shown = await text({ ...live, cancelling: true, contact });
    assert.ok(shown.includes(label), shown);
    if (contact) assert.ok(!shown.includes(m.status_cancelling()), shown);
    assert.ok((await text({ ...live, contact })).includes(m.common_stop()));
  }
});
