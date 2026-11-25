import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { act, createElement } from "react";
import { ExperimentOverview } from "../src/components/ExperimentOverview.tsx";
import { renderChat, session } from "./chat.mjs";
import { fmtClock } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { capitalized } from "../src/runMonitor/words.ts";
import { createWorkspace } from "../src/workspace.tsx";
import { find, render, serveApp, settle } from "./render.mjs";

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
  estimate: null, limitEndsAt: null, flag: null, rev: monitor.rev, generation: monitor.generation, asOf: monitor.asOf, ...extra,
});
const run = (id, extra = {}) => ({
  id, experimentId: "e1", projectId: "p", status: "running", createdAt: now - 600_000, startedAt: now - 600_000, updatedAt: now,
  contact: null, cancelling: false, lostReason: null, host: null, backend: { kind: "local_job" }, command: "python pipeline.py", ...extra,
});
const experiment = { id: "e1", projectId: "p", slug: "distil-glue-6l", branchName: "distil", title: "Distil to a 6-layer student", description: null, runCommand: "python pipeline.py", agentStatus: "", createdAt: now - 900_000, updatedAt: now, archived: false };
const project = { id: "p", name: "glue", slug: "glue", githubOwner: "", githubRepo: "", baselineBranch: "main", repoPath: "/r", path: "/r", githubEnabled: false, artifactsDir: "/a" };

/** The experiment's overview over `runs`, newest first; `monitor(id)` answers each monitor read. */
async function overview(runs, monitor = () => pipeline(), { chat = null, sessions = [], selectedRunId = null, hosts = [] } = {}) {
  const requests = [];
  const shown = { ...experiment, chatSessionId: chat };
  serveApp({
    "GET /api/compute/hosts": () => hosts,
    "GET /api/chat/sessions": () => ({ sessions }),
    "GET /api/projects/p/experiments": () => ({ experiments: [shown] }),
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
  const props = (runs) => ({ experiment: shown, lineageNode: null, onOpenExperiment() {}, parentExperiment: null, project, runs, selectedRunId, onOpenRun() {}, onOpenLogs() {}, onOpenTrackio() {}, onOpenCode() {}, onOpenDiff() {} });
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
  assert.match(text(container), /train \(2\/4\) · step 2,280 of 4,200/i);
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

/** The page's one Details disclosure, at its foot. */
const pageDetails = (container) => {
  const details = [...container.querySelectorAll("details")].filter((element) => text(element.querySelector("summary")) === m.run_details());
  assert.equal(details.length, 1, "one Details disclosure");
  return details[0];
};

test("Details, folded at the foot of the page, holds how the view is built, what the watcher was told and the rules", async () => {
  const brief = [{ label: "question", text: "Does the student keep 95% of GLUE?" }];
  const rules = [{ name: "nan-loss", text: "Stop when the loss is NaN.", does: "stop" }];
  const monitor = { ...pipeline(), brief, rules };
  const { container } = await overview([run("r2", { monitor: summary(monitor) })], () => monitor);
  const details = pageDetails(container);
  assert.equal(details.open, false);
  assert.equal(details.nextElementSibling, null, "nothing follows it");
  assert.deepEqual([...details.querySelectorAll("h3")].map(text), [m.run_rules_heading(), m.run_watch_told(), m.run_view_how()]);
  assert.ok(text(details).includes(`${m.run_brief_question()}Does the student keep 95% of GLUE?`), text(details));
  assert.equal(container.querySelectorAll("details").length, 1, "nothing else on the page folds away");
});

test("how the view is built shows the view as the agent wrote it, where its signals come from and what an agent reads", async () => {
  const monitor = { ...pipeline(), viewFrom: { ...pipeline().viewFrom, experimentId: "e1" } };
  const { container } = await overview([run("r2", { monitor: summary(monitor) })], () => monitor);
  const details = pageDetails(container);
  assert.ok(text(details).includes("The view, stored on distil-glue-6l and inherited by its children"));
  assert.deepEqual([...details.querySelectorAll("pre")].map((pre) => pre.textContent), [monitor.viewFrom.markup, monitor.statusText]);
  assert.ok(details.textContent.includes(m.run_view_source_declared()));
});

test("a default view with no markup says where it comes from and shows no code; a source from a newer server goes unnamed", async () => {
  const monitor = { ...pipeline(), viewFrom: { experimentId: "e1", markup: "", isDefault: true }, source: "wandb" };
  const { container } = await overview([run("r2", { monitor: summary(monitor) })], () => monitor);
  const details = pageDetails(container);
  assert.ok(text(details).includes(m.run_view_default()));
  assert.deepEqual([...details.querySelectorAll("pre")].map((pre) => pre.textContent), [monitor.statusText]);
  assert.ok(!text(details).includes(m.run_view_source_none()));
});

test("the rules a run was launched with are listed under Details, the one that stopped it marked", async () => {
  const rules = [
    { name: "nan-loss", text: "Stop when the loss is NaN.", does: "stop" },
    { name: "plateau", text: "Ask when val loss stops improving.", does: "ask" },
    { name: "budget", text: "Stop at $5 spent.", does: "stop" },
  ];
  const monitor = { ...pipeline(), rules };
  const outcome = { by: "rule", at: now, rule: rules[2], reason: "it would cost up to $41.20 by its end, past its $5.00 budget" };
  const stopped = run("r2", { status: "cancelled", endedAt: now, outcome, monitor: summary(monitor, { tone: "stopped" }) });
  const { container } = await overview([stopped], () => monitor);
  const budget = rules[2].text;
  assert.ok(text(container).includes(`${budget} ${m.run_rule_detail({ text: "", name: "budget" }).trim()}`), text(container));
  const details = pageDetails(container);
  assert.ok(text(details).includes(m.run_rules_when_active()));
  assert.deepEqual([...details.querySelectorAll("li")].map(text), [
    `nan-loss Stop when the loss is NaN. · ${m.run_rule_stops()}`,
    `plateau Ask when val loss stops improving. · ${m.run_rule_asks()}`,
    `budget ${budget} · ${m.run_rule_stops()}${m.run_rule_stopped_this()}`,
  ]);

  const { container: none } = await overview([run("r2", { monitor: summary(pipeline()) })], () => pipeline());
  assert.ok(![...pageDetails(none).querySelectorAll("h3")].some((heading) => text(heading) === m.run_rules_heading()));
});

test("a starting run draws none of its view's blocks", async () => {
  const monitor = pipeline();
  const { container } = await overview([run("r2", { status: "starting", startedAt: null, monitor: summary(monitor, { tone: "starting" }) })], () => monitor);
  assert.equal(container.querySelector("section h3"), null);
  assert.equal(container.querySelector("dl"), null);
});

test("a named stage shows its count and floor before the first metric", async () => {
  const estimate = { loMs: 330_000, midMs: 630_000, hiMs: 900_000, at: now, floor: true, pending: false, pastLimitMs: null };
  const monitor = {
    ...pipeline(), source: "declared", setup: null, estimate, latest: {}, headline: null,
    phases: [{ name: "download", of: 10, unit: "chunks", priorMs: null, startedAt: now - 600_000, endedAt: null }],
    stageOrder: { index: 0, count: null, first: 0 },
    progress: { phase: 0, done: 9, segments: [{ weight: 1, fill: 0.9 }], whole: 0.9, lastAt: now, stallMs: 0 },
    viewFrom: { ...pipeline().viewFrom, isDefault: true },
    view: { ...pipeline().view, headline: [], blocks: [] },
  };
  const { container } = await overview([run("r2", { monitor: summary(monitor, { setup: null, estimate }) })], () => monitor);
  const page = container.querySelector('[role="group"]');
  assert.match(text(page), /download · 9 of 10 chunks/);
  assert.match(text(page), /At least 10m left/);
  assert.match(text(page), /Timed from progress in the log\. More work may follow\./);
  assert.ok(text(page).includes(m.run_page_no_signals()));
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

test("a checkpoint read error does not claim the log had no metrics", async () => {
  const unreadable = readFileSync(new URL("../../tests/contracts/run-monitor-api.json", import.meta.url), "utf8");
  const { monitor } = JSON.parse(unreadable).states.unreadable;
  const data = { ...monitor, runId: "r2", asOf: now };
  const { container } = await overview([run("r2", { monitor: summary(data) })], () => data);
  const words = text(container);
  assert.ok(words.includes(data.availability.reason), words);
  assert.ok(!words.includes(m.run_page_no_signals()), words);
  assert.ok(!words.includes(m.run_view_source_none()), words);
});

test("an ended run with incomplete collection explains what was kept and still shows its progress", async () => {
  const base = pipeline();
  const monitor = {
    ...base,
    availability: { state: "live", reason: "The rental was deleted", collectionIncomplete: true },
    viewFrom: { ...base.viewFrom, isDefault: true },
    view: { ...base.view, blocks: [] },
  };
  const ended = run("r2", { status: "lost", endedAt: now, monitor: summary(monitor) });
  const { container } = await overview([ended], () => monitor);
  const page = container.querySelector('[role="group"]');
  assert.equal(text(page.querySelector('[role="status"]')), m.run_page_collection_incomplete());
  assert.equal(page.querySelector('[role="progressbar"]').getAttribute("aria-valuenow"), "50");
  assert.ok(!text(page).includes(m.run_page_no_signals_ended()));
  assert.ok(!text(pageDetails(container)).includes(m.run_view_source_none()));
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
  const rebuilt = { ...old, generation: "g9", rev: old.rev + 1, rows: { eval: [{ task: "sst2", score: 99.9 }] } };
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

test("a finished run's page shows its blocks during quiet final collection", async () => {
  const availability = { state: "stale", reason: null };
  const monitor = { ...pipeline(), availability };
  const finished = run("r2", { status: "done", endedAt: now, monitor: summary(monitor, { tone: "done", availability }) });
  const { container } = await overview([finished], () => monitor);
  assert.deepEqual([...container.querySelectorAll("section h3")].map(text), ["Eval", "Train loss"]);
  assert.ok(!text(container).includes(m.run_monitor_load_failed()));
  assert.ok(!find("button", m.app_retry()));
});

test("a run that can't be reached says so once with its next retry, where its sentence goes", async () => {
  const monitor = pipeline();
  const contact = { type: "unreachable", since: now - 60_000, cause: "ssh: connect timed out" };
  const nextRetryMs = now + 60_000;
  const hosts = [{ alias: "lab-a40", status: "backoff", class: "key", sinceMs: contact.since, nextRetryMs }];
  const { container } = await overview([run("r2", { host: "lab-a40", contact, monitor: summary(monitor) })], () => monitor, { hosts });
  const latest = [...container.querySelectorAll("section")].find((section) => section.querySelector("h2")?.textContent === m.experiment_latest_run());
  assert.equal(text(latest).split("Can't reach lab-a40").length - 1, 1);
  assert.equal(text(latest).split(m.run_notice_retry_at({ time: fmtClock(nextRetryMs) })).length - 1, 1);
});

test("run history says what each run finished with or where it stopped", async () => {
  const monitor = pipeline();
  const stopped = run("r1", {
    status: "cancelled", endedAt: now - 700_000, createdAt: now - 900_000, outcome: { by: "user", at: now - 700_000 },
    monitor: summary(monitor, { tone: "stopped", stage: { name: "", done: 26, of: 120, unit: null } }),
  });
  const done = run("r0", {
    status: "done", endedAt: now - 950_000, createdAt: now - 990_000,
    monitor: summary(monitor, { tone: "done", headline: { name: "loss", label: "Train loss", value: 0.41 } }),
  });
  const { container } = await overview([run("r2", { monitor: summary(monitor) }), stopped, done]);
  const rows = [...container.querySelectorAll(".experiment-run-history button")].map(text);
  assert.match(rows[1], /By you, at step 26 of 120\./);
  assert.match(rows[2], /Train loss 0\.41/);
});

test("run history opens the older SSH run's chart, stats and journal while the latest attempt is unreachable", async () => {
  const older = { ...pipeline(), runId: "r1", estimate: null, watch: fixture("running").watch };
  const latest = pipeline();
  const ssh = { backend: { kind: "ssh_job" }, host: "lab-a40" };
  const previous = run("r1", { ...ssh, status: "done", endedAt: now - 630_000, createdAt: now - 900_000, monitor: summary(older, { tone: "done" }) });
  const unreachable = run("r2", { ...ssh, contact: { type: "unreachable", since: now - 60_000, cause: "timed out" }, monitor: summary(latest) });
  const pane = encodeURIComponent(JSON.stringify({ kind: "experiment", experimentId: "e1", view: "overview" }));
  const { container, requests } = await renderChat(`/projects/p/tasks/s?pane=${pane}`, { routes: {
    "GET /api/projects": () => ({ projects: [project] }),
    "GET /api/projects/p/experiments": () => ({ experiments: [experiment] }),
    "GET /api/projects/p/runs": () => ({ runs: [unreachable, previous] }),
    "GET /api/runs/r1/monitor": () => older,
    "GET /api/runs/r2/monitor": () => latest,
    "GET /api/compute/hosts": () => [],
  } });
  assert.ok(find("h2", m.experiment_latest_run()));
  const row = () => find(".experiment-run-history button", "Run 1");
  await act(async () => row().click());
  await settle();
  assert.ok([...container.querySelectorAll("h2")].some((heading) => text(heading) === "Run 1"));
  assert.equal(row().getAttribute("aria-current"), "true");
  assert.ok(container.querySelector('section[aria-label="Train loss"] svg[role="img"]'));
  assert.ok(find("dt", "GLUE average"));
  const journal = find(`ol[aria-label="${m.run_watch_journal()}"]`);
  assert.match(text(journal), /falling steadily/);
  assert.ok(requests.some(({ request }) => request === "GET /api/runs/r1/monitor"));
  assert.ok(!requests.some(({ request }) => request === "GET /api/runs/r1/log"));
});

test("a flagged run only names the agent when the launch chat is known", async () => {
  const flagged = fixture("flagged");
  const waiting = { ...flagged, runId: "r2", asOf: now, flags: [{ ...flagged.flags[0], state: "waiting" }] };
  const flaggedRun = () => run("r2", { monitor: summary(waiting, { tone: "flag", sentence: flagged.sentence, flag: { id: "f1", state: "waiting" } }) });
  const asks = (container) => [...container.querySelectorAll("button")].map(text);

  const subscribed = await overview([flaggedRun()], () => waiting, { chat: "s1", sessions: [{ ...session("s1"), harness: "claude-code" }] });
  assert.ok(asks(subscribed.container).includes(m.run_flag_ask_unnamed()), asks(subscribed.container).join(" | "));
  await subscribed.unmount();

  const launched = await overview([{ ...flaggedRun(), chatSessionId: "s2" }], () => waiting, { chat: "s1", sessions: [{ ...session("s1"), harness: "codex" }, { ...session("s2"), harness: "claude-code" }] });
  assert.ok(asks(launched.container).includes(m.run_flag_ask({ agent: "Claude Code" })), "the run's own chat wins over the experiment's");
  await launched.unmount();

  const unnamed = await overview([flaggedRun()], () => waiting);
  assert.ok(asks(unnamed.container).includes("Ask the agent"), asks(unnamed.container).join(" | "));
  await unnamed.unmount();
});


test("opening an older run reads that run's monitor and journal instead of the latest run", async () => {
  const monitor = pipeline();
  const runs = [run("r2", { host: "lab-newer", contact: { type: "unreachable", since: now - 60_000, cause: "SSH timed out" }, monitor: summary(monitor) }), run("r1", { createdAt: now - 900_000, monitor: summary(monitor) })];
  const { container, requests } = await overview(runs, (id) => ({ ...monitor, runId: id }), { selectedRunId: "r1" });
  assert.ok(![...container.querySelectorAll("h2")].some((heading) => text(heading) === m.experiment_latest_run()));
  const heading = [...container.querySelectorAll("h2")].find((heading) => text(heading) === capitalized(m.run_number({ number: "1" })));
  assert.ok(heading);
  assert.ok(!text(heading.closest("section")).includes("lab-newer"));
  assert.ok(requests.includes("/api/runs/r1/monitor"));
  assert.ok(!requests.includes("/api/runs/r2/monitor"));
});

test("a run page explains when its chat has no watching model or sign-in", async () => {
  for (const watch of [
    { state: "needs_pick", why: null },
    { state: "no_model", why: "Sign in to Claude Code on this workspace" },
  ]) {
    const monitor = pipeline();
    monitor.watch = { ...monitor.watch, ...watch, looks: [], model: null };
    const { container, unmount } = await overview([run("r2", { monitor: summary(monitor) })], () => monitor);
    const section = [...container.querySelectorAll("section")].find((section) => section.querySelector("h2")?.textContent === m.run_watch_heading());
    assert.ok(section);
    assert.match(text(section), /^WatchNot watched/);
    if (watch.why) assert.ok(text(section).includes(watch.why));
    assert.ok(![...section.querySelectorAll("button")].some((button) => text(button) === m.run_watch_look_now()));
    await unmount();
  }
});

test("a run that never started says why where its sentence goes and says once that it never started, with no stages, clock or charts", async () => {
  const monitor = { ...pipeline(), progress: null, stageOrder: { index: null, count: 5, first: null }, latest: {}, series: {}, estimate: null };
  const reason = "There was no L40S free to rent. Try another GPU, or launch again.";
  const never = run("r2", { status: "failed", startedAt: null, endedAt: now - 1_000, resultMarkdown: reason, monitor: summary(monitor, { tone: "failed" }) });
  const { container } = await overview([never], () => monitor);
  const latest = [...container.querySelectorAll("section")].find((section) => section.querySelector("h2")?.textContent === m.experiment_latest_run());
  assert.equal(text(latest).split(reason).length - 1, 1, "the provider's reason is said once, in the run's sentence");
  assert.equal(text(latest).split(m.run_page_never_started()).length - 1, 1);
  assert.equal(latest.querySelector('[aria-current="step"]'), null, "no stage is marked as the one it failed in");
  assert.equal(latest.querySelector('[role="progressbar"]'), null);
  assert.equal(latest.querySelectorAll('section[aria-label="Eval"], section[aria-label="Train loss"], dl').length, 0, "no empty charts");
  assert.ok(!/Failed after/.test(text(latest)), text(latest));
  const row = text(container.querySelector(".experiment-run-history button"));
  assert.ok(row.includes(reason) && row.includes("—"), row);
});

test("a Ray run that failed between polls keeps its metric charts and duration", async () => {
  const monitor = pipeline();
  const failed = run("r2", {
    status: "failed", startedAt: null, exitCode: null, endedAt: now,
    backend: { kind: "ray_job", jobId: "r2" }, resultMarkdown: "The job failed.",
    monitor: summary(monitor, { tone: "failed", bindings: monitor.latest }),
  });
  const { container } = await overview([failed], () => monitor);
  const latest = [...container.querySelectorAll("section")].find((section) => section.querySelector("h2")?.textContent === m.experiment_latest_run());
  assert.ok(text(latest).includes(failed.resultMarkdown));
  assert.ok(!text(latest).includes(m.run_page_never_started()));
  assert.ok(latest.querySelector('section[aria-label="Train loss"] svg[role="img"]'));
  assert.ok(latest.querySelector(`[title="${m.experiment_overview_duration()}"]`));
});

test("run history says why each failed run failed: the provider's words, else the log's last error", async () => {
  const monitor = pipeline();
  const provider = run("r3", { status: "failed", startedAt: null, endedAt: now - 1_000, resultMarkdown: "The provider couldn't start the box. Launch again.", monitor: summary(monitor, { tone: "failed" }) });
  const crashed = run("r2", {
    status: "failed", exitCode: 1, createdAt: now - 900_000, startedAt: now - 600_000, endedAt: now - 300_000,
    monitor: summary(monitor, { tone: "failed", logError: "ImportError: cannot import name 'Parakeet'", stage: { name: "label", done: 2543, of: 5086, unit: "clips" } }),
  });
  const { container } = await overview([provider, crashed], () => monitor);
  const rows = [...container.querySelectorAll(".experiment-run-history button")].map(text);
  assert.match(rows[0], /The provider couldn't start the box\. Launch again\./);
  assert.match(rows[1], /ImportError: cannot import name 'Parakeet'/);
  assert.match(rows[1], /5m/, "a run that ran is timed from its start, not from when it was launched");
});
