import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { DetailDrawer } from "../src/components/DetailDrawer.tsx";
import { m } from "../src/paraglide/messages.js";
import { find, press, render, serve, settle } from "./render.mjs";

const click = async (element) => { await act(async () => element.click()); await settle(); };

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
async function show(overrides, trackioRuns = [], sessions = []) {
  const logs = [];
  serve((path) => {
    const log = path.match(/^\/api\/runs\/([^/]+)\/log/);
    if (log) {
      logs.push(log[1]);
      return { generation: "g", offset: 0, dataBase64: "", nextOffset: 0, eof: true };
    }
    if (path === "/api/compute/hosts") return [];
    if (path.startsWith("/api/chat/sessions")) return { sessions };
    if (path.startsWith("/api/projects/project-a/code-tree")) return { root: "repo", branch: "main", entries: ["notes.md"] };
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

test("stop is offered until a cancel is pending, and the badge carries the pending state", async () => {
  const live = { id: "run-live", experimentId: "experiment-a", createdAt: 4, status: "running", cancelling: false, contact: null, host: "gpu-box" };
  const view = async (run) => {
    const { container } = await show({ runs: [run] });
    const header = container.querySelector(".term-bar");
    return { text: header.textContent, buttons: [...header.querySelectorAll("button")].map((button) => button.textContent) };
  };
  assert.ok((await view(live)).buttons.includes(m.common_stop()));
  for (const [contact, label] of [
    [null, m.status_cancelling()],
    [{ type: "unreachable", since: 1, cause: "Offline" }, m.status_cancellation_pending()],
    [{ type: "needs_auth", since: 1, cause: "Connect" }, m.status_cancellation_pending()],
    [{ type: "unreconciled", since: 1, detail: "2 survivors" }, m.status_cancellation_pending()],
  ]) {
    const cancelling = await view({ ...live, cancelling: true, contact });
    assert.ok(cancelling.text.includes(label), cancelling.text);
    if (contact) assert.ok(!cancelling.text.includes(m.status_cancelling()), cancelling.text);
    assert.ok(!cancelling.buttons.includes(m.common_stop()), cancelling.buttons.join());
    assert.ok((await view({ ...live, contact })).buttons.includes(m.common_stop()));
  }
});

test("an unconfirmed launch offers Stop and Abandon", async () => {
  const launch = { id: "run-launch", experimentId: "experiment-a", createdAt: 4, status: "submitting", cancelling: false, host: "gpu-box", contact: { type: "unreconciled", since: 1, detail: "No orx-host for darwin-aarch64" } };
  const buttons = [...(await show({ runs: [launch] })).container.querySelectorAll("button")].map((button) => button.textContent);
  assert.ok(buttons.includes(m.common_stop()), buttons.join());
  assert.ok(buttons.includes(m.run_abandon()), buttons.join());
});

test("Stop asks first; Escape keeps the run, a refusal stays in the dialog, and Stop run cancels it", async () => {
  const live = { id: "run-live", experimentId: "experiment-a", projectId: "project-a", createdAt: 4, status: "running", cancelling: false, contact: null, host: null };
  const cancels = [];
  let refusal = "The run's machine is offline.";
  serve((path, init) => {
    if (path === "/api/runs/run-live/cancel" && init.method === "POST") {
      cancels.push(path);
      return refusal ? Response.json({ error: refusal }, { status: 409 }) : { ok: true };
    }
    if (path.startsWith("/api/runs/run-live/log")) return { generation: "g", offset: 0, dataBase64: "", nextOffset: 0, eof: true };
    if (path === "/api/compute/hosts") return [];
    if (path === "/api/projects/project-a/experiments") return { experiments: [] };
    if (path === "/api/trackio/projects/project-a/runs") return { dashboardUrl: null, project: "p", runs: [] };
  });
  const { container } = await render(createElement(DetailDrawer, { ...props, selectedRunId: null, runs: [live] }));
  await settle();
  const stop = () => [...container.querySelectorAll(".term-bar button")].find((button) => button.textContent === m.common_stop());
  const dialog = () => document.querySelector('[role="dialog"]');

  await click(stop());
  assert.ok(dialog().textContent.replace(/[⁦-⁩]/g, "").includes(m.run_stop_title({ name: "example" })));
  await press("Escape");
  await settle();
  assert.equal(dialog(), null);
  assert.deepEqual(cancels, []);

  await click(stop());
  await click(find('[role="dialog"] button', m.experiments_stop_run()));
  assert.ok(dialog().textContent.includes(refusal), "the refusal is said where the user confirmed");

  refusal = null;
  await click(find('[role="dialog"] button', m.experiments_stop_run()));
  assert.equal(cancels.length, 2);
  assert.equal(dialog(), null);
});

const described = { ...props.experiment, description: '<file path="notes.md" />', chatSessionId: "chat-a", createdAt: 1 };

async function openDescribedFile(sessions) {
  const opened = [];
  const { container } = await show({ view: "overview", experiment: described, experiments: [], onOpenFile: (...args) => opened.push(args) }, [], sessions);
  const chip = container.querySelector(".file-chip");
  assert.equal(chip.disabled, false);
  await click(chip);
  return opened[0];
}

test("a file reference in the overview description opens the file in the experiment's chat", async () => {
  assert.deepEqual((await openDescribedFile([{ id: "chat-a", harness: "claude" }])).slice(0, 2), ["notes.md", "chat-a"]);
});

test("a file reference opens without the chat once the chat is deleted", async () => {
  assert.deepEqual((await openDescribedFile([{ id: "chat-b" }])).slice(0, 2), ["notes.md", null]);
});
