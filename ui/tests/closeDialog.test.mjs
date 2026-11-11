import assert from "node:assert/strict";
import test, { afterEach } from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { taskLocation } from "../src/workspaceState.ts";
import { find, press, renderApp, serveApp, settle } from "./render.mjs";

const dialog = () => document.querySelector('[role="dialog"], [role="alertdialog"]');
const agent = (title, extra = {}) => ({
  projectId: "project", project: "Scaling laws", sessionId: title, sideParentId: null, harness: "codex", title, ...extra,
});
const run = (name, afterQuit, backend = "modal_job") => ({
  projectId: "project", project: "Scaling laws", experimentId: `${name}-experiment`, runId: name, sessionId: "chat", name, backend, afterQuit,
});
/** The list under `heading`, by the names in its rows. */
const listed = (heading) => {
  const section = [...dialog().querySelectorAll("section")].find((element) => element.querySelector("h3").textContent === heading);
  return section ? [...section.querySelectorAll("li button")].map((row) => row.textContent) : null;
};
/** The dialog's button labelled exactly `label`; `find` would also match "Force Quit" for "Quit". */
const button = (label) => [...dialog().querySelectorAll("button")].find((element) => element.textContent === label);
const quit = () => button(m.close_pending_quit());
/** Compares DOM nodes by identity: a failed `assert.equal` on one prints the whole document graph. */
const same = (actual, expected) => assert.ok(actual === expected);

/** What the server reports pending while the dialog is open; a test changes it to make work finish. */
let pending;

/** The app as the desktop window shows it, asked about a close that found `work` pending; returns what it told the window. */
async function requestClose(work, minimizes = false) {
  const sent = [];
  window.ipc = { postMessage: (message) => sent.push(message) };
  pending = work;
  serveApp({ "GET /api/pending-work": () => pending ?? undefined });
  await renderApp("/");
  await ask(work, minimizes);
  return sent;
}

async function ask(work, minimizes = false) {
  await act(async () => window.dispatchEvent(new CustomEvent("orx:close-request", { detail: { work, minimizes } })));
  await settle();
}

async function click(label) {
  await act(async () => find("[role] button", label).click());
  await settle();
}

afterEach(() => {
  delete window.ipc;
});

test("each pending chat and experiment is listed by name under what quitting does to it", async () => {
  const sent = await requestClose({
    agents: [agent("Tune the schedule"), agent(null, { sessionId: "side", sideParentId: "chat" })],
    runs: [run("lr-sweep", "continues"), run("baseline", "reports", "hf_job")],
  });

  assert.deepEqual(listed(m.close_pending_agents()), [
    `Tune the scheduleScaling laws`,
    `${m.side_chat_untitled()}Scaling laws`,
  ]);
  assert.deepEqual(listed(m.close_pending_continues()), ["lr-sweepScaling laws"]);
  assert.deepEqual(listed(m.close_pending_unsent()), ["baselineScaling laws"]);
  assert.equal(listed(m.close_pending_needs_open()), null);
  assert.equal(dialog().textContent.includes(m.close_pending_blocked()), false);
  assert.equal(quit().disabled, false);
  same(document.activeElement, quit());
  assert.deepEqual(sent, ["close:asking"]);
});

test("an experiment that needs OpenResearch open says why, and only a force quit stops it", async () => {
  const backends = { local_job: m.close_pending_reason_local(), tinker_job: m.close_pending_reason_tinker(), vercel_job: m.close_pending_reason_vercel(), openresearch_job: m.close_pending_reason_box() };
  const sent = await requestClose({ agents: [], runs: Object.keys(backends).map((backend) => run(backend, "needsApp", backend)) });

  assert.deepEqual(listed(m.close_pending_needs_open()), Object.entries(backends).map(([name, reason]) => `${name}${reason}Scaling laws`));
  assert.ok(dialog().textContent.includes(m.close_pending_blocked()));
  assert.ok(!quit());
  same(document.activeElement, button(m.close_pending_background()));

  await click(m.close_pending_force_quit());
  assert.deepEqual(sent, ["close:asking", "close:quit"]);
});

test("the list follows the work as it finishes, and the close is dropped once nothing is left", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const sent = await requestClose({ agents: [agent("Tune the schedule")], runs: [run("lr-sweep", "needsApp", "local_job")] });
  const poll = async () => {
    await act(async () => t.mock.timers.tick(1_000));
    await settle();
  };

  pending = { agents: [agent("Tune the schedule")], runs: [] };
  await poll();
  assert.equal(listed(m.close_pending_needs_open()), null);
  assert.equal(dialog().textContent.includes(m.close_pending_blocked()), false);
  assert.equal(quit().disabled, false);

  pending = { agents: [], runs: [] };
  await poll();
  assert.ok(!dialog());
  assert.deepEqual(sent.filter((message) => message.startsWith("close:")), ["close:asking", "close:cancel"]);
});

test("opening a listed item cancels the close and shows it", async () => {
  const sent = await requestClose({ agents: [agent("Tune the schedule")], runs: [run("lr-sweep", "needsApp", "local_job")] });

  await act(async () => find("li button", "lr-sweep").click());
  await settle();
  assert.ok(!dialog());
  const pane = { kind: "experiment", experimentId: "lr-sweep-experiment", view: "overview", runId: "lr-sweep" };
  assert.equal(location.pathname + location.search, taskLocation("project", "chat", pane));

  await ask({ agents: [agent(null, { sessionId: "side", sideParentId: "chat" })], runs: [] });
  await act(async () => find("li button", m.side_chat_untitled()).click());
  await settle();
  assert.equal(location.pathname + location.search, taskLocation("project", "chat", { kind: "side", sessionId: "side" }));
  assert.deepEqual(sent.filter((message) => message.startsWith("close:")), ["close:asking", "close:cancel", "close:asking", "close:cancel"]);
});

test("work that could not be counted says so and still lets the user quit", async () => {
  const sent = await requestClose(null);
  assert.ok(dialog().textContent.includes(m.close_pending_uncounted()));
  await click(m.close_pending_quit());
  assert.deepEqual(sent, ["close:asking", "close:quit"]);
});

test("keeping the work running is named for what the window does", async () => {
  const sent = await requestClose({ agents: [agent("Tune the schedule")], runs: [] }, true);
  assert.ok(!button(m.close_pending_background()));
  await click(m.close_pending_minimize());
  assert.deepEqual(sent, ["close:asking", "close:background"]);
});

test("each choice reaches the window and closes the dialog", async () => {
  const work = { agents: [agent("Tune the schedule")], runs: [run("lr-sweep", "continues")] };
  const sent = await requestClose(work);
  await click(m.close_pending_quit());
  assert.ok(!dialog());

  await ask(work);
  await click(m.common_cancel());

  await ask(work);
  await press("Escape");
  await settle();

  assert.ok(!dialog());
  assert.deepEqual(sent, ["close:asking", "close:quit", "close:asking", "close:cancel", "close:asking", "close:cancel"]);
});

test("a repeated close while the dialog is open acknowledges without opening a second one", async () => {
  const work = { agents: [agent("Tune the schedule")], runs: [] };
  const sent = await requestClose(work);
  await ask(work);

  assert.equal(document.querySelectorAll('[role="dialog"], [role="alertdialog"]').length, 1);
  assert.deepEqual(sent, ["close:asking", "close:asking"]);
});

/** The app with an update waiting for a restart; returns the bodies of the restarts it asked for. */
async function restartable(work) {
  const restarts = [];
  pending = work;
  serveApp({
    "GET /api/pending-work": () => pending ?? undefined,
    "GET /api/update": () => ({
      current: "0.2.21", latest: "0.3.0", latestTag: "v0.3.0", channel: "app-bundle", releaseChannel: "stable", releaseChannelsAvailable: true,
      waitingForStable: false, waitingForCanary: false, selfUpdates: true, autoUpdate: true, envDisabled: false, updateAvailable: false,
      installedVersion: "0.3.0", installedTag: "v0.3.0", restartRequired: true, canRestart: true, instance: "old", changes: null,
    }),
    "POST /api/update/restart": (init) => {
      restarts.push(JSON.parse(init.body));
      return Response.json({ error: "held for the test" }, { status: 409 });
    },
  });
  await renderApp("/");
  return restarts;
}

const restart = async () => {
  await act(async () => find("button", m.update_banner_restart()).click());
  await settle();
};

test("restarting for an update first asks to stop the agents at work, and only then stops them", async () => {
  const restarts = await restartable({ agents: [agent("Tune the schedule")], runs: [run("lr-sweep", "needsApp", "local_job")] });

  await restart();
  assert.ok(dialog().textContent.includes(m.restart_pending_title()));
  assert.deepEqual([...dialog().querySelectorAll("li button")].map((row) => row.textContent), ["Tune the scheduleScaling laws"]);
  await click(m.common_cancel());
  assert.ok(!dialog());
  assert.deepEqual(restarts, []);

  await restart();
  await act(async () => button(m.update_banner_restart()).click());
  await settle();
  assert.ok(!dialog());
  assert.deepEqual(restarts, [{ stopAgents: true }]);
});

test("experiments carry on through a restart, so they alone never ask", async () => {
  const restarts = await restartable({ agents: [], runs: [run("lr-sweep", "needsApp", "local_job"), run("baseline", "continues")] });
  await restart();
  assert.ok(!dialog());
  assert.deepEqual(restarts, [{ stopAgents: false }]);
});

test("a restart asked about while its agents finish is dropped", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const restarts = await restartable({ agents: [agent("Tune the schedule")], runs: [] });
  await restart();
  assert.ok(dialog());

  pending = { agents: [], runs: [] };
  await act(async () => t.mock.timers.tick(1_000));
  await settle();
  assert.ok(!dialog());
  assert.deepEqual(restarts, []);
});

test("a close asks about what the window just counted, not an older count", async () => {
  await restartable({ agents: [], runs: [] });
  await restart();
  const sent = [];
  window.ipc = { postMessage: (message) => sent.push(message) };

  pending = { agents: [agent("Tune the schedule")], runs: [] };
  await ask(pending);
  assert.deepEqual(listed(m.close_pending_agents()), ["Tune the scheduleScaling laws"]);
  assert.deepEqual(sent, ["close:asking"]);
});
