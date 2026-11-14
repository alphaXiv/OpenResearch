import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { act, createElement } from "react";
import { ExperimentOverview } from "../src/components/ExperimentOverview.tsx";
import { m } from "../src/paraglide/messages.js";
import { createWorkspace } from "../src/workspace.tsx";
import { render, serveApp, settle } from "./render.mjs";

const fixture = (name) => JSON.parse(readFileSync(new URL(`./fixtures/runMonitor/${name}.json`, import.meta.url), "utf8"));
const now = Date.now();
// A page's table virtualizes against its box, which happy-dom never lays out.
for (const size of ["clientHeight", "clientWidth", "offsetHeight", "offsetWidth"]) {
  Object.defineProperty(HTMLElement.prototype, size, { configurable: true, get: () => 800 });
}

/** What a reader sees, without the marks that keep numbers and names whole in either direction. */
const text = (element) => element.textContent.replace(/[⁦-⁩]/g, "");

const pipeline = () => ({ ...fixture("pipeline"), runId: "r2", asOf: now, estimate: { ...fixture("pipeline").estimate, at: now }, statusText: "distil-glue-6l  running  50%\nsignals  loss 0.47" });
const summary = (monitor, extra = {}) => ({
  tone: "running", whole: monitor.progress?.whole ?? null, stage: null, headline: null, sentence: null, bindings: {}, signals: monitor.view.signals,
  etaMs: null, limitEndsAt: null, flag: null, rev: monitor.rev, generation: monitor.generation, asOf: monitor.asOf, ...extra,
});
const run = (id, extra = {}) => ({
  id, experimentId: "e1", projectId: "p", status: "running", createdAt: now - 600_000, startedAt: now - 600_000, updatedAt: now,
  contact: null, cancelling: false, lostReason: null, host: null, backend: { kind: "local_job" }, command: "python pipeline.py", ...extra,
});
const experiment = { id: "e1", projectId: "p", slug: "distil-glue-6l", branchName: "distil", title: "Distil to a 6-layer student", description: null, runCommand: "python pipeline.py", agentStatus: "", createdAt: now - 900_000, updatedAt: now, archived: false };
const project = { id: "p", name: "glue", slug: "glue", githubOwner: "", githubRepo: "", baselineBranch: "main", repoPath: "/r", path: "/r", githubEnabled: false, artifactsDir: "/a" };

/** The experiment's overview over `runs`, newest first; `monitor(id)` answers each monitor read. */
async function overview(runs, monitor = () => pipeline()) {
  const requests = [];
  serveApp({
    "GET /api/compute/hosts": () => [],
    "GET /api/projects/p/experiments": () => ({ experiments: [experiment] }),
    "GET /api/projects/p/trackio/runs": () => ({ runs: [] }),
  });
  const served = globalThis.fetch;
  globalThis.fetch = async (input, init = {}) => {
    const path = new URL(String(input), location.href).pathname;
    requests.push(path);
    const read = /^\/api\/runs\/([^/]+)\/monitor$/.exec(path);
    if (!read) return served(input, init);
    const body = await monitor(read[1]);
    return body instanceof Response ? body : Response.json(body);
  };
  const props = (runs) => ({ experiment, lineageNode: null, onOpenExperiment() {}, parentExperiment: null, project, runs, onOpenLogs() {}, onOpenTrackio() {}, onOpenCode() {}, onOpenDiff() {} });
  const view = await render(createElement(ExperimentOverview, props(runs)), createWorkspace("test"));
  await settle();
  const update = async (next) => {
    await view.rerender(createElement(ExperimentOverview, props(next)));
    await settle();
  };
  return { ...view, requests, update };
}

test("the latest run draws its sentence, stages, bar and captions, then every block in the view's order", async () => {
  const monitor = pipeline();
  const { container } = await overview([run("r2", { monitor: summary(monitor) })]);
  assert.match(text(container), /train · step 2,280 of 4,200/i);
  const figures = [...container.querySelectorAll("strong")].map((strong) => text(strong));
  assert.ok(figures.includes("2,280") && figures.includes("4,200"), `the stage's numbers draw bold: ${figures}`);
  assert.deepEqual([...container.querySelectorAll("ol li")].map((stage) => text(stage).split(/\d/)[0].replace(/about\s*$/, "").trim()), ["download", "tokenize", "train", "eval", "export"]);
  assert.equal(container.querySelector('[role="progressbar"]').getAttribute("aria-valuenow"), "50");
  assert.match(text(container), /\d+m left/);
  assert.deepEqual([...container.querySelectorAll("section h3")].map(text), ["Eval", "Train loss"]);
  assert.deepEqual([...container.querySelectorAll("dl dt")].map((term) => term.textContent), ["GLUE average", "Train loss"]);
  const blocks = [...container.querySelectorAll('section[aria-label="Eval"], section[aria-label="Train loss"], dl')];
  assert.deepEqual(blocks.map((block) => block.tagName), ["SECTION", "SECTION", "DL"], "blocks keep the agent's order on the page, whatever stage is running");
});

test("how the view is built shows the view as the agent wrote it, where its signals come from and what an agent reads", async () => {
  const monitor = { ...pipeline(), viewFrom: { ...pipeline().viewFrom, experimentId: "e1" } };
  const { container } = await overview([run("r2", { monitor: summary(monitor) })], () => monitor);
  const details = container.querySelector("details");
  assert.equal(text(details.querySelector("summary")), m.run_view_how());
  assert.ok(text(details).includes("The view, stored on distil-glue-6l and inherited by its children"));
  assert.deepEqual([...details.querySelectorAll("pre")].map((pre) => pre.textContent), [monitor.viewFrom.markup, monitor.newestLine, monitor.statusText]);
  assert.ok(details.textContent.includes(m.run_view_source_declared()));
});

test("a default view with no markup says where it comes from and shows no code; a source from a newer server goes unnamed", async () => {
  const monitor = { ...pipeline(), viewFrom: { experimentId: "e1", markup: "", isDefault: true }, source: "wandb", newestLine: undefined };
  const { container } = await overview([run("r2", { monitor: summary(monitor) })], () => monitor);
  const details = container.querySelector("details");
  assert.ok(text(details).includes(m.run_view_default()));
  assert.deepEqual([...details.querySelectorAll("pre")].map((pre) => pre.textContent), [monitor.statusText]);
  assert.ok(!text(details).includes(m.run_view_source_none()));
});

test("a queued run draws none of its view's blocks", async () => {
  const monitor = { ...pipeline(), before: { kind: "queued", startsAt: null, text: null } };
  const { container } = await overview([run("r2", { status: "queued", startedAt: null, monitor: summary(monitor, { tone: "starting" }) })], () => monitor);
  assert.equal(container.querySelector("section h3"), null);
  assert.equal(container.querySelector("dl"), null);
});

test("a view with nothing to draw says why: nothing found in the log yet, nothing found at all, or a view set to show nothing", async () => {
  const empty = (viewFrom) => ({ ...pipeline(), view: { ...pipeline().view, blocks: [{ heatmap: "loss" }] }, viewFrom: { ...pipeline().viewFrom, ...viewFrom } });
  const said = async (runs, monitor) => text((await overview(runs, () => monitor)).container);
  const fresh = empty({ isDefault: true, markup: "" });
  assert.ok((await said([run("r2", { monitor: summary(fresh) })], fresh)).includes(m.run_page_no_signals()));
  const ended = run("r2", { status: "done", endedAt: now, monitor: summary(fresh, { tone: "done" }) });
  assert.ok((await said([ended], fresh)).includes(m.run_page_no_signals_ended()));
  const stored = empty({ isDefault: false });
  const words = await said([run("r2", { monitor: summary(stored) })], stored);
  assert.ok(words.includes(m.run_page_view_empty()) && !words.includes(m.run_page_no_signals()), words);
});

test("the page's clock does not redraw the blocks; new data does", async () => {
  const monitor = pipeline();
  const latest = run("r2", { monitor: summary(monitor) });
  const view = await overview([latest], () => monitor);
  // A chart block names itself with a list format, once each time it draws.
  const { ListFormat } = Intl;
  let drawn = 0;
  Intl.ListFormat = class extends ListFormat {
    constructor(...args) {
      super(...args);
      drawn++;
    }
  };
  try {
    await view.update([{ ...latest }]);
    assert.equal(drawn, 0, "the overview drew again, and its blocks did not");
    const key = view.workspace.queries.getRunMonitorQuery("r2").queryKey;
    await act(async () => view.workspace.queryClient.setQueryData(key, { ...monitor, rev: monitor.rev + 1 }));
    await settle();
    assert.ok(drawn > 0, "a new revision draws them");
  } finally {
    Intl.ListFormat = ListFormat;
  }
});

test("across a rebuilt view the blocks wait for its monitor, which is read again at once", async () => {
  const old = pipeline();
  const rebuilt = { ...pipeline(), generation: "g9", rev: 1, asOf: now + 1000, rows: { eval: [{ task: "sst2", score: 99.9 }] } };
  let answer = () => old;
  const view = await overview([run("r2", { monitor: summary(old) })], () => answer());
  const reads = () => view.requests.filter((path) => path === "/api/runs/r2/monitor").length;
  let release;
  answer = () => new Promise((resolve) => { release = () => resolve(rebuilt); });
  await view.update([run("r2", { monitor: summary(rebuilt) })]);
  assert.equal(reads(), 2, "read again without waiting");
  assert.equal(view.container.querySelector("section h3"), null, "no block draws the old basis meanwhile");
  assert.ok(view.container.querySelector('[aria-busy="true"]'));
  await act(async () => release());
  await settle();
  assert.ok(text(view.container).includes("sst299.9"));
});

test("a summary ahead of the view keeps the blocks and reads the view again two seconds later, once its own event hasn't come", async (t) => {
  const monitor = pipeline();
  const view = await overview([run("r2", { monitor: summary(monitor) })], () => monitor);
  const reads = () => view.requests.filter((path) => path === "/api/runs/r2/monitor").length;
  t.mock.timers.enable({ apis: ["setTimeout"] });
  await view.update([run("r2", { monitor: summary(monitor, { rev: monitor.rev + 1 }) })]);
  await act(async () => t.mock.timers.tick(1_900));
  await settle();
  assert.equal(reads(), 1, "its event usually lands first");
  assert.equal(view.container.querySelectorAll("section h3").length, 2, "the blocks keep drawing");
  await act(async () => t.mock.timers.tick(200));
  await settle();
  assert.equal(reads(), 2);
});

test("against an older server the latest run keeps today's status and log, and no view is read", async () => {
  const { container, requests } = await overview([run("r2")]);
  assert.equal(container.querySelector('[role="progressbar"]'), null);
  assert.ok(!requests.some((path) => path.endsWith("/monitor")));
});

test("while the view loads its room is held; a view that fails says so and tries again", async () => {
  let answer = () => new Promise(() => {});
  const monitor = pipeline();
  const latest = run("r2", { monitor: summary(monitor) });
  const view = await overview([latest], () => answer());
  assert.equal(view.container.querySelector('[aria-busy="true"]')?.className.includes("min-h-40"), true);
  assert.ok(view.container.querySelector('[role="progressbar"]'), "the bar draws from the run's summary meanwhile");

  answer = () => Response.json({ error: "The run's signals couldn't be read." }, { status: 503 });
  await act(async () => view.workspace.queryClient.resetQueries());
  await settle();
  assert.ok(text(view.container).includes(m.run_monitor_load_failed()));

  answer = () => Response.json(monitor);
  const retry = [...view.container.querySelectorAll("button")].find((button) => button.textContent === m.app_retry());
  await act(async () => retry.click());
  await settle();
  assert.ok(!text(view.container).includes(m.run_monitor_load_failed()));
  assert.equal(view.container.querySelectorAll("section h3").length, 2);
});

test("a view that stopped updating says why and since when, and the countdown stops", async () => {
  const monitor = { ...pipeline(), asOf: now - 120_000, availability: { state: "stale", reason: "The host stopped answering", omitted: {} } };
  const { container } = await overview([run("r2", { monitor: summary(monitor) })], () => monitor);
  assert.ok(text(container).includes(`${m.run_view_stale()} · The host stopped answering`));
  assert.doesNotMatch(text(container), /\d+m left/);
});

test("a run that can't be reached says so once, where its sentence goes", async () => {
  const monitor = pipeline();
  const contact = { type: "unreachable", since: now - 60_000, cause: "ssh: connect timed out" };
  const { container } = await overview([run("r2", { host: "lab-a40", contact, monitor: summary(monitor) })], () => monitor);
  const latest = [...container.querySelectorAll("section")].find((section) => section.querySelector("h2")?.textContent === m.experiment_latest_run());
  assert.equal(text(latest).split("Can't reach lab-a40").length - 1, 1);
});

test("run history says what each run finished with or where it stopped", async () => {
  const monitor = pipeline();
  const stopped = run("r1", {
    status: "cancelled", endedAt: now - 700_000, createdAt: now - 900_000, outcome: { by: "user", at: now - 700_000 },
    monitor: summary(monitor, { tone: "stopped", stage: { name: "", done: 26, of: 120, unit: null } }),
  });
  const done = run("r0", {
    status: "done", endedAt: now - 950_000, createdAt: now - 990_000,
    monitor: summary(monitor, { tone: "done", headline: { name: "loss", label: "Train loss", value: 0.41, vs: null } }),
  });
  const { container } = await overview([run("r2", { monitor: summary(monitor) }), stopped, done]);
  const rows = [...container.querySelectorAll(".experiment-run-history button")].map(text);
  assert.match(rows[1], /By you, at step 26 of 120\./);
  assert.match(rows[2], /Train loss 0\.41/);
});
