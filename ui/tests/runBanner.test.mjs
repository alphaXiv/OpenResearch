import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test, { afterEach } from "node:test";
import { act, createElement } from "react";
import { DetailDrawer } from "../src/components/DetailDrawer.tsx";
import { RunBanner } from "../src/components/run/RunBanner.tsx";
import { fmtClock } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { QueryEvents } from "../src/queries/Events.tsx";
import { chooseFlag } from "../src/queries/projects.ts";
import { createWorkspace } from "../src/workspace.tsx";
import { find, press, render, serve, settle, streams } from "./render.mjs";

const fixture = (name) => JSON.parse(readFileSync(new URL(`./fixtures/runMonitor/${name}.json`, import.meta.url), "utf8"));
const running = fixture("running");
const flagged = fixture("flagged");
const now = Date.now();
afterEach(() => window.sessionStorage.clear());
const summary = (extra = {}) => ({
  tone: "running", whole: 0.248, headline: { name: "loss", label: "Loss", value: 2.31 }, sentence: running.sentence,
  bindings: { loss: 2.31 }, signals: running.view.signals, estimate: null, flag: null, rev: 1, generation: "g1", asOf: now, ...extra,
});
const run = (id, extra = {}) => ({
  id, experimentId: `e-${id}`, projectId: "p", chatSessionId: "s", status: "running", createdAt: now - 60_000, startedAt: now - 60_000, updatedAt: now,
  contact: null, cancelling: false, lostReason: null, host: null, backend: { kind: "local_job" }, monitor: summary(), ...extra,
});
const item = (value) => ({ sessionId: "s", run: value, experimentName: value.id, experimentSlug: `lr-${value.id}`, runNo: 1, runCount: 1 });

/** Serves today's server for one chat's runs; every request is kept. */
function backend({ uiState = { onboardingCompleted: true }, monitors = { r1: running }, hosts = [], flagRefusal, retryRefusal } = {}) {
  const requests = [];
  serve((path, init) => {
    const request = `${init.method ?? "GET"} ${path}`;
    requests.push({ request, body: init.body ? JSON.parse(init.body) : undefined });
    if (request === "GET /api/settings/ui-state") return uiState;
    if (request === "POST /api/settings/ui-state") return { ...uiState };
    if (request === "GET /api/compute/hosts") return hosts;
    const monitor = /^GET \/api\/runs\/([^/]+)\/monitor$/.exec(request);
    if (monitor) return monitors[monitor[1]];
    if (/^POST \/api\/runs\/[^/]+\/cancel$/.test(request)) return { ok: true };
    if (retryRefusal && /^POST \/api\/compute\/hosts\/[^/]+\/retry$/.test(request)) return Response.json({ error: retryRefusal }, { status: 409 });
    if (/^POST \/api\/runs\/[^/]+\/flag$/.test(request)) {
      const refusal = flagRefusal?.();
      return refusal ? Response.json({ error: refusal }, { status: 409 }) : { flag: { ...flagged.flags[0], state: "kept" }, run: run("r1") };
    }
    if (/^POST \/api\/chat\/sessions\/s\/shown-runs\/[^/]+\/dismiss$/.test(request)) return { dismissed: true };
  });
  return requests;
}

/** Chat s's banner over `runs` as the shown-runs query holds them; `update` replaces them as an event would. */
async function banner(runs, { shown = true, workspace = createWorkspace("test"), props = {} } = {}) {
  const { queryKey } = workspace.queries.listChatShownRunsQuery();
  const hold = (next) => workspace.queryClient.setQueryData(queryKey, { source: shown ? "shown" : "active", runs: next.map(item) });
  hold(runs);
  const view = await render(createElement(RunBanner, { sessionId: "s", agent: "Claude", onOpenRunPage: () => {}, ...props }), workspace);
  return { ...view, update: (next) => act(async () => hold(next)) };
}
const button = (label) => find(`button[aria-label="${label}"]`);
const click = async (element) => { await act(async () => element.click()); await settle(); };
const status = () => document.querySelector('[role="status"].sr-only').textContent;

test("one run opens as its strip; the chevrons and Escape step between line and strip, keeping focus on a chevron", async () => {
  const requests = backend();
  const { container } = await banner([run("r1")]);
  await settle();
  assert.ok(button(m.run_show_less()), "the strip is the default");
  assert.ok(container.querySelector('[role="img"][aria-label*="Loss"]'), "the strip draws the chart");
  assert.equal(requests.filter((r) => r.request === "GET /api/runs/r1/monitor").length, 1);

  await click(button(m.run_show_less()));
  assert.ok(button(m.run_show_more()));
  assert.equal(document.activeElement, button(m.run_show_more()));
  assert.deepEqual(requests.find((r) => r.request === "POST /api/settings/ui-state").body, { runBanner: "line" });
  await press("Escape");
  assert.ok(button(m.run_show_more()), "Escape at the line does nothing");

  await click(button(m.run_show_more()));
  assert.equal(document.activeElement, button(m.run_show_less()));
  await press("Escape");
  await settle();
  assert.ok(button(m.run_show_more()));
  assert.equal(document.activeElement, button(m.run_show_more()));
});

test("a strip's chart names what it draws beside a headline it doesn't draw", async () => {
  const view = { ...running.view, signals: { ...running.view.signals, test_acc: { label: "Test accuracy", digits: 3, better: "higher" } }, headline: ["test_acc"] };
  backend({ monitors: { r1: { ...running, view, headline: "test_acc", latest: { ...running.latest, test_acc: 0.896 } } } });
  const { container } = await banner([run("r1")]);
  await settle();
  const hero = container.querySelector('[role="img"][aria-label*="Loss"]').closest(".flex-col");
  assert.equal(hero.firstElementChild.textContent.replace(/[⁦-⁩]/g, ""), "Loss");
  assert.ok(container.textContent.includes("Test accuracy"));
});

test("the remembered level opens the line, and a run waiting for a choice opens its strip anyway", async () => {
  const monitors = { r1: running };
  backend({ uiState: { onboardingCompleted: true, runBanner: "line" }, monitors });
  const workspace = createWorkspace("test");
  const view = await banner([run("r1")], { workspace });
  await settle();
  assert.ok(button(m.run_show_more()));
  monitors.r1 = flagged;
  await view.update([run("r1", { monitor: summary({ tone: "flag", sentence: flagged.sentence, bindings: { loss: "NaN" }, flag: { id: "f1", state: "waiting" }, rev: flagged.rev }) })]);
  await settle();
  assert.ok(button(m.run_show_less()), "the strip opened by itself");
  await act(async () => workspace.queryClient.refetchQueries({ queryKey: workspace.queries.getRunMonitorQuery("r1").queryKey }));
  await settle();
  assert.ok(find("button", m.run_flag_keep_going()));
  assert.ok(find("pre", "::orx::"), "the choices and evidence show once the monitor has them");
});

test("Stop asks first, with Cancel focused; Escape and Cancel keep the run, and Stop run sends one cancel", async () => {
  const requests = backend();
  await banner([run("r1")]);
  await settle();
  const cancels = () => requests.filter((r) => r.request === "POST /api/runs/r1/cancel").length;
  const dialog = () => document.querySelector('[role="dialog"]');

  await click(button(m.run_stop()));
  assert.ok(dialog().textContent.replace(/[⁦-⁩]/g, "").includes(m.run_stop_title({ name: "r1" })), "the title names the run");
  assert.ok(dialog().textContent.includes(m.run_stop_effect()));
  assert.equal(document.activeElement, find('[role="dialog"] button', m.common_cancel()));
  assert.equal(cancels(), 0, "opening the dialog stops nothing");
  await press("Escape");
  await settle();
  assert.equal(dialog(), null);
  assert.equal(cancels(), 0, "Escape keeps the run");

  await click(button(m.run_stop()));
  await click(find('[role="dialog"] button', m.common_cancel()));
  assert.equal(cancels(), 0, "Cancel keeps the run");

  await click(button(m.run_stop()));
  const confirm = find('[role="dialog"] button', m.experiments_stop_run());
  await act(async () => confirm.click());
  await act(async () => confirm.click());
  await settle();
  assert.equal(cancels(), 1, "a second press while the first is out sends nothing");
});

test("against an older server the line reads from the run's status and never asks for its monitor", async () => {
  const requests = backend();
  const { container } = await banner([run("r1", { monitor: undefined })], { shown: false });
  await settle();
  assert.equal(button(m.run_show_less()), null);
  assert.ok(container.textContent.includes(m.status_running()), container.textContent);
  assert.equal(requests.filter((r) => r.request.includes("/monitor")).length, 0);
  assert.equal(button(m.run_dismiss()), null);
});

test("a new sentence is announced once; a number refilling is not", async () => {
  backend();
  const matching = (extra = {}) => run("r1", { monitor: summary({ stageOrder: running.stageOrder, stage: { name: "train", done: 1240, of: 5000, unit: "steps" }, ...extra }) });
  const view = await banner([matching()]);
  await settle();
  assert.equal(status(), "");
  await view.update([matching({ bindings: { loss: 2.1 } })]);
  await settle();
  assert.equal(status(), "");
  const next = { template: "Loss **{loss}**, flattening out.", t: now };
  await view.update([matching({ sentence: next, bindings: { loss: 2.1 }, rev: running.rev + 1 })]);
  await settle();
  assert.equal(status().replace(/[⁦-⁩]/g, ""), "train (2/2) · Step 1,240 of 5,000. Loss 2.10, flattening out.");
});

test("the status holds an announcement only until it is made, and nothing of a run that left", async (t) => {
  const reading = new Promise(() => {});
  backend({ uiState: { onboardingCompleted: true, runBanner: "line" }, monitors: { r1: reading, r2: reading } });
  const plain = () => status().replace(/[⁦-⁩]/g, "");
  // Each a newer summary than the last, as the server's events are.
  const step = (done) => summary({ sentence: null, headline: null, stage: { name: "", done, of: 45, unit: null }, rev: done + 2 });
  const view = await banner([run("r1", { status: "starting", startedAt: null, monitor: summary({ tone: "starting", sentence: null, headline: null }) }), run("r2")]);
  await settle();
  assert.equal(plain(), "");
  t.mock.timers.enable({ apis: ["setTimeout"] });
  // The query cache hands on its updates in a zero-delay timeout.
  const update = async (runs) => { await view.update(runs); await act(async () => t.mock.timers.tick(1)); await settle(); };

  await update([run("r1", { monitor: step(0) }), run("r2")]);
  assert.ok(plain().includes("Step 0 of 45"), plain());
  await act(async () => t.mock.timers.tick(2_000));
  assert.equal(plain(), "", "said, then emptied");
  for (const done of [1, 20, 44]) {
    await update([run("r1", { monitor: step(done) }), run("r2")]);
    assert.equal(plain(), "", `a step filling is never news, and no old step is held: ${done}`);
  }

  await update([run("r1", { monitor: step(44) }), run("r2", { status: "done", endedAt: now, monitor: summary({ tone: "done", rev: 2 }) })]);
  assert.ok(plain().startsWith("r2: "), plain());
  await update([run("r1", { monitor: step(44) })]);
  assert.equal(plain(), "", "a run that left is not read out");
});

test("the end is announced once, as how long the run took, never as the last step a stale summary held", async () => {
  backend({ monitors: { r1: { ...running, sentence: null } } });
  const stale = summary({ sentence: null, stage: { name: "", done: 58, of: 60, unit: null } });
  const view = await banner([run("r1", { monitor: stale })]);
  await settle();
  await view.update([run("r1", { status: "done", endedAt: now, monitor: stale })]);
  await settle();
  const said = status().replace(/[⁦-⁩]/g, "");
  assert.equal(said.trim(), m.run_done_in({ duration: "1m" }).replace(/[⁦-⁩]/g, ""));
});

const sweep = (id, depth) => ({ ...item(run(id)), experimentName: `Depth ${depth} at 2e15 FLOPs`, experimentSlug: `depth-${depth}` });
/** Chat s's banner over shown runs as given, titles and all. */
async function several(items, { workspace = createWorkspace("test"), onOpenRunPage = () => {} } = {}) {
  workspace.queryClient.setQueryData(workspace.queries.listChatShownRunsQuery().queryKey, { source: "shown", runs: items });
  return render(createElement(RunBanner, { sessionId: "s", agent: "Claude", onOpenRunPage }), workspace);
}
const rows = () => [...document.querySelectorAll('[role="list"] > li > [role="group"]')];

test("several runs show one summary over a card each, all open without a click and named by what sets it apart", async () => {
  const requests = backend({ monitors: { r1: running, r2: running, r3: running } });
  const runs = [sweep("r1", 4), sweep("r2", 6), sweep("r3", 8)];
  const { container, unmount } = await several(runs);
  await settle();
  assert.ok(plain(container.textContent).includes("Depth 4, 6, 8 at 2e15 FLOPs"), "the summary names every run by what they share");
  assert.deepEqual(rows().map((row) => row.getAttribute("aria-label")), ["Depth 4 at 2e15 FLOPs", "Depth 6 at 2e15 FLOPs", "Depth 8 at 2e15 FLOPs"]);
  assert.deepEqual(rows().map((row) => plain(row.querySelector('button[aria-haspopup="dialog"]').textContent).split(" · ")[0]), ["4", "6", "8"]);
  assert.ok(plain(rows()[0].querySelector('button[aria-haspopup="dialog"]').getAttribute("aria-label")).includes("Depth 4 at 2e15 FLOPs"), "the accessible name includes the whole title");
  assert.deepEqual(rows().map((row) => Boolean(row.querySelector(`button[aria-label="${m.run_show_less()}"]`) && row.querySelector('[role="img"][aria-label*="Loss"]'))), [true, true, true], "every card is open with its chart");
  assert.equal(requests.filter((r) => r.request.endsWith("/monitor")).length, 3, "each card reads its monitor once");

  await click(rows()[1].querySelector(`button[aria-label="${m.run_show_less()}"]`));
  assert.equal(document.activeElement, rows()[1].querySelector(`button[aria-label="${m.run_show_more()}"]`));
  assert.equal(rows()[1].querySelector('[role="img"][aria-label*="Loss"]'), null, "the folded card hides its chart");
  assert.ok(rows()[2].querySelector('[role="img"][aria-label*="Loss"]'), "and leaves the others open");

  await press("Escape", rows()[0].querySelector(`button[aria-label="${m.run_show_less()}"]`));
  await settle();
  assert.equal(rows().length, 3);
  assert.equal(document.activeElement, rows()[0].querySelector(`button[aria-label="${m.run_show_more()}"]`), "Escape folds the card it is in");
  await press("Escape");
  await settle();
  assert.equal(rows().length, 0, "the second Escape folds the rows into the summary");
  assert.equal(document.activeElement, button(m.run_show_each()));
  assert.equal(container.querySelectorAll('[role="progressbar"]').length, 3, "every run's bar stacks under it");
  assert.deepEqual(requests.filter((r) => r.request === "POST /api/settings/ui-state").map((r) => r.body), [{ runBanner: "line" }]);
  await press("Escape");
  assert.ok(button(m.run_show_each()), "Escape at the summary does nothing");

  await click(button(m.run_show_each()));
  assert.equal(rows().length, 3);
  assert.equal(document.activeElement, button(m.run_show_less()));
  assert.deepEqual(rows().map((row) => row.querySelector("[data-run-toggle]").getAttribute("aria-expanded")), ["false", "false", "true"], "a card the user folded stays folded");
  assert.equal(requests.filter((r) => r.request.endsWith("/monitor")).length, 3, "reopening reads nothing again");

  await unmount();
  await several(runs, { workspace: createWorkspace("reloaded-workspace") });
  await settle();
  assert.deepEqual(rows().map((row) => row.querySelector("[data-run-toggle]").getAttribute("aria-expanded")), ["false", "false", "true"]);
});

test("a setup line whose only stat has no value yet has no card, and gains one with its first value", async () => {
  const setup = { ...running, sentence: null, setup: { stage: "Label", line: null, startsAt: null }, series: {}, latest: {}, headline: "test_wer",
    view: { ...running.view, signals: { ...running.view.signals, test_wer: { label: "Test WER", digits: 2 } }, headline: ["test_wer"] },
    phases: [{ name: "Label", of: 5086, unit: "clips", priorMs: null, startedAt: now, endedAt: null }],
    progress: { ...running.progress, phase: 0, done: 4351, whole: null, segments: [{ weight: 1, fill: 4351 / 5086 }] } };
  backend({ monitors: { r1: setup, r2: running } });
  const workspace = createWorkspace("test");
  const pending = summary({ headline: { name: "test_wer", label: "Test WER", value: null } });
  await several([{ ...sweep("r1", 4), run: run("r1", { monitor: pending }) }, sweep("r2", 6)], { workspace });
  await settle();
  assert.equal(rows()[0].querySelector("[data-run-toggle]"), null, "no chevron and no card");
  assert.ok(!plain(rows()[0].textContent).includes("Test WER"), "the empty stat isn't drawn");
  assert.equal(rows()[1].querySelector("[data-run-toggle]").getAttribute("aria-expanded"), "true", "a run beside it with a chart is open");

  const { queryKey } = workspace.queries.getRunMonitorQuery("r1");
  await act(async () => workspace.queryClient.setQueryData(queryKey, { ...setup, rev: setup.rev + 1, latest: { test_wer: 3.55 } }));
  await settle();
  assert.equal(rows()[0].querySelector("[data-run-toggle]").getAttribute("aria-expanded"), "true", "the first value opens its card");
  assert.ok(plain(rows()[0].querySelector(".text-xl").textContent).includes("3.55"));
});

test("eight runs all open and each read its monitor once", async () => {
  const ids = Array.from({ length: 8 }, (_, index) => `r${index + 1}`);
  const requests = backend({ monitors: Object.fromEntries(ids.map((id) => [id, running])) });
  await several(ids.map((id, index) => sweep(id, index + 2)));
  await settle();
  assert.equal(rows().length, 8);
  assert.equal(document.querySelectorAll(`[role="list"] button[aria-label="${m.run_show_less()}"]`).length, 8);
  assert.equal(requests.filter((r) => r.request.endsWith("/monitor")).length, 8);
  const list = find('[role="list"]');
  assert.ok(!list.contains(button(m.run_show_less())), "the summary's toggle stays outside the scrolling list");
  assert.ok(list.compareDocumentPosition(button(m.run_show_less())) & Node.DOCUMENT_POSITION_PRECEDING, "and comes first in Tab order");
});

test("an artifact change refreshes only open rows in view in a list of eight runs", async (t) => {
  const ids = Array.from({ length: 8 }, (_, index) => `r${index + 1}`);
  const monitor = { ...running, media: { "samples/*.png": { items: [], error: { kind: "unreadable" } } } };
  const requests = backend({ monitors: Object.fromEntries(ids.map((id) => [id, monitor])) });
  const workspace = createWorkspace("test");
  await render(createElement(QueryEvents), workspace);
  let intersect;
  const observed = new Set();
  t.mock.method(IntersectionObserver.prototype, "observe", function (target) {
    intersect = this.callback;
    observed.add(target);
    intersect([{ target, isIntersecting: true }]);
  });
  await several(ids.map((id, index) => sweep(id, index + 2)), { workspace });
  await settle();
  await click(rows()[1].querySelector(`button[aria-label="${m.run_show_less()}"]`));
  const offscreen = [...observed].slice(3).map((target) => ({ target, isIntersecting: false }));
  await act(async () => intersect?.(offscreen));
  await settle();

  requests.length = 0;
  t.mock.timers.enable({ apis: ["Date", "setTimeout"] });
  await act(async () => t.mock.timers.tick(15_000));
  const artifactsChanged = () => act(async () => streams[0].dispatchEvent(new MessageEvent("files.updated", { data: '{"projectId":"p"}' })));
  await artifactsChanged();
  await act(async () => t.mock.timers.tick(15_000));
  await settle();
  const reads = () => requests.map(({ request }) => request).filter((request) => request.endsWith("/monitor")).sort();
  assert.deepEqual(reads(), ["GET /api/runs/r1/monitor", "GET /api/runs/r3/monitor"]);

  requests.length = 0;
  await act(async () => intersect([{ target: offscreen.at(-1).target, isIntersecting: true }]));
  await settle();
  assert.deepEqual(reads(), ["GET /api/runs/r8/monitor"], "a row refreshes when scrolled back into view");
});

test("a summary lists its runs in the order they were launched", async () => {
  backend({ monitors: {} });
  const launched = (id, depth, at) => ({ ...sweep(id, depth), run: run(id, { createdAt: at }) });
  await several([launched("r3", 8, now - 1_000), launched("r1", 4, now - 3_000), launched("r2", 6, now - 2_000)]);
  await settle();
  assert.deepEqual(rows().map((row) => row.getAttribute("aria-label")), ["Depth 4 at 2e15 FLOPs", "Depth 6 at 2e15 FLOPs", "Depth 8 at 2e15 FLOPs"]);
});

test("sweep rows stop counting down when their summaries go stale or rebuild while their monitors are read", async () => {
  const reading = new Promise(() => {});
  const requests = backend({ monitors: { r1: reading, r2: reading } });
  const estimate = { ...running.estimate, at: now };
  const items = [sweep("r1", 4), sweep("r2", 6)].map((item) => ({
    ...item, run: { ...item.run, monitor: summary({ estimate }) },
  }));
  const workspace = createWorkspace("test");
  await several(items, { workspace });
  await settle();
  assert.match(plain(rows()[0].textContent), /\d+[mh].*left/);

  const { queryKey } = workspace.queries.listChatShownRunsQuery();
  const reasons = ["The host stopped answering.", "The run's log was replaced."];
  await act(async () => workspace.queryClient.setQueryData(queryKey, {
    source: "shown",
    runs: items.map((item, index) => ({
      ...item, run: { ...item.run, monitor: { ...item.run.monitor, rev: 2, availability: { state: index === 0 ? "stale" : "rebuilding", reason: reasons[index] } } },
    })),
  }));
  await settle();
  rows().forEach((row, index) => {
    assert.ok(plain(row.textContent).includes(plain(m.status_last_seen({ time: fmtClock(now) }))));
    assert.doesNotMatch(plain(row.textContent), /\d+[mh].*left/);
    assert.ok(plain(row.textContent).includes(reasons[index]), "the open card says why");
  });
  assert.equal(requests.filter((r) => r.request.endsWith("/monitor")).length, 2);
});

test("a summary says who needs attention first, and a run waiting for a choice opens itself from a folded summary", async () => {
  const monitors = { r1: running, r2: running };
  backend({ uiState: { onboardingCompleted: true, runBanner: "line" }, monitors });
  const workspace = createWorkspace("test");
  await several([sweep("r1", 4), sweep("r2", 6)], { workspace });
  await settle();
  assert.equal(rows().length, 0, "the remembered line folds the summary");
  assert.ok(button(m.run_show_each()));

  monitors.r2 = { ...flagged, flags: [{ ...flagged.flags[0], detail: flagged.flags[0].template }] };
  const waiting = run("r2", { monitor: summary({ tone: "flag", sentence: flagged.sentence, bindings: { loss: "NaN" }, flag: { id: "f1", state: "waiting", text: "Loss is NaN." }, rev: flagged.rev }) });
  await act(async () => workspace.queryClient.setQueryData(workspace.queries.listChatShownRunsQuery().queryKey, { source: "shown", runs: [sweep("r1", 4), { ...sweep("r2", 6), run: waiting }] }));
  await settle();
  const warning = "Loss is NaN.";
  assert.equal(plain(find("[data-run-frame]").textContent).split(warning).length - 1, 1);
  const head = plain(find("button[aria-expanded]").textContent);
  assert.ok(head.startsWith(plain(m.run_status_for({ name: "6", status: m.status_flagged() }))), head);
  assert.equal(rows().length, 2, "the rows opened");
  assert.ok(rows()[1].querySelector(`button[aria-label="${m.run_show_less()}"]`), "and so did the waiting run's strip");
  assert.ok(plain(rows()[1].textContent).includes(warning));

  await click(find('[data-run-toggle="*"]'));
  assert.equal(rows().length, 0);
  assert.ok(plain(find("button[aria-expanded]").textContent).includes(warning));
});

test("a finished sweep stays one summary, which is dismissed whole once none of it runs", async () => {
  const requests = backend({ monitors: {} });
  const ended = (id, depth) => ({ ...sweep(id, depth), run: run(id, { status: "done", endedAt: now, monitor: summary({ tone: "done" }) }) });
  const workspace = createWorkspace("test");
  await several([ended("r1", 4), sweep("r2", 6)], { workspace });
  await settle();
  assert.equal(document.querySelector(`button[aria-label="${m.run_dismiss_all()}"]`), null, "not while one still runs");
  await act(async () => workspace.queryClient.setQueryData(workspace.queries.listChatShownRunsQuery().queryKey, { source: "shown", runs: [ended("r1", 4), ended("r2", 6)] }));
  await settle();
  await click(button(m.run_dismiss_all()));
  assert.deepEqual(requests.filter((r) => r.request.endsWith("/dismiss")).map((r) => r.request).sort(), [
    "POST /api/chat/sessions/s/shown-runs/r1/dismiss",
    "POST /api/chat/sessions/s/shown-runs/r2/dismiss",
  ]);
});

test("a summary of runs with no shared words names each by its whole title and opens it", async () => {
  backend();
  const opens = [];
  const { container } = await several([item(run("r1")), item(run("r2"))], { onOpenRunPage: (...args) => opens.push(args) });
  await settle();
  assert.ok(!plain(container.textContent).includes("…"));
  await click(find('button[aria-haspopup="dialog"]', "r2"));
  await click(find('[role="dialog"] [role="link"]', "r2"));
  assert.equal(opens[0][0].id, "r2");
});

test("an ended run is dismissed for every window, and Keep going answers the flag it shows", async () => {
  const requests = backend({ monitors: { r1: flagged } });
  const workspace = createWorkspace("test");
  const shown = workspace.queries.listChatShownRunsQuery();
  workspace.queryClient.setQueryData(shown.queryKey, { source: "shown", runs: [item(run("r2", { status: "done", endedAt: now }))] });
  await banner([run("r2", { status: "done", endedAt: now })], { workspace });
  await settle();
  await click(button(m.run_dismiss()));
  assert.equal(requests.filter((r) => r.request === "POST /api/chat/sessions/s/shown-runs/r2/dismiss").length, 1);
  assert.deepEqual(workspace.queryClient.getQueryData(shown.queryKey).runs, []);

  const waiting = run("r1", { monitor: summary({ tone: "flag", flag: { id: "f1", state: "waiting" } }) });
  await banner([waiting]);
  await settle();
  await click(find("button", m.run_flag_keep_going()));
  assert.deepEqual(requests.find((r) => r.request === "POST /api/runs/r1/flag").body.choice, "keep");
  assert.equal(requests.find((r) => r.request === "POST /api/runs/r1/flag").body.flagId, "f1");
});

test("a flag's Stop run asks first and keeps the dialog open when the server refuses", async () => {
  let refusal = "The run's machine is offline.";
  const requests = backend({ monitors: { r1: flagged }, flagRefusal: () => refusal });
  await banner([run("r1", { monitor: summary({ tone: "flag", flag: { id: "f1", state: "waiting" } }) })]);
  await settle();
  const answers = () => requests.filter((r) => r.request === "POST /api/runs/r1/flag").map((r) => r.body.choice);
  const strip = () => [...document.querySelectorAll("button")].find((element) => element.textContent.includes(m.experiments_stop_run()) && !element.closest('[role="dialog"]'));

  await click(strip());
  assert.ok(document.querySelector('[role="dialog"]').textContent.includes(m.run_stop_effect()));
  await press("Escape");
  await settle();
  assert.deepEqual(answers(), []);

  await click(strip());
  await click(find('[role="dialog"] button', m.experiments_stop_run()));
  assert.ok(document.querySelector('[role="dialog"]').textContent.includes(refusal));

  refusal = null;
  await click(find('[role="dialog"] button', m.experiments_stop_run()));
  assert.deepEqual(answers(), ["stop", "stop"]);
  assert.equal(document.querySelector('[role="dialog"]'), null);
});

test("the reporting error appears once with the run strip open", async () => {
  backend({ uiState: { onboardingCompleted: true, runBanner: "line" } });
  const error = "The run's log directory cannot be read.";
  const { container } = await banner([run("r1", { backend: { kind: "ssh_job", monitoringError: error } })]);
  await settle();
  const occurrences = () => plain(container.querySelector('[role="group"]').textContent).split(error).length - 1;
  assert.equal(occurrences(), 1);
  await click(button(m.run_show_more()));
  assert.equal(occurrences(), 1);
});

test("an SSH contact problem shows its message and retry time once, with Retry and Abandon in the header", async () => {
  const since = now - 60_000;
  const nextRetryMs = now + 60_000;
  const host = { alias: "lab-a40", status: "backoff", class: "key", sinceMs: since, nextRetryMs };
  backend({
    uiState: { onboardingCompleted: true, runBanner: "line" },
    hosts: [host],
  });
  const away = run("r1", { host: "lab-a40", backend: { kind: "ssh_job" }, contact: { type: "unreachable", since, cause: "timed out" } });
  const { container, workspace } = await banner([away]);
  await settle();
  const said = () => plain(container.querySelector('[role="group"]').textContent);
  const lead = plain(m.run_notice_unreachable({ host: "lab-a40", time: fmtClock(since) }));
  const retry = plain(m.run_notice_retry_at({ time: fmtClock(nextRetryMs) }));
  for (const message of [lead, retry]) assert.equal(said().split(message).length - 1, 1);
  assert.ok(find("button", m.app_retry()));
  await click(button(m.run_show_more()));
  for (const message of [lead, retry]) assert.equal(said().split(message).length - 1, 1);
  assert.ok(!said().includes("timed out"), "the header already says the host is out of reach");
  await click(document.querySelector(`[aria-label="${m.run_more_actions()}"]`));
  await settle();
  const abandon = [...document.querySelectorAll('[role="menuitem"]')].find((item) => item.textContent === m.run_abandon());
  assert.ok(abandon, "Abandon waits in Retry's menu");

  const later = nextRetryMs + 60_000;
  await act(async () => workspace.queries.setEngineHost({ ...host, nextRetryMs: later }));
  await settle();
  const rescheduled = plain(m.run_notice_retry_at({ time: fmtClock(later) }));
  assert.equal(said().split(rescheduled).length - 1, 1);
  assert.equal(plain(status()).split(rescheduled).length - 1, 1);
});

test("a Retry that fails from the closed strip says why under its header", async () => {
  const since = now - 60_000;
  const host = { alias: "lab-a40", status: "backoff", class: "key", sinceMs: since, nextRetryMs: now + 60_000 };
  backend({ uiState: { onboardingCompleted: true, runBanner: "line" }, hosts: [host], retryRefusal: "lab-a40's host key changed." });
  const away = (at) => run("r1", { host: "lab-a40", backend: { kind: "ssh_job" }, contact: { type: "unreachable", since: at, cause: "timed out" } });
  const view = await banner([away(since)]);
  await settle();
  assert.ok(button(m.run_show_more()), "the strip stays closed");
  await click(find("button", m.app_retry()));
  assert.ok(document.querySelector('[role="alert"]')?.textContent.includes("lab-a40's host key changed."));

  await view.update([away(now)]);
  await settle();
  assert.ok(!document.querySelector('[role="alert"]'), "a later outage starts without the old refusal");
});

test("a host that needs authentication offers one Connect action with the strip closed or open", async () => {
  backend({ uiState: { onboardingCompleted: true, runBanner: "line" } });
  const workspace = createWorkspace("test");
  const since = now - 60_000;
  const auth = run("r1", { host: "lab-a40", backend: { kind: "slurm_job" }, contact: { type: "needs_auth", since, cause: "Duo approval required" } });
  const { container } = await banner([auth], { workspace });
  await settle();
  const line = container.querySelector('[role="group"]').textContent.replace(/[⁦-⁩]/g, "");
  assert.ok(line.includes(m.run_notice_needs_auth({ host: "lab-a40" })), line);
  assert.ok(container.querySelector('[role="progressbar"] .bg-subtext'), "the bar freezes in grey");
  const asked = [];
  workspace.events.onHostConnectRequest(() => asked.push(workspace.events.pendingHostConnect()));
  const connect = () => [...container.querySelectorAll("button")].filter((element) => element.textContent === m.settings_connect());
  assert.equal(connect().length, 1);
  await click(button(m.run_show_more()));
  assert.equal(connect().length, 1);
  assert.ok(!container.textContent.includes(auth.contact.cause), "Connect already says the host needs signing in to");
  await click(connect()[0]);
  assert.deepEqual(asked, ["lab-a40"]);
});

test("a flag's detail pluralizes the count from its own look", async () => {
  const counted = { ...flagged, counts: { "ERR 429": 43 }, flags: [{ ...flagged.flags[0], bindings: { 'count:"ERR 429"': 1 }, detail: '**{count:"ERR 429"}** lines like "ERR 429" so far.' }] };
  backend({ monitors: { r1: counted } });
  await banner([run("r1", { monitor: summary({ tone: "flag", flag: { id: "f1", state: "waiting" } }) })]);
  await settle();
  assert.ok([...document.querySelectorAll("[role=group] span")].some((p) => p.textContent.replace(/[⁦-⁩]/g, "").startsWith('1 line like "ERR 429" so far.')));
});

for (const [source, template, warning] of [
  ["look", "Loss is **{loss}**.", "Loss is NaN."],
  ["tripwire", "A new kind of error in the log: **RuntimeError**.", "A new kind of error in the log: RuntimeError."],
  ["rule", "Rule **memory** asks: Memory use is rising.", "Rule memory asks: Memory use is rising."],
]) {
  test(`a ${source} flag says its warning once when template and detail match`, async () => {
    const flag = { ...flagged.flags[0], source, template, detail: template };
    backend({ monitors: { r1: { ...flagged, flags: [flag] } } });
    const { container } = await banner([waitingRun()]);
    await settle();
    assert.equal(plain(container.querySelector("[role=group]").textContent).split(warning).length - 1, 1);
  });
}

test("the expanded flag renders the full warning in one place", async () => {
  const flag = { ...flagged.flags[0], detail: "Loss is NaN. Check the log." };
  backend({ monitors: { r1: { ...flagged, flags: [flag] } } });
  const { container } = await banner([waitingRun()]);
  await settle();
  const strip = plain(container.querySelector("[role=group]").textContent);
  assert.match(strip, /Loss is NaN\. Check the log\./);
  assert.equal(strip.split("Loss is NaN.").length - 1, 1);
});

const plain = (text) => text.replace(/[⁦-⁩]/g, "");
const named = (label) => [...document.querySelectorAll("button")].find((element) => plain(element.textContent) === plain(label)) ?? null;
const waitingRun = () => run("r1", { monitor: summary({ tone: "flag", flag: { id: "f1", state: "waiting" } }) });

test("a follower's flag does not name the current chat's agent as the Ask recipient", async () => {
  backend({ monitors: { r1: flagged } });
  await banner([{ ...waitingRun(), chatSessionId: "another-chat" }]);
  await settle();
  assert.ok(named(m.run_flag_ask_unnamed()));
  assert.equal(named(m.run_flag_ask({ agent: "Claude" })), null);
});

test("a flag shows when its review is queued, waking the agent or being read, in the strip and line", async () => {
  const states = [
    ["raised", m.chat_queued(), m.chat_queued()],
    ["asking", m.chat_queued(), m.chat_queued()],
    ["waking", m.run_flag_waking({ agent: "Claude" }), m.chat_starting()],
    ["agent_looking", m.run_flag_reading({ agent: "Claude" }), m.run_agent_looking({ agent: "Claude" })],
  ];
  for (const [state, stripStatus, lineStatus] of states) {
    const monitor = { ...flagged, flags: [{ ...flagged.flags[0], state }] };
    backend({ monitors: { r1: monitor } });
    const view = await banner([run("r1", { monitor: summary({ tone: "flag", flag: { id: "f1", state } }) })]);
    await settle();
    assert.ok(plain(view.container.textContent).includes(stripStatus), state);
    if (state !== "agent_looking") assert.ok(!plain(view.container.textContent).includes(m.run_flag_reading({ agent: "Claude" })), state);
    await click(button(m.run_show_less()));
    assert.ok(plain(view.container.textContent).includes(lineStatus), state);
    if (state !== "agent_looking") assert.ok(!plain(view.container.textContent).includes(m.run_agent_looking({ agent: "Claude" })), state);
    await view.unmount();
    window.sessionStorage.clear();
  }
});

/** Today's server with run r1 flagged and waiting; each flag answer waits until the test gives it. */
function heldFlag(monitor = () => flagged) {
  const choices = [];
  const answers = [];
  serve((path, init) => {
    const request = `${init.method ?? "GET"} ${path}`;
    if (request === "GET /api/settings/ui-state") return { onboardingCompleted: true };
    if (request === "GET /api/compute/hosts") return [];
    if (request === "GET /api/runs/r1/monitor") return monitor();
    if (request === "POST /api/runs/r1/flag") {
      choices.push(JSON.parse(init.body));
      return new Promise((resolve) => answers.push(resolve));
    }
  });
  const answer = (body, status = 200) => act(async () => answers.shift()(Response.json(body, { status })));
  const lose = () => act(async () => answers.shift()(Promise.reject(new TypeError("Failed to fetch"))));
  return { choices, answer, lose };
}

const askReply = (flag, rev) => ({ flag, run: run("r1", { monitor: summary({ tone: "flag", rev, flag }) }) });

test("Ask retries reuse the request id and a new Ask after handback gets a new id", async () => {
  const { choices, answer } = heldFlag();
  const workspace = createWorkspace("test");
  await banner([waitingRun()], { workspace });
  await settle();
  const ask = named(m.run_flag_ask({ agent: "Claude" }));
  await act(async () => { ask.click(); ask.click(); });
  await act(async () => named(m.experiments_stop_run()).click());
  assert.equal(choices.length, 1, "one choice in flight, whatever is pressed");
  await answer({ error: "The server is restarting." }, 503);
  await settle();
  await click(ask);
  assert.equal(choices.length, 2);
  assert.equal(choices[1].requestId, choices[0].requestId);
  assert.equal(choices[1].choice, "ask");
  const queued = { ...flagged.flags[0], state: "asking" };
  await answer(askReply(queued, flagged.rev + 1));
  await settle();
  assert.equal(named(m.run_flag_ask({ agent: "Claude" })), null);
  const { queryKey } = workspace.queries.getRunMonitorQuery("r1");
  await act(async () => workspace.queryClient.setQueryData(queryKey, { ...flagged, rev: flagged.rev + 2 }));
  await settle();
  await click(named(m.run_flag_ask({ agent: "Claude" })));
  assert.equal(choices.length, 3);
  assert.notEqual(choices[2].requestId, choices[1].requestId, "a new Ask after the handback wakes the agent again");
  await answer(askReply(queued, flagged.rev + 3));
  await settle();
});

for (const by of ["agent", "user"]) test(`a lost Ask reply gets a new request id when the ${by}'s Ask returns to waiting`, async () => {
  const { choices, answer, lose } = heldFlag();
  const workspace = createWorkspace("test");
  const { queryKey } = workspace.queries.getRunMonitorQuery("r1");
  const view = await banner([waitingRun()], { workspace });
  await settle();
  await click(named(m.run_flag_ask({ agent: "Claude" })));

  const handedBack = { ...flagged.flags[0], handback: 11, choice: { by, c: "ask", at: now } };
  await view.update([run("r1", { monitor: summary({ tone: "flag", flag: handedBack, rev: flagged.rev + 2 }) })]);
  await lose();
  await settle();
  assert.equal(workspace.queryClient.getQueryData(queryKey).rev, flagged.rev, "the full monitor still lags");
  await click(named(m.run_flag_ask({ agent: "Claude" })));
  await answer(askReply({ ...handedBack, state: "asking" }, flagged.rev + 3));
  assert.equal(choices.length, 2);
  assert.notEqual(choices[0].requestId, choices[1].requestId);
});

test("a late Ask reply keeps the newer waking status", async () => {
  let monitor = flagged;
  const { answer } = heldFlag(() => monitor);
  const workspace = createWorkspace("test");
  const { container } = await banner([waitingRun()], { workspace });
  await render(createElement(QueryEvents), workspace);
  await settle();
  await click(named(m.run_flag_ask({ agent: "Claude" })));

  const queued = { ...flagged.flags[0], state: "asking", choice: { by: "user", c: "ask", at: now } };
  const waking = { ...queued, state: "waking" };
  const wakingSummary = summary({ tone: "flag", generation: "g2", rev: flagged.rev + 2, flag: waking });
  monitor = { ...flagged, generation: wakingSummary.generation, rev: wakingSummary.rev, asOf: wakingSummary.asOf, flags: [waking] };
  await act(async () => {
    streams.at(-1).dispatchEvent(new MessageEvent("run.updated", { data: JSON.stringify({ run: run("r1", { monitor: wakingSummary }) }) }));
  });
  const { queryKey } = workspace.queries.getRunMonitorQuery("r1");
  await act(async () => workspace.queryClient.refetchQueries({ queryKey, exact: true }));
  await settle();
  const status = () => container.querySelector('[data-run-frame] [role="status"]')?.textContent ?? "";
  const beforeReply = status();

  await answer(askReply(queued, flagged.rev + 1));
  await settle();
  assert.match(beforeReply, /Waking/);
  assert.match(status(), /Waking/);
  const shown = workspace.queries.listChatShownRunsQuery();
  assert.equal(workspace.queryClient.getQueryData(shown.queryKey).runs[0].run.monitor.rev, wakingSummary.rev);
});

test("a refused Keep going stays where it was, says why and keeps focus", async () => {
  const { choices, answer } = heldFlag();
  await banner([waitingRun()]);
  await settle();
  const keep = named(m.run_flag_keep_going());
  keep.focus();
  await click(keep);
  assert.equal(choices[0].choice, "keep");
  assert.ok(named(m.run_flag_keep_going()), "the choice waits for the server's answer in place");
  await answer({ error: "This flag was already answered." }, 409);
  await settle();
  assert.ok(find('[role="alert"]', "This flag was already answered."));
  assert.equal(document.activeElement, named(m.run_flag_keep_going()));
});

test("a choice's answer settles only its own flag: a newer flag keeps its choices", async () => {
  const { answer } = heldFlag();
  const workspace = createWorkspace("test");
  const { queryKey } = workspace.queries.getRunMonitorQuery("r1");
  await banner([waitingRun()], { workspace });
  await settle();
  await click(named(m.run_flag_keep_going()));
  await act(async () => workspace.queryClient.setQueryData(queryKey, { ...flagged, rev: flagged.rev + 2, flagId: "f2", flags: [{ ...flagged.flags[0], id: "f2" }] }));
  await answer({ flag: { ...flagged.flags[0], state: "kept" }, run: waitingRun() });
  await settle();
  assert.equal(workspace.queryClient.getQueryData(queryKey).flagId, "f2");
  assert.ok(named(m.run_flag_keep_going()), "the newer flag still asks");
});

test("a choice on its way is never forgotten, however long its answer takes, and is let go once answered", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const { choices, answer } = heldFlag();
  // Its own workspace: a second request would queue behind the first, and must not hold up later tests.
  const workspace = createWorkspace("pending");
  const { queryKey } = workspace.queries.flagChoiceQuery("r1", "f1");
  const keep = () => act(async () => {
    void chooseFlag(workspace, { id: "r1" }, "f1", "keep", () => {});
  });
  await keep();
  await act(async () => t.mock.timers.tick(2 * 3_600_000));
  await keep();
  await answer({ flag: { ...flagged.flags[0], state: "kept" }, run: waitingRun() });
  await settle();
  assert.equal(choices.length, 1, "nothing on screen, and still one request");
  await act(async () => t.mock.timers.tick(2 * 3_600_000));
  assert.equal(workspace.queryClient.getQueryData(queryKey), undefined);
});

test("a run its newest source says is waiting for a choice opens its strip, whatever its older summary says", async () => {
  backend({ uiState: { onboardingCompleted: true, runBanner: "line" }, monitors: { r1: flagged } });
  const workspace = createWorkspace("test");
  workspace.queryClient.setQueryData(workspace.queries.getRunMonitorQuery("r1").queryKey, flagged);
  await banner([run("r1", { monitor: summary({ rev: flagged.rev - 1, flag: null }) })], { workspace });
  await settle();
  assert.ok(button(m.run_show_less()), "the strip opened by itself");
  assert.ok(named(m.run_flag_keep_going()));
});

test("a monitor that can't be refreshed keeps its chart and says so once, how old it is, and offers to try again", async () => {
  let failing = false;
  serve((path) => {
    if (path === "/api/settings/ui-state") return { onboardingCompleted: true };
    if (path === "/api/compute/hosts") return [];
    if (path === "/api/runs/r1/monitor") return failing ? Response.json({ error: "The server stopped." }, { status: 500 }) : running;
  });
  const workspace = createWorkspace("test");
  const { container } = await banner([run("r1")], { workspace });
  await settle();
  failing = true;
  await act(async () => workspace.queryClient.invalidateQueries({ queryKey: workspace.queries.getRunMonitorQuery("r1").queryKey }));
  await settle();
  const said = plain(container.textContent).replace(plain(status()), "");
  assert.equal(said.split(m.run_view_refresh_failed()).length - 1, 1, said);
  assert.ok(!said.includes(m.run_monitor_load_failed()), said);
  assert.ok(find("div", m.run_view_refresh_failed()).contains(named(m.app_retry())), "Retry sits beside the one message");
  assert.ok(container.querySelector('[role="img"][aria-label*="Loss"]'), "the chart stays");
  assert.ok(plain(container.textContent).includes(m.status_last_seen({ time: plain(fmtClock(running.asOf)) })));
  assert.ok(container.querySelector('[role="progressbar"]'), "the bar stays below the chart");
});

test("a finished run's strip shows its result during quiet final collection", async () => {
  const availability = { state: "stale", reason: null };
  backend({ monitors: { r1: { ...running, availability } } });
  const { container } = await banner([run("r1", { status: "done", endedAt: now, monitor: summary({ tone: "done", availability }) })]);
  await settle();
  assert.ok(button(m.run_show_less()), "the strip is open");
  assert.ok(container.textContent.includes("2.31"));
  assert.ok(!container.textContent.includes(m.run_monitor_load_failed()));
  assert.ok(!named(m.app_retry()));
});

test("a closed line uses its summary until opening reads the monitor, and Retry restores the countdown", async () => {
  const monitors = { r1: Response.json({ error: "The monitor is down." }, { status: 500 }) };
  const requests = backend({ uiState: { onboardingCompleted: true, runBanner: "line" }, monitors });
  const estimate = { ...running.estimate, at: now };
  const { container } = await banner([run("r1", { monitor: summary({ estimate }) })]);
  await settle();
  const line = container.querySelector('[role="group"]');
  assert.match(plain(line.textContent), /\d+[mh].*left/);
  assert.ok(!requests.some(({ request }) => request.endsWith("/monitor")));
  await click(button(m.run_show_more()));
  assert.ok(plain(line.textContent).includes(m.run_monitor_load_failed()));
  assert.doesNotMatch(plain(line.textContent), /\d+[mh].*left/);
  assert.equal(plain(status()), `${m.run_monitor_load_failed()} ${plain(m.status_last_seen({ time: fmtClock(now) }))}.`);

  monitors.r1 = { ...running, estimate };
  await click(named(m.app_retry()));
  assert.match(plain(line.textContent), /\d+[mh].*left/);
});

test("a run whose view was rebuilt reads its monitor again, and shows none of the old one meanwhile", async () => {
  const requests = backend();
  const view = await banner([run("r1")]);
  await settle();
  await view.update([run("r1", { monitor: summary({ generation: "g2", rev: running.rev + 1, asOf: running.asOf + 1000, bindings: { loss: 1.5 }, headline: { name: "loss", label: "Loss", value: 1.5 } }) })]);
  await settle();
  assert.equal(requests.filter((r) => r.request === "GET /api/runs/r1/monitor").length, 2);
  const text = plain(view.container.textContent);
  assert.ok(text.includes("Loss") && !text.includes("2.31"), text);
  assert.equal(plain(view.container.querySelector(".text-xl").textContent), "1.50");
  assert.equal(view.container.querySelector('[role="img"][aria-label*="Loss"]'), null, "the chart waits for the rebuilt monitor");
});

test("a chat whose runs can't be listed says so and lists them once it can, never falling back on an error", async () => {
  const requests = [];
  let failing = true;
  serve((path, init) => {
    requests.push(path);
    if (path === "/api/settings/ui-state") return { onboardingCompleted: true };
    if (path === "/api/compute/hosts") return [];
    if (path === "/api/runs/r1/monitor") return running;
    if (path === "/api/chat/shown-runs?includeFailures=true") return failing ? Response.json({ error: "The server stopped." }, { status: 500 }) : [item(run("r1"))];
    if (init.method === "POST") return {};
  });
  const { container } = await render(createElement(RunBanner, { sessionId: "s", agent: "Claude" }));
  await settle();
  assert.ok(plain(container.textContent).includes(m.run_banner_load_failed()));
  assert.ok(!requests.includes("/api/chat/active-runs"), "only an older server's 404 falls back");
  failing = false;
  await click(named(m.app_retry()));
  assert.ok(!plain(container.textContent).includes(m.run_banner_load_failed()));
  assert.ok(button(m.run_show_less()), "the run shows");
});

test("an unreadable row shows Retry in its own chat and keeps healthy rows visible", async () => {
  let failedSessions = ["s"];
  serve((path) => {
    if (path === "/api/settings/ui-state") return { onboardingCompleted: true };
    if (path === "/api/compute/hosts") return [];
    if (path === "/api/runs/r1/monitor") return running;
    if (path === "/api/chat/shown-runs?includeFailures=true") return { runs: [item(run("r1"))], failedSessions };
  });
  const { container } = await render([
    createElement(RunBanner, { key: "other", sessionId: "other", agent: "Claude" }),
    createElement(RunBanner, { key: "s", sessionId: "s", agent: "Claude" }),
  ]);
  await settle();
  assert.equal(container.querySelectorAll("section").length, 1, "only the affected chat shows anything");
  assert.ok(find('[role="status"]', m.run_banner_load_failed()));
  assert.ok(button(m.run_show_less()), "the readable run still shows");
  failedSessions = [];
  await click(named(m.app_retry()));
  assert.ok(!find('[role="status"]', m.run_banner_load_failed()));
  assert.ok(button(m.run_show_less()));
});

test("the evidence's log lines ask to open the run's log at the cited bytes of the cited file", async () => {
  backend({ monitors: { r1: flagged } });
  const opened = [];
  await banner([waitingRun()], { props: { onOpenRun: (...args) => opened.push(args) } });
  await settle();
  await click(find("button", "91"));
  const { byteFrom, ident } = flagged.flags[0].evidence;
  assert.deepEqual(opened, [["r1", "preview", { at: byteFrom, ident }]]);
});

test("a choice belongs to its flag: collapsing and reopening the strip neither sends it again nor forgets why it was refused", async () => {
  const { choices, answer } = heldFlag();
  const reopen = async () => {
    await click(button(m.run_show_less()));
    await click(button(m.run_show_more()));
  };
  await banner([waitingRun()]);
  await settle();
  await click(named(m.run_flag_keep_going()));
  await reopen();
  await act(async () => { named(m.run_flag_keep_going()).click(); named(m.experiments_stop_run()).click(); });
  await answer({ flag: flagged.flags[0], run: waitingRun() });
  await settle();
  assert.equal(choices.length, 1, "presses while the answer was on its way sent nothing, before or after it came back");

  await click(named(m.run_flag_keep_going()));
  assert.notEqual(choices[1].requestId, choices[0].requestId, "the acknowledged choice gets a new request");
  await answer({ error: "This flag was already answered." }, 409);
  await settle();
  await reopen();
  assert.ok(find('[role="alert"]', "This flag was already answered."), "the refusal outlives the strip that pressed it");
  await click(named(m.run_flag_keep_going()));
  assert.equal(choices[2].requestId, choices[1].requestId, "trying again is the same request");
  await answer({ flag: { ...flagged.flags[0], state: "kept" }, run: waitingRun() });
  await settle();
});

test("summaries faster than the catch-up keep the chart and read the monitor again once, two seconds after it fell behind", async (t) => {
  const requests = backend();
  const view = await banner([run("r1", { monitor: summary({ rev: running.rev }) })]);
  await settle();
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const reads = () => requests.filter((r) => r.request === "GET /api/runs/r1/monitor").length;
  for (let step = 1; step <= 4; step++) {
    await view.update([run("r1", { monitor: summary({ rev: running.rev + step, bindings: { loss: 2.3 - step / 10 }, headline: { name: "loss", label: "Loss", value: 2.3 - step / 10 } }) })]);
    await act(async () => t.mock.timers.tick(700));
    await settle();
    assert.ok(view.container.querySelector('[role="img"][aria-label*="Loss"]'), `the chart keeps drawing at step ${step}`);
  }
  assert.equal(reads(), 2, "one reread, at two seconds, however many summaries arrived");
  assert.equal(plain(view.container.querySelector(".text-xl").textContent), "1.90", "the headline is the newest summary's");
});

test("a degraded line names its state and reason on hover, focus and to a screen reader, and announces the change once", async () => {
  backend({ uiState: { onboardingCompleted: true, runBanner: "line" } });
  const initial = summary({ asOf: running.asOf });
  const stale = { ...initial, availability: { state: "rebuilding", reason: "The run's log moved." } };
  const { container, update } = await banner([run("r1", { monitor: initial })]);
  await settle();
  await update([run("r1", { monitor: stale })]);
  await settle();
  const why = `${m.run_view_rebuilding()} · The run's log moved.`;
  const age = m.status_last_seen({ time: plain(fmtClock(running.asOf)) });
  const toggle = container.querySelector('[aria-describedby]');
  assert.equal(plain(document.getElementById(toggle.getAttribute("aria-describedby")).textContent), why, "the line's description says why");
  const tip = [...container.querySelectorAll('[role="img"]')].find((element) => element.textContent.includes(plain(age).slice(0, 9)));
  assert.equal(plain(tip.getAttribute("aria-label")), `${why} · ${age}`);
  assert.equal(plain(tip.querySelector('[role="tooltip"]').textContent), `${why} · ${age}`, "hover or focus shows the reason");
  assert.equal(plain(status()), `${why} ${age}.`);
  assert.ok(!plain(container.textContent).includes(m.run_eta_left({ duration: "" }).trim()), "no time left");

  await update([run("r1", { monitor: { ...stale, rev: stale.rev + 1 } })]);
  await settle();
  assert.equal(plain(status()), `${why} ${age}.`, "said once");
});

test("a flag handback announces that the run needs attention even when the warning stays the same", async () => {
  const looking = { ...flagged, flags: [{ ...flagged.flags[0], state: "agent_looking" }] };
  backend({ monitors: { r1: looking } });
  const workspace = createWorkspace("test");
  const { queryKey } = workspace.queries.getRunMonitorQuery("r1");
  workspace.setScopedQueryData(queryKey, looking);
  await banner([waitingRun()], { workspace });
  await settle();
  assert.equal(status(), "");

  await act(async () => workspace.setScopedQueryData(queryKey, { ...flagged, rev: flagged.rev + 1 }));
  await settle();
  assert.ok(named(m.run_flag_keep_going()));
  assert.ok(plain(status()).includes(m.chat_run_needs_attention()), status());
});

test("a state change and a freshness change in one update both reach the live region", async () => {
  backend();
  const view = await banner([run("r1")]);
  await settle();

  const reason = "The run's log could not be read.";
  const monitor = summary({ tone: "failed", sentence: null, rev: running.rev + 1, unavailable: reason });
  await view.update([run("r1", { status: "failed", endedAt: now, monitor })]);
  await settle();
  assert.ok(plain(status()).includes(m.status_failed()), status());
  assert.ok(plain(status()).includes(reason), status());
  assert.ok(plain(status()).includes(plain(m.status_last_seen({ time: fmtClock(now) }))), status());
});

test("shown runs without a summary draw their terminal status at both levels without fetching a monitor", async () => {
  for (const status of ["done", "failed", "cancelled"]) {
    const requests = backend();
    const opens = [];
    const view = await banner([run("r1", { status, endedAt: now, monitor: undefined })], {
      props: { onOpenRunPage: (...args) => opens.push(args) },
    });
    await settle();
    assert.ok(button(m.run_dismiss()));
    const label = status === "done" ? m.status_done() : status === "failed" ? m.status_failed() : m.run_lead_stopped();
    assert.ok(view.container.textContent.includes(label), view.container.textContent);
    await click(view.container.querySelector('button[aria-haspopup="dialog"]'));
    await click(document.querySelector('[role="dialog"] [role="link"]'));
    assert.equal(opens[0][0].id, "r1");
    assert.equal(button(m.run_show_less()), null);
    assert.ok(view.container.textContent.includes(label));
    assert.ok(button(m.run_dismiss()));
    await click(view.container.querySelector('button[aria-haspopup="dialog"]'));
    await click(document.querySelector('[role="dialog"] [role="link"]'));
    assert.equal(opens[1][0].id, "r1");
    assert.equal(requests.filter((r) => r.request.includes("/monitor")).length, 0);
    assert.ok(!view.container.textContent.includes(m.run_monitor_load_failed()));
    await view.unmount();
  }
});

test("a shown active run without a summary ignores cached monitor data and draws from its status", async () => {
  const requests = backend();
  const workspace = createWorkspace("test");
  workspace.queryClient.setQueryData(workspace.queries.getRunMonitorQuery("r1").queryKey, flagged);
  const { container } = await banner([run("r1", { monitor: null })], { workspace });
  await settle();
  assert.ok(button(m.run_stop()));
  assert.ok(container.textContent.includes(m.status_running()));
  assert.equal(container.querySelector('[role="img"][aria-label*="Loss"]'), null);
  assert.equal(requests.filter((r) => r.request.includes("/monitor")).length, 0);
});

test("an unresolved flag stays visible after completion and announces that the run ended", async () => {
  const ended = { ...flagged, flags: [{ ...flagged.flags[0], state: "waiting" }] };
  const requests = backend({ monitors: { r1: ended } });
  const done = run("r1", { status: "done", endedAt: now, monitor: summary({ tone: "flag", sentence: flagged.sentence, flag: { id: "f1", state: "waiting" }, rev: flagged.rev }) });
  const view = await banner([{ ...done, status: "running", endedAt: null }]);
  await settle();
  await view.update([done]);
  await settle();
  const { container } = view;
  assert.ok(plain(status()).includes("Loss is NaN"));
  assert.ok(plain(status()).includes(m.run_done_in({ duration: "1m" })));
  const text = container.textContent.replace(/[⁦-⁩]/g, "");
  assert.ok(text.includes(m.run_flag_ended_choose({ agent: "Claude" })), text);
  assert.ok(![...container.querySelectorAll("button")].some((b) => b.textContent.includes(m.experiments_stop_run())), "an ended run has nothing to stop");
  assert.ok(text.includes(m.run_flag_ask({ agent: "Claude" })), text);
  await click(find("button", m.run_flag_keep_result()));
  assert.equal(requests.find((r) => r.request === "POST /api/runs/r1/flag").body.choice, "keep");
});

test("a chart needs two points and a finished card keeps its headline", async () => {
  const setup = { ...running, sentence: null, setup: { stage: "Download", line: null, startsAt: now + 60_000 }, series: {}, latest: {}, headline: null,
    phases: [{ name: "Download", of: 9, unit: "shards", priorMs: null, startedAt: now, endedAt: null }],
    progress: { ...running.progress, phase: 0, done: 4, whole: null, segments: [{ weight: 1, fill: 4 / 9 }] } };
  backend({ monitors: { r1: setup } });
  const workspace = createWorkspace("test");
  const view = await banner([run("r1")], { workspace });
  await settle();
  assert.equal(view.container.querySelector('svg[role="img"]'), null);
  assert.equal(button(m.run_show_more()), null);
  assert.equal(view.container.querySelector('[role="progressbar"]').getAttribute("aria-valuenow"), null);
  assert.ok(view.container.querySelector('[role="progressbar"] i.bg-subtext'));
  assert.ok(plain(view.container.textContent).includes("Run starts in about"));

  const { queryKey } = workspace.queries.getRunMonitorQuery("r1");
  const chart = { ...running, sentence: null, setup: null, rev: running.rev + 1, series: { loss: [[0, 3.4]] } };
  await act(async () => workspace.queryClient.setQueryData(queryKey, chart));
  await settle();
  assert.equal(view.container.querySelector('svg[role="img"]'), null);
  await act(async () => workspace.queryClient.setQueryData(queryKey, { ...chart, rev: chart.rev + 1, series: { loss: [[0, 3.4], [20, 3.3]] } }));
  await settle();
  assert.ok(view.container.querySelector('svg[role="img"]'));
  assert.ok(view.container.querySelector('[role="progressbar"]'));

  await view.update([run("r1", { status: "done", endedAt: now, monitor: summary({ tone: "done" }) })]);
  await settle();
  assert.equal(view.container.querySelector('svg[role="img"]'), null);
  assert.equal(view.container.querySelector('[role="progressbar"]'), null);
  assert.ok(plain(view.container.textContent).includes("Done in"));
  assert.ok(button(m.run_show_less()));
  assert.ok(plain(view.container.querySelector(".text-xl").textContent).includes("2.3"));
});


test("a single run opens its stage card from the raw status and links the run from its name", async () => {
  backend({ monitors: { r1: { ...running, sentence: null } } });
  const opens = [];
  const { container } = await banner([run("r1")], { props: { onOpenRunPage: (...args) => opens.push(args) } });
  await settle();
  const trigger = container.querySelector('button[aria-haspopup="dialog"]');
  assert.ok(trigger);
  assert.ok(!trigger.textContent.includes("r1"));
  assert.ok(plain(trigger.getAttribute("aria-label")).includes("r1"));
  assert.equal(trigger.previousElementSibling.getAttribute("tabindex"), null);
  await click(trigger);
  const card = document.querySelector('[role="dialog"]');
  assert.ok(plain(card.textContent).includes(m.compute_target_local()));
  await click(card.querySelector('[role="link"]'));
  await click(trigger);
  await press("Enter", document.querySelector('[role="dialog"] [role="link"]'));
  assert.deepEqual(opens.map(([run, intent]) => [run.id, intent]), [["r1", "preview"], ["r1", "keepOpen"]]);
});

test("a terminal lead keeps a visible separator before its sentence", async () => {
  backend();
  const { container } = await banner([run("r1", { status: "cancelled", endedAt: now, outcome: { by: "user" } })]);
  await settle();
  const line = container.querySelector('button[aria-haspopup="dialog"]');
  assert.ok(line.textContent.includes(`${m.run_lead_stopped()}\u00a0`), line.textContent);
});

test("a lost run names its outcome and draws the last count in its card", async () => {
  backend({ monitors: { r1: { ...running, sentence: null } } });
  const { container } = await banner([run("r1", { status: "lost", endedAt: now })]);
  await settle();
  const line = container.querySelector('button[aria-haspopup="dialog"]');
  assert.ok(plain(line.textContent).startsWith(m.status_lost()), line.textContent);
  assert.ok(plain(line.getAttribute("aria-label")).includes(m.status_lost()));
  await click(line);
  const card = document.querySelector('[role="dialog"]');
  assert.ok(plain(card.textContent).includes(m.status_lost()), card.textContent);
  assert.ok(!card.textContent.includes("**"), card.textContent);
  assert.deepEqual([...card.querySelectorAll("strong")].map((figure) => plain(figure.textContent)), ["1,240", "5,000"]);
});

test("cancellation is said once while a failed stop retains its explanation", async () => {
  backend();
  const view = await banner([run("r1", { cancelling: true })]);
  await settle();
  assert.equal(view.container.textContent.split(m.status_cancelling()).length - 1, 1);
  await view.update([run("r1", { cancelling: true, cancelError: "The scheduler refused this stop" })]);
  await settle();
  assert.ok(view.container.textContent.includes("The scheduler refused this stop"));
});

test("a chart can open on its own while its headline is pending", async () => {
  const view = { ...running.view, signals: { ...running.view.signals, val_bpb: { label: "Val bpb", digits: 3 } }, headline: ["val_bpb"] };
  backend({ monitors: { r1: { ...running, view, headline: null } } });
  const { container } = await banner([run("r1", { monitor: summary({ headline: null }) })]);
  await settle();
  assert.ok(container.querySelector('svg[role="img"]'));
  assert.ok(!container.textContent.includes("Val bpb"));
});
