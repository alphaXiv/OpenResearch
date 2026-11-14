import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { act, createElement } from "react";
import { DetailDrawer } from "../src/components/DetailDrawer.tsx";
import { RunBanner } from "../src/components/run/RunBanner.tsx";
import { fmtClock } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { chooseFlag } from "../src/queries/projects.ts";
import { createWorkspace } from "../src/workspace.tsx";
import { find, press, render, serve, settle } from "./render.mjs";

const fixture = (name) => JSON.parse(readFileSync(new URL(`./fixtures/runMonitor/${name}.json`, import.meta.url), "utf8"));
const running = fixture("running");
const flagged = fixture("flagged");
const now = Date.now();
const summary = (extra = {}) => ({
  tone: "running", whole: 0.248, headline: { name: "loss", label: "Loss", value: 2.31, vs: null }, sentence: running.sentence,
  bindings: { loss: 2.31 }, signals: running.view.signals, etaMs: null, flag: null, rev: 1, generation: "g1", asOf: now, ...extra,
});
const run = (id, extra = {}) => ({
  id, experimentId: `e-${id}`, projectId: "p", status: "running", createdAt: now - 60_000, startedAt: now - 60_000, updatedAt: now,
  contact: null, cancelling: false, lostReason: null, host: null, backend: { kind: "local_job" }, monitor: summary(), ...extra,
});
const item = (value) => ({ sessionId: "s", run: value, experimentName: value.id, experimentSlug: `lr-${value.id}`, runNo: 1, runCount: 1 });

/** Serves today's server for one chat's runs; every request is kept. */
function backend({ uiState = { onboardingCompleted: true }, monitors = { r1: running } } = {}) {
  const requests = [];
  serve((path, init) => {
    const request = `${init.method ?? "GET"} ${path}`;
    requests.push({ request, body: init.body ? JSON.parse(init.body) : undefined });
    if (request === "GET /api/settings/ui-state") return uiState;
    if (request === "POST /api/settings/ui-state") return { ...uiState };
    if (request === "GET /api/compute/hosts") return [];
    const monitor = /^GET \/api\/runs\/([^/]+)\/monitor$/.exec(request);
    if (monitor) return monitors[monitor[1]];
    if (/^POST \/api\/runs\/[^/]+\/cancel$/.test(request)) return { ok: true };
    if (/^POST \/api\/runs\/[^/]+\/flag$/.test(request)) return { flag: { ...flagged.flag, state: "kept" }, run: run("r1") };
    if (/^POST \/api\/chat\/sessions\/s\/shown-runs\/[^/]+\/dismiss$/.test(request)) return { dismissed: true };
  });
  return requests;
}

/** Chat s's banner over `runs` as the shown-runs query holds them; `update` replaces them as an event would. */
async function banner(runs, { shown = true, workspace = createWorkspace("test"), props = {} } = {}) {
  const { queryKey } = workspace.queries.listChatShownRunsQuery();
  const hold = (next) => workspace.queryClient.setQueryData(queryKey, { source: shown ? "shown" : "active", runs: next.map(item) });
  hold(runs);
  const view = await render(createElement(RunBanner, { sessionId: "s", agent: "Claude", ...props }), workspace);
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
  await view.update([run("r1", { monitor: summary({ tone: "flag", sentence: flagged.sentence, bindings: { loss: "NaN" }, flag: { id: "f1", state: "waiting", mode: "copilot" }, rev: flagged.rev }) })]);
  await settle();
  assert.ok(button(m.run_show_less()), "the strip opened by itself");
  await act(async () => workspace.queryClient.refetchQueries({ queryKey: workspace.queries.getRunMonitorQuery("r1").queryKey }));
  await settle();
  assert.ok(find("button", m.run_flag_keep_going()));
  assert.ok(find("pre", "::orx::"), "the choices and evidence show once the monitor has them");
});

test("Stop sends exactly one cancel", async () => {
  const requests = backend();
  await banner([run("r1")]);
  await settle();
  const stop = button(m.run_stop());
  await act(async () => { stop.click(); stop.click(); });
  await settle();
  assert.equal(requests.filter((r) => r.request === "POST /api/runs/r1/cancel").length, 1);
});

test("against an older server the line reads from the run's status and never asks for its monitor", async () => {
  const requests = backend();
  const { container } = await banner([run("r1", { monitor: undefined })], { shown: false });
  await settle();
  assert.ok(button(m.run_show_less()));
  assert.ok(container.textContent.includes(m.run_running_on({ where: "⁨" + m.compute_target_local() + "⁩" })), container.textContent);
  assert.equal(requests.filter((r) => r.request.includes("/monitor")).length, 0);
  assert.equal(button(m.run_dismiss()), null);
});

test("a new sentence is announced once; a number refilling is not", async () => {
  backend();
  const view = await banner([run("r1")]);
  await settle();
  assert.equal(status(), "");
  await view.update([run("r1", { monitor: summary({ bindings: { loss: 2.1 } }) })]);
  await settle();
  assert.equal(status(), "");
  const next = { template: "Loss **{loss}**, flattening out.", t: now };
  await view.update([run("r1", { monitor: summary({ sentence: next, bindings: { loss: 2.1 }, rev: running.rev + 1 }) })]);
  await settle();
  assert.equal(status().replace(/[⁦-⁩]/g, ""), "Loss 2.10, flattening out.");
});

test("the status holds an announcement only until it is made, and nothing of a run that left", async (t) => {
  backend({ monitors: {} });
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
async function several(items, { workspace = createWorkspace("test"), onOpenExperiment = () => {} } = {}) {
  workspace.queryClient.setQueryData(workspace.queries.listChatShownRunsQuery().queryKey, { source: "shown", runs: items });
  return render(createElement(RunBanner, { sessionId: "s", agent: "Claude", onOpenExperiment }), workspace);
}
const rows = () => [...document.querySelectorAll('[role="list"] > li > [role="group"]')];

test("several runs show one summary that opens into a row each, named by what sets it apart", async () => {
  const requests = backend({ monitors: { r1: running, r2: running, r3: running } });
  const { container } = await several([sweep("r1", 4), sweep("r2", 6), sweep("r3", 8)]);
  await settle();
  assert.ok(plain(container.textContent).includes("… at 2e15 FLOPs"), "the summary says what they share");
  assert.deepEqual(rows().map((row) => row.getAttribute("aria-label")), ["Depth 4 at 2e15 FLOPs", "Depth 6 at 2e15 FLOPs", "Depth 8 at 2e15 FLOPs"]);
  assert.deepEqual(rows().map((row) => plain(row.querySelector(".experiment-run-link").textContent)), ["Depth 4", "Depth 6", "Depth 8"]);
  assert.equal(plain(rows()[0].querySelector(".experiment-run-link").getAttribute("aria-label")), "Depth 4 at 2e15 FLOPs", "the whole title is its name");
  assert.equal(requests.filter((r) => r.request.endsWith("/monitor")).length, 0, "closed rows read their summaries");

  await click(rows()[1].querySelector(`button[aria-label="${m.run_show_more()}"]`));
  const open = rows()[1];
  assert.ok(open.querySelector(`button[aria-label="${m.run_show_less()}"]`), "the row opened into its strip in place");
  assert.equal(document.activeElement, open.querySelector(`button[aria-label="${m.run_show_less()}"]`));
  assert.equal(plain(open.querySelector(".experiment-run-link").textContent), "Depth 6");

  await press("Escape");
  await settle();
  assert.equal(rows().length, 3);
  assert.equal(document.activeElement, rows()[1].querySelector(`button[aria-label="${m.run_show_more()}"]`));
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
});

test("a summary lists its runs in the order they were launched", async () => {
  backend({ monitors: {} });
  const launched = (id, depth, at) => ({ ...sweep(id, depth), run: run(id, { createdAt: at }) });
  await several([launched("r3", 8, now - 1_000), launched("r1", 4, now - 3_000), launched("r2", 6, now - 2_000)]);
  await settle();
  assert.deepEqual(rows().map((row) => row.getAttribute("aria-label")), ["Depth 4 at 2e15 FLOPs", "Depth 6 at 2e15 FLOPs", "Depth 8 at 2e15 FLOPs"]);
});

test("a summary says who needs attention first, and a run waiting for a choice opens itself from a folded summary", async () => {
  const monitors = { r1: running, r2: running };
  backend({ uiState: { onboardingCompleted: true, runBanner: "line" }, monitors });
  const workspace = createWorkspace("test");
  await several([sweep("r1", 4), sweep("r2", 6)], { workspace });
  await settle();
  assert.equal(rows().length, 0, "the remembered line folds the summary");
  assert.ok(button(m.run_show_each()));

  monitors.r2 = flagged;
  const waiting = run("r2", { monitor: summary({ tone: "flag", sentence: flagged.sentence, bindings: { loss: "NaN" }, flag: { id: "f1", state: "waiting", mode: "copilot" }, rev: flagged.rev }) });
  await act(async () => workspace.queryClient.setQueryData(workspace.queries.listChatShownRunsQuery().queryKey, { source: "shown", runs: [sweep("r1", 4), { ...sweep("r2", 6), run: waiting }] }));
  await settle();
  const head = plain(find("button[aria-expanded]").textContent);
  assert.ok(head.startsWith(plain(m.run_summary_attention({ name: "Depth 6", state: m.status_flagged() }))), head);
  assert.equal(rows().length, 2, "the rows opened");
  assert.ok(rows()[1].querySelector(`button[aria-label="${m.run_show_less()}"]`), "and so did the waiting run's strip");
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
  const { container } = await several([item(run("r1")), item(run("r2"))], { onOpenExperiment: (...args) => opens.push(args) });
  await settle();
  assert.ok(!plain(container.textContent).includes("…"));
  await click(find("button", "r2"));
  assert.equal(opens[0][0], "e-r2");
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

  const waiting = run("r1", { monitor: summary({ tone: "flag", flag: { id: "f1", state: "waiting", mode: "copilot" } }) });
  await banner([waiting]);
  await settle();
  await click(find("button", m.run_flag_keep_going()));
  assert.deepEqual(requests.find((r) => r.request === "POST /api/runs/r1/flag").body.choice, "keep");
  assert.equal(requests.find((r) => r.request === "POST /api/runs/r1/flag").body.flagId, "f1");
});

test("lost contact puts the contact words on the line, Connect beside them, and the notice's actions in the strip", async () => {
  backend({ uiState: { onboardingCompleted: true, runBanner: "line" } });
  const workspace = createWorkspace("test");
  const since = now - 60_000;
  const auth = run("r1", { host: "lab-a40", backend: { kind: "slurm_job" }, contact: { type: "needs_auth", since, cause: "Duo approval required" } });
  const { container, unmount } = await banner([auth], { workspace });
  await settle();
  const line = container.querySelector('[role="group"]').textContent.replace(/[⁦-⁩]/g, "");
  assert.ok(line.includes(m.run_notice_needs_auth({ host: "lab-a40" })), line);
  assert.ok(container.querySelector('[role="progressbar"] .bg-subtext'), "the bar freezes in grey");
  const asked = [];
  workspace.events.onHostConnectRequest(() => asked.push(workspace.events.pendingHostConnect()));
  await click([...container.querySelectorAll("button")].find((element) => element.textContent === m.settings_connect()));
  assert.deepEqual(asked, ["lab-a40"]);
  await unmount();

  const away = run("r2", { host: "lab-a40", backend: { kind: "ssh_job" }, contact: { type: "unreachable", since, cause: "timed out" } });
  await banner([away]);
  await settle();
  await click(button(m.run_show_more()));
  assert.ok(find("button", m.run_abandon()), "the strip offers the notice's way out");
  assert.ok(find("p", "timed out"));
});

test("a flag's detail keeps its counts live in the strip", async () => {
  const counted = { ...flagged, counts: { "ERR 429": 43 }, flag: { ...flagged.flag, detail: '{count:"ERR 429"} items came back HTTP 429.' } };
  backend({ monitors: { r1: counted } });
  await banner([run("r1", { monitor: summary({ tone: "flag", flag: { id: "f1", state: "waiting", mode: "copilot" } }) })]);
  await settle();
  assert.ok([...document.querySelectorAll("p")].some((p) => p.textContent.replace(/[⁦-⁩]/g, "").startsWith("43 items came back HTTP 429.")));
});

const plain = (text) => text.replace(/[⁦-⁩]/g, "");
const named = (label) => [...document.querySelectorAll("button")].find((element) => plain(element.textContent) === plain(label)) ?? null;
const waitingRun = () => run("r1", { monitor: summary({ tone: "flag", flag: { id: "f1", state: "waiting", mode: "copilot" } }) });

/** Today's server with run r1 flagged and waiting; each flag answer waits until the test gives it. */
function heldFlag() {
  const choices = [];
  const answers = [];
  serve((path, init) => {
    const request = `${init.method ?? "GET"} ${path}`;
    if (request === "GET /api/settings/ui-state") return { onboardingCompleted: true };
    if (request === "GET /api/compute/hosts") return [];
    if (request === "GET /api/runs/r1/monitor") return flagged;
    if (request === "POST /api/runs/r1/flag") {
      choices.push(JSON.parse(init.body));
      return new Promise((resolve) => answers.push(resolve));
    }
  });
  const answer = (body, status = 200) => act(async () => answers.shift()(Response.json(body, { status })));
  return { choices, answer };
}

test("a flag choice is sent once, and trying it again after a failure is the same request", async () => {
  const { choices, answer } = heldFlag();
  await banner([waitingRun()]);
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
  await answer({ flag: { ...flagged.flag, state: "asking" }, run: waitingRun() });
  await settle();
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
  await act(async () => workspace.queryClient.setQueryData(queryKey, { ...flagged, rev: flagged.rev + 2, flag: { ...flagged.flag, id: "f2" } }));
  await answer({ flag: { ...flagged.flag, state: "kept" }, run: waitingRun() });
  await settle();
  assert.equal(workspace.queryClient.getQueryData(queryKey).flag.id, "f2");
  assert.ok(named(m.run_flag_keep_going()), "the newer flag still asks");
});

test("a choice on its way is never forgotten, however long its answer takes, and is let go once answered", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const { choices, answer } = heldFlag();
  // Its own workspace: a second request would queue behind the first, and must not hold up later tests.
  const workspace = createWorkspace("pending");
  const { queryKey } = workspace.queries.flagChoiceQuery("r1", "f1");
  const keep = () => act(async () => chooseFlag(workspace, { id: "r1" }, "f1", "keep", () => {}));
  await keep();
  await act(async () => t.mock.timers.tick(2 * 3_600_000));
  await keep();
  await answer({ flag: { ...flagged.flag, state: "kept" }, run: waitingRun() });
  await settle();
  assert.equal(choices.length, 1, "nothing on screen, and still one request");
  await act(async () => t.mock.timers.tick(2 * 3_600_000));
  assert.equal(workspace.queryClient.getQueryData(queryKey), undefined);
});

test("a run its newest source says is waiting for a choice opens its strip, whatever its older summary says", async () => {
  backend({ uiState: { onboardingCompleted: true, runBanner: "line" }, monitors: { r1: flagged } });
  await banner([run("r1", { monitor: summary({ rev: flagged.rev - 1, flag: null }) })]);
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
  assert.ok(container.querySelector('[role="progressbar"] .bg-subtext'), "the bar freezes");
});

test("a run whose view was rebuilt reads its monitor again, and shows none of the old one meanwhile", async () => {
  const requests = backend();
  const view = await banner([run("r1")]);
  await settle();
  await view.update([run("r1", { monitor: summary({ generation: "g2", rev: 1, asOf: running.asOf + 1000, bindings: { loss: 1.5 } }) })]);
  await settle();
  assert.equal(requests.filter((r) => r.request === "GET /api/runs/r1/monitor").length, 2);
  const text = plain(view.container.textContent);
  assert.ok(text.includes("Loss 1.50") && !text.includes("2.31"), text);
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
    if (path === "/api/chat/shown-runs") return failing ? Response.json({ error: "The server stopped." }, { status: 500 }) : [item(run("r1"))];
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

test("the evidence's log lines ask to open the run's log at the cited bytes of the cited file", async () => {
  backend({ monitors: { r1: flagged } });
  const opened = [];
  await banner([waitingRun()], { props: { onOpenRun: (...args) => opened.push(args) } });
  await settle();
  await click(find("button", "91"));
  const { byteFrom, ident } = flagged.flag.evidence;
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
  await answer({ flag: flagged.flag, run: waitingRun() });
  await settle();
  assert.equal(choices.length, 1, "presses while the answer was on its way sent nothing, before or after it came back");

  await click(named(m.run_flag_keep_going()));
  assert.equal(choices[1].requestId, choices[0].requestId, "the same choice is the same request");
  await answer({ error: "This flag was already answered." }, 409);
  await settle();
  await reopen();
  assert.ok(find('[role="alert"]', "This flag was already answered."), "the refusal outlives the strip that pressed it");
  await click(named(m.run_flag_keep_going()));
  assert.equal(choices[2].requestId, choices[0].requestId, "trying again is the same request");
  await answer({ flag: { ...flagged.flag, state: "kept" }, run: waitingRun() });
  await settle();
});

test("summaries faster than the catch-up keep the chart and read the monitor again once, two seconds after it fell behind", async (t) => {
  const requests = backend();
  const view = await banner([run("r1", { monitor: summary({ rev: running.rev }) })]);
  await settle();
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const reads = () => requests.filter((r) => r.request === "GET /api/runs/r1/monitor").length;
  for (let step = 1; step <= 4; step++) {
    await view.update([run("r1", { monitor: summary({ rev: running.rev + step, bindings: { loss: 2.3 - step / 10 } }) })]);
    await act(async () => t.mock.timers.tick(700));
    await settle();
    assert.ok(view.container.querySelector('[role="img"][aria-label*="Loss"]'), `the chart keeps drawing at step ${step}`);
  }
  assert.equal(reads(), 2, "one reread, at two seconds, however many summaries arrived");
  assert.ok(plain(view.container.textContent).includes("Loss 1.90"), "the facts are the newest summary's");
});

test("a degraded line names its state and reason on hover, focus and to a screen reader, and announces the change once", async () => {
  const stale = { ...running, availability: { state: "rebuilding", reason: "The run's log moved.", omitted: {} } };
  let monitor = running;
  serve((path) => {
    if (path === "/api/settings/ui-state") return { onboardingCompleted: true, runBanner: "line" };
    if (path === "/api/compute/hosts") return [];
    if (path === "/api/runs/r1/monitor") return monitor;
  });
  const workspace = createWorkspace("test");
  const { container } = await banner([run("r1", { monitor: summary({ rev: running.rev }) })], { workspace });
  await settle();
  monitor = stale;
  await act(async () => workspace.queryClient.refetchQueries({ queryKey: workspace.queries.getRunMonitorQuery("r1").queryKey }));
  await settle();
  const why = `${m.run_view_rebuilding()} · The run's log moved.`;
  const age = m.status_last_seen({ time: plain(fmtClock(running.asOf)) });
  const toggle = container.querySelector('button[aria-expanded="false"]');
  assert.equal(plain(document.getElementById(toggle.getAttribute("aria-describedby")).textContent), why, "the line's description says why");
  const tip = [...container.querySelectorAll('[role="img"]')].find((element) => element.textContent.includes(plain(age).slice(0, 9)));
  assert.equal(plain(tip.getAttribute("aria-label")), `${why} · ${age}`);
  assert.equal(plain(tip.querySelector('[role="tooltip"]').textContent), `${why} · ${age}`, "hover or focus shows the reason");
  assert.equal(plain(status()), `${why}. ${age}.`);
  assert.ok(!plain(container.textContent).includes(m.run_eta_left({ duration: "" }).trim()), "no time left");

  await act(async () => workspace.queryClient.refetchQueries({ queryKey: workspace.queries.getRunMonitorQuery("r1").queryKey }));
  await settle();
  assert.equal(plain(status()), `${why}. ${age}.`, "said once");
});

test("shown runs without a summary draw their terminal status at both levels without fetching a monitor", async () => {
  for (const status of ["done", "failed", "cancelled"]) {
    const requests = backend();
    const opens = [];
    const view = await banner([run("r1", { status, endedAt: now, monitor: undefined })], {
      props: { onOpenExperiment: (...args) => opens.push(args) },
    });
    await settle();
    assert.ok(button(m.run_dismiss()));
    const label = status === "done" ? m.status_done() : status === "failed" ? m.status_failed() : m.run_lead_stopped();
    assert.ok(view.container.textContent.includes(label), view.container.textContent);
    await click(find("button", "r1"));
    assert.equal(opens[0][0], "e-r1");
    await click(button(m.run_show_less()));
    assert.ok(view.container.textContent.includes(label));
    assert.ok(button(m.run_dismiss()));
    await click(find("button", "r1"));
    assert.equal(opens[1][0], "e-r1");
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
  assert.ok(container.textContent.includes(m.run_running_on({ where: "⁨" + m.compute_target_local() + "⁩" })));
  assert.equal(container.querySelector('[role="img"][aria-label*="Loss"]'), null);
  assert.equal(requests.filter((r) => r.request.includes("/monitor")).length, 0);
});
