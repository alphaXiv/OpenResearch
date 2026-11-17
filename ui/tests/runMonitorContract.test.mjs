import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { createElement } from "react";
import { Rail } from "../src/components/run/Rail.tsx";
import { RunBanner } from "../src/components/run/RunBanner.tsx";
import { applyMonitorEvent } from "../src/queries/Events.tsx";
import { m } from "../src/paraglide/messages.js";
import { fmtPercent, fmtSignal } from "../src/runMonitor/format.ts";
import { runModel } from "../src/runMonitor/model.ts";
import { createWorkspace } from "../src/workspace.tsx";
import { render, serve, settle } from "./render.mjs";

/** What the server writes, pinned in `tests/contracts/` by its own test. */
const contract = (name) => JSON.parse(readFileSync(new URL(`../../tests/contracts/${name}.json`, import.meta.url), "utf8"));
const { states } = contract("run-monitor-api");
const [reset, append] = contract("run-monitor-events");
const bare = (text) => text.replace(/[⁦-⁩]/g, "");
const capital = (text) => text.charAt(0).toUpperCase() + text.slice(1);

/** One server state as a run every list carries, its summary on it. */
const runOf = (name, state = states[name]) => {
  const { run, monitor, summary } = state;
  return { id: monitor.runId, experimentId: "exp", projectId: "p", contact: null, cancelling: false, lostReason: null, host: null, ...run, monitor: summary };
};
/** The line (no monitor) or the strip (`full`) of a server state, at the moment the server evaluated it. */
const model = (name, full = true, monitor = states[name].monitor) => runModel({
  item: { sessionId: "s", run: runOf(name), experimentName: null, experimentSlug: "baseline" },
  monitor: full ? monitor : null, shown: true, agent: "Claude", now: states[name].now, nextRetryMs: null,
});

/** A server state drawn the way a chat shows it: the strip unless `level` says the line. */
async function banner(name, level = "strip", state = states[name]) {
  const { monitor } = state;
  serve((path) => path === `/api/runs/${monitor.runId}/monitor` ? monitor : path === "/api/settings/ui-state" ? { onboardingCompleted: true, runBanner: level } : path === "/api/compute/hosts" ? [] : undefined);
  const workspace = createWorkspace("test");
  const { queryKey } = workspace.queries.listChatShownRunsQuery();
  const shift = Date.now() - states[name].now;
  const run = runOf(name, state);
  const moved = { ...run, createdAt: run.createdAt + shift, startedAt: run.startedAt + shift, updatedAt: run.updatedAt + shift, endedAt: run.endedAt && run.endedAt + shift };
  workspace.queryClient.setQueryData(queryKey, { source: "shown", runs: [{ sessionId: "s", run: moved, experimentName: name, experimentSlug: "baseline", runNo: 1, runCount: 1 }] });
  const view = await render(createElement(RunBanner, { sessionId: "s", agent: "Claude" }), workspace);
  await settle();
  return view;
}

test("time left and its range read as `orx exp status` reads them, on the strip and the line", () => {
  for (const [name, { expect }] of Object.entries(states)) {
    for (const full of [true, false]) {
      const view = model(name, full);
      // A finished run says how long it took as its sentence.
      if (expect.eta !== null && !view.settingUp && !states[name].monitor.estimate?.floor) assert.equal(bare(view.eta || view.sentence.text), expect.eta, `${name} ${full}`);
      if (expect.range !== null && !view.settingUp && !states[name].monitor.estimate?.floor) assert.ok(bare(view.etaDetail).startsWith(expect.range), `${name} ${full}: ${view.etaDetail}`);
    }
  }
});

test("a range whose ends read alike is left out", () => {
  const { monitor } = states.running;
  const close = { ...monitor, estimate: { ...monitor.estimate, loMs: 19_600, midMs: 19_800, hiMs: 19_900 } };
  assert.ok(!bare(model("running", true, close).etaDetail).includes(bare(m.run_eta_range({ lo: "19s", hi: "19s" }))));
  assert.ok(bare(model("running").etaDetail).includes(states.running.expect.range));
});

test("with no total the stage reads as steps, from the full monitor and from the summary alone", () => {
  for (const full of [true, false]) {
    assert.equal(bare(model("open", full).sentence.text), `${m.status_running()} · ${capital(bare(m.run_progress_step({ done: "24" })))}`, `full: ${full}`);
    assert.equal(bare(model("running", full).sentence.text), `${m.status_running()} · ${capital(bare(m.run_progress_steps({ done: "200", of: "400" })))}`, `full: ${full}`);
  }
  assert.equal(bare(model("limit").sentence.text), `${m.status_running()} · ${capital(bare(m.run_progress_step({ done: "5" })))}`);
});

test("the caption says only what the sentence doesn't", () => {
  assert.equal(bare(model("running").progress), fmtPercent(0.5));
  const watched = { ...states.running.monitor, sentence: { template: "Loss **{loss}**, falling.", t: states.running.now } };
  assert.equal(bare(model("running", true, watched).progress), fmtPercent(0.5));
  assert.equal(bare(model("limit").progress), bare(m.run_progress_limit({ elapsed: "15m", limit: "1h 0m" })));
});

test("a run past its estimate with no progress since says how long it has been silent, never 0s left", () => {
  for (const full of [true, false]) {
    const view = model("stall", full);
    assert.equal(bare(view.eta), states.stall.expect.eta, `full: ${full}`);
    assert.equal(view.leftMs, null, `full: ${full}: no countdown`);
  }
  const { monitor, now } = states.stall;
  const going = model("stall", true, { ...monitor, estimate: { ...monitor.estimate, at: now - 1_000 } });
  assert.match(bare(going.eta), /left$/, "still within its estimate it counts down");
});

test("the bar's accessible text is one sentence, and elapsed time says it is elapsed", () => {
  for (const name of Object.keys(states)) {
    const { label } = model(name).bar;
    assert.ok(label.endsWith(".") && !/[.。]\s*[,.]/.test(label), `${name}: ${label}`);
  }
  assert.ok(bare(model("open").bar.label).includes(bare(m.run_elapsed({ duration: "20s" }))), model("open").bar.label);
});

test("a finished run says it finished, and the line's right carries its number instead", async () => {
  for (const full of [true, false]) {
    const view = model("ended", full);
    assert.deepEqual(view.bar.segments, [{ weight: 1, fill: 1 }]);
    assert.equal(bare(view.sentence.text), bare(m.run_done_in({ duration: "10m" })), `full: ${full}`);
    assert.equal(view.eta, "", "said once");
  }
  const { container } = await banner("ended", "line");
  const text = bare(container.textContent);
  assert.equal(text.split(bare(m.run_done_in({ duration: "10m" }))).length, 2, text);
  assert.ok(!text.includes(bare(m.run_progress_steps({ done: "100", of: "100" }))), text);
  assert.ok(text.includes(bare(model("ended").headline.value)), text);
});

/** Why each state with no estimate has none, in the page's words for the reason `orx exp status` gives. */
const reasons = { "no progress found in the log": m.run_eta_no_progress(), "no total or time limit": m.run_eta_no_total(), "how much is left is unknown": m.run_eta_unknown_left() };
const noEstimate = Object.fromEntries(Object.entries(states).flatMap(([name, { expect, summary }]) => {
  const why = expect.cli?.match(/^No estimate \((.+)\)$/)?.[1];
  // A run still setting up says so instead.
  return why && !summary.setup ? [[name, reasons[why]]] : [];
}));

test("every state with no estimate gives a reason both sides share", () => {
  assert.ok(noEstimate.open);
  assert.ok(Object.values(noEstimate).every(Boolean), JSON.stringify(noEstimate));
});

test("unknown whole-run progress reads the step, the time so far and why there is no estimate, on the strip and the line", async () => {
  for (const [name, why] of Object.entries(noEstimate)) {
    for (const full of [true, false]) {
      const view = model(name, full);
      assert.equal(bare(view.etaDetail), view.settingUp || name === "open" ? "" : bare(why), `${name} ${full}`);
      assert.equal(bare(view.eta), bare(m.run_elapsed({ duration: bare(view.eta).split(" ")[0] })), `${name} ${full}: ${view.eta}`);
      assert.equal(view.bar.segments.length, 1, `${name} ${full}`);
      assert.ok(!bare(`${view.progress} ${view.eta}`).includes(fmtPercent(1)), `${name} ${full}: ${view.progress} ${view.eta}`);
    }
  }
  assert.equal(bare(model("full").sentence.text), `${m.run_stage_setup()} · ${capital(bare(m.run_progress_steps({ done: "4", of: "4" })))}`);
  const { container } = await banner("full");
  const text = bare(container.textContent);
  assert.ok(!text.includes(bare(m.run_eta_no_total())), text);
  assert.ok(!text.includes(fmtPercent(1)) && !text.includes(bare(m.run_eta_left({ duration: "0s" }))), text);
  const bar = container.querySelector('[role="progressbar"]');
  assert.ok(bar, "the stage keeps its own fill");
  assert.equal(bar.getAttribute("aria-valuenow"), null);
});

test("a run whose log has said nothing orx recognises is setting up, and says what it printed last", () => {
  const { setup } = states.setup.summary;
  assert.equal(setup.line, "Successfully downloaded shard_00001.parquet");
  for (const full of [true, false]) {
    const view = model("setup", full);
    assert.equal(bare(view.sentence.text), `${m.run_stage_setup()} · ${setup.line}`, `full: ${full}`);
    assert.equal(view.etaDetail, "", `full: ${full}: no fault to explain`);
    assert.equal(view.eta, "", `full: ${full}`);
    assert.deepEqual(view.bar.segments, [{ weight: 1, fill: null }], `full: ${full}`);
  }
});

test("a stage the run declared and has no step of yet says it started, and what it printed since, with no fault to explain", () => {
  const { setup } = states.announced.summary;
  assert.equal(setup.stage, "train");
  for (const full of [true, false]) {
    const view = model("announced", full);
    assert.equal(bare(view.sentence.text), `${m.run_stage_setup()} · ${capital(bare(m.run_stage_started({ stage: "Train" })))}`, `full: ${full}`);
    assert.equal(view.etaDetail, "", `full: ${full}`);
    assert.equal(view.eta, "", `full: ${full}`);
  }
  const printing = { ...setup, line: "Loading 9 shards" };
  const state = { ...states.announced, summary: { ...states.announced.summary, setup: printing } };
  const line = runModel({ item: { sessionId: "s", run: runOf("announced", state), experimentName: null, experimentSlug: "baseline" }, monitor: null, shown: true, agent: "Claude", now: state.now + 60 * 60_000, nextRetryMs: null });
  assert.equal(bare(line.sentence.text), "Setup · Loading 9 shards");
});

test("a setting-up run with nothing printed yet keeps its machine off the line", () => {
  const setup = { ...states.setup.summary.setup, line: null };
  const state = { ...states.setup, summary: { ...states.setup.summary, setup }, monitor: { ...states.setup.monitor, setup } };
  for (const full of [true, false]) {
    const view = runModel({ item: { sessionId: "s", run: runOf("setup", state), experimentName: null, experimentSlug: "baseline" }, monitor: full ? state.monitor : null, shown: true, agent: "Claude", now: state.now, nextRetryMs: null });
    assert.equal(bare(view.sentence.text), bare(m.run_stage_setup()), `full: ${full}`);
  }
});

test("a log line never draws as bold, and a long one is shortened", () => {
  const at = (line) => {
    const state = { ...states.setup, summary: { ...states.setup.summary, setup: { ...states.setup.summary.setup, line } } };
    return bare(runModel({ item: { sessionId: "s", run: runOf("setup", state), experimentName: null, experimentSlug: "baseline" }, monitor: null, shown: true, agent: "Claude", now: state.now, nextRetryMs: null }).sentence.text);
  };
  assert.ok(!at("**bold** claim").includes("**"));
  const long = at(`${"word ".repeat(60)}`);
  assert.ok(long.endsWith("…") && long.length < 140, long);
});

test("after a long time with nothing recognised the run is just running, with no estimate and why", () => {
  const state = states.setup;
  for (const full of [true, false]) {
    const view = runModel({ item: { sessionId: "s", run: runOf("setup"), experimentName: null, experimentSlug: "baseline" }, monitor: full ? state.monitor : null, shown: true, agent: "Claude", now: state.now + 16 * 60_000, nextRetryMs: null });
    assert.ok(view.settingUp, `full: ${full}`);
    assert.equal(view.etaDetail, "", `full: ${full}`);
  }
});

test("an older server's run, which says nothing of setup, reads as it always did", () => {
  const { setup: _, ...summary } = states.setup.summary;
  const state = { ...states.setup, summary };
  const view = runModel({ item: { sessionId: "s", run: runOf("setup", state), experimentName: null, experimentSlug: "baseline" }, monitor: null, shown: true, agent: "Claude", now: state.now, nextRetryMs: null });
  assert.equal(bare(view.sentence.text), bare(m.status_running()));
  assert.equal(bare(view.etaDetail), bare(m.run_eta_no_progress()));
});

test("the line says its time is elapsed, and why there is no estimate in its description", async () => {
  for (const [name, why] of Object.entries(noEstimate)) {
    if (!model(name, false).etaDetail) continue;
    const { container } = await banner(name, "line");
    const button = container.querySelector("[aria-describedby]");
    const time = bare(model(name, false).eta);
    assert.ok(bare(container.textContent).includes(time), `${name}: ${container.textContent}`);
    const description = container.querySelector(`[id="${button.getAttribute("aria-describedby")}"]`);
    assert.equal(bare(description?.textContent ?? ""), bare(why), name);
  }
});

test("a detected counter times only its stage and promises no whole-run finish", async () => {
  for (const full of [true, false]) {
    const view = model("detected", full);
    assert.equal(view.leftMs, null);
    assert.equal(view.etaDetail, "");
    assert.equal(view.bar.whole, null);
  }
  const { container } = await banner("detected");
  assert.ok(!bare(container.textContent).includes("done about"));
});

test("the line and the strip say the same whether the estimate waits on its pace or on progress never reported", () => {
  for (const [name, eta] of [["named", ""], ["first", m.run_elapsed({ duration: "10s" })]]) {
    const [line, strip] = [model(name, false), model(name)];
    assert.equal(bare(line.eta), bare(eta), name);
    assert.deepEqual([line.eta, line.etaDetail], [strip.eta, strip.etaDetail], name);
  }
});

test("a run whose projection fails or whose signals can't be read is drawn as last seen, never counting down, wherever it is listed", async () => {
  const { summary, now } = states.failing;
  assert.equal(summary.etaMs, null);
  assert.notEqual(states.unreadable.summary.unavailable, summary.unavailable, "each cause in its own words");
  for (const name of ["failing", "unreadable"]) {
    const { summary, now } = states[name];
    for (const later of [60_000, 600_000]) {
      for (const full of [true, false]) {
        const at = `${name} ${later} ${full}`;
        const view = runModel({
          item: { sessionId: "s", run: runOf(name), experimentName: null, experimentSlug: "baseline" },
          monitor: full ? states[name].monitor : null, shown: true, agent: "Claude", now: now + later, nextRetryMs: null,
        });
        assert.equal(view.eta, "", at);
        assert.ok(view.bar.frozen, at);
        assert.equal(view.stale?.state, "unavailable", at);
        assert.ok(view.stale.why.includes(summary.unavailable), view.stale.why);
        assert.ok(!bare(view.bar.label).includes(bare(m.run_eta_left({ duration: "" })).trim()), view.bar.label);
      }
    }
    const { container } = await banner(name, "line");
    const text = bare(container.textContent);
    assert.ok(!text.includes(bare(m.run_eta_left({ duration: "" })).trim()), text);
    assert.ok(!text.includes(bare(m.run_eta_no_progress())), text);
    const button = container.querySelector("[aria-describedby]");
    const description = container.querySelector(`[id="${button.getAttribute("aria-describedby")}"]`);
    assert.ok(bare(description.textContent).includes(summary.unavailable), description.textContent);
    assert.ok(!bare(description.textContent).includes(bare(m.run_eta_no_progress())), description.textContent);
  }
});

test("a failing line's clock stops where its data stopped", () => {
  for (const name of ["failing", "unreadable", "limit"]) {
    const state = name === "limit" ? { ...states.limit, summary: { ...states.limit.summary, unavailable: states.failing.summary.unavailable } } : states[name];
    const at = (later, full) => runModel({
      item: { sessionId: "s", run: runOf(name, state), experimentName: null, experimentSlug: "baseline" },
      monitor: full ? state.monitor : null, shown: true, agent: "Claude", now: state.now + later, nextRetryMs: null,
    });
    for (const full of [true, false]) assert.equal(JSON.stringify(at(600_000, full)), JSON.stringify(at(60_000, full)), `${name} ${full}`);
  }
});

test("a run that ended while its projection fails is listed as ended, in the tone the server gives", () => {
  const { summary, now } = states.failing_done;
  assert.equal(summary.tone, "done");
  for (const full of [true, false]) {
    const done = runModel({
      item: { sessionId: "s", run: runOf("failing_done"), experimentName: null, experimentSlug: "baseline" },
      monitor: full ? states.failing_done.monitor : null, shown: true, agent: "Claude", now: now + 60_000, nextRetryMs: null,
    });
    assert.equal(done.tone, "done", `full: ${full}`);
    assert.equal(bare(done.sentence.text), bare(m.run_done_in({ duration: "22s" })), `full: ${full}`);
  }
});

test("bars a log printed have no names, so the strip lists no stages for them", async () => {
  const { container } = await banner("epochs");
  assert.ok(states.epochs.monitor.phases.length > 1);
  assert.ok(!container.querySelector("ol"), container.textContent);
});

test("several bars draw only the current fill and claim no share of the whole run", async () => {
  assert.deepEqual(model("epochs").bar.segments, [{ weight: 1, fill: 0.25 }]);
  assert.deepEqual(model("epochs", false).bar.segments, [{ weight: 1, fill: 0.25 }]);
  const { container } = await render(createElement(Rail, { model: model("epochs"), kind: "thick" }));
  const bar = container.querySelector('[role="progressbar"]');
  assert.equal(bar.children.length, 1);
  assert.equal(bar.querySelector("i").style.width, "25%");
  assert.equal(bar.getAttribute("aria-valuenow"), null);
});

test("a NaN headline is drawn as NaN in the strip", async () => {
  serve((path) => path === "/api/runs/r1/monitor" ? states.nan.monitor : path === "/api/settings/ui-state" ? { onboardingCompleted: true } : path === "/api/compute/hosts" ? [] : undefined);
  const workspace = createWorkspace("test");
  const { queryKey } = workspace.queries.listChatShownRunsQuery();
  const run = { ...runOf("nan"), createdAt: Date.now(), startedAt: Date.now(), updatedAt: Date.now() };
  workspace.queryClient.setQueryData(queryKey, { source: "shown", runs: [{ sessionId: "s", run, experimentName: "nan", experimentSlug: "baseline", runNo: 1, runCount: 1 }] });
  const { container } = await render(createElement(RunBanner, { sessionId: "s", agent: "Claude" }), workspace);
  await settle();
  const numbers = [...container.querySelectorAll(".tabular-nums")].map((element) => bare(element.textContent));
  assert.ok(numbers.includes("NaN"), numbers.join(" | "));
});

test("the server's events: a reset rereads, an append on the same basis extends each series once", () => {
  const { monitor } = states.running;
  assert.equal(applyMonitorEvent(monitor, { ...reset, generation: monitor.generation }), null);
  const next = applyMonitorEvent(monitor, append);
  assert.ok(next, "the append continues the monitor it was cut from");
  assert.equal(next.rev, append.rev);
  assert.equal(next.seriesRev, append.seriesRev);
  const steps = next.series.loss.map(([x]) => x);
  assert.equal(new Set(steps).size, steps.length, "each step once");
  assert.deepEqual(next.series.loss.slice(-2), append.append.loss);
  assert.equal(next.latest.loss, append.latest.loss);
  assert.equal(applyMonitorEvent(next, append), next, "a repeated event changes nothing");
});

test("a number no view gave digits reads by its size, as `orx exp status` reads it", () => {
  const { monitor, expect } = states.measures;
  for (const [name, words] of Object.entries(expect.measures)) {
    assert.equal(monitor.view.signals[name].digits, undefined, name);
    assert.equal(bare(fmtSignal(monitor.view.signals[name], monitor.latest[name])), words, name);
  }
  for (const { value, spec, words } of expect.declared) assert.equal(bare(fmtSignal(spec, value)), words, JSON.stringify(spec));
});

test("a table's columns come in the order the run printed them, and the headline is the run's own measure, never a row's column", () => {
  const { monitor, expect } = states.measures;
  const table = monitor.view.blocks.find((block) => block.table);
  assert.deepEqual(table.columns, expect.columns);
  assert.deepEqual(monitor.view.headline, [expect.headline]);
  assert.ok(!expect.columns.includes(expect.headline), expect.headline);
  assert.deepEqual(monitor.view.blocks[0].chart, [expect.headline], "the strip draws the headline's chart first");
  const stats = monitor.view.blocks.find((block) => block.stats)?.stats ?? [];
  assert.ok(stats.every((name) => !expect.columns.includes(name)), `a row's columns are no stats: ${stats}`);
  for (const full of [true, false]) assert.equal(model("measures", full).headline?.label, expect.headline, `full: ${full}`);
});

test("stage names and counts start with capitals", () => {
  for (const full of [true, false]) {
    assert.equal(bare(model("named", full).sentence.text), "Setup · Train Step **2,280** of **4,200**", `full: ${full}`);
    assert.equal(bare(model("measures", full).sentence.text), "Round · Round **14** of **24**", `full: ${full}`);
  }
});

test("beside the headline, better or worse is said in words on the line and the strip, the difference by the number rule", async () => {
  const state = states.running;
  const name = state.monitor.view.headline[0];
  for (const [vs, words] of [
    [{ d: -0.1123, good: true, flat: false, mag: 0.1123, parent: 1 }, m.run_vs_better({ delta: "0.112" })],
    [{ d: 0.00004, good: false, flat: false, mag: 0.00004, parent: 1 }, m.run_vs_worse({ delta: "4e-5" })],
  ]) {
    const compared = { ...state, monitor: { ...state.monitor, versus: { step: { [name]: vs }, final: { [name]: vs } } }, summary: { ...state.summary, headline: { ...state.summary.headline, vs } } };
    for (const level of ["line", "strip"]) {
      const { container } = await banner("running", level, compared);
      for (const hidden of container.querySelectorAll(".sr-only")) hidden.remove();
      const shown = bare(container.textContent);
      assert.ok(shown.includes(`${bare(words)} ${m.run_than_parent()}`), `${level}: ${shown}`);
    }
  }
});

test("a run compared with its parent says by how much, as `orx exp status` says it", () => {
  const { expect } = states.compared;
  for (const full of [true, false]) {
    const { headline } = model("compared", full);
    assert.equal(headline.label, "Loss", `full: ${full}`);
    assert.equal(headline.delta.good, true, `full: ${full}`);
    assert.ok(expect.said.includes(`${bare(headline.delta.magnitude)} better than parent`), `full: ${full}`);
  }
});

test("a stage going far slower than the run that timed it says by how much against which run, while it goes", () => {
  const { monitor } = states.running;
  for (const [than, words] of [["parent", m.run_pace_slower_parent({ times: "3" })], ["earlier", m.run_pace_slower_earlier({ times: "2.5" })]]) {
    const times = than === "parent" ? 3 : 2.5;
    assert.equal(bare(model("running", true, { ...monitor, pace: { times, than } }).note), bare(words));
  }
  assert.equal(model("running").note, null);
});

test("a failed run that recorded no message says the last error its log names, quoted on one line", () => {
  const { summary, now } = states.running;
  const line = "torch.OutOfMemoryError: CUDA out of memory.   Tried *2.00* GiB";
  const run = { ...runOf("running"), status: "failed", endedAt: now, monitor: { ...summary, tone: "failed", logError: line } };
  const view = runModel({ item: { sessionId: "s", run, experimentName: null, experimentSlug: "baseline" }, monitor: null, shown: true, agent: "Claude", now, nextRetryMs: null });
  assert.equal(bare(view.note), bare(m.run_failed_log_error({ line: "torch.OutOfMemoryError: CUDA out of memory. Tried ∗2.00∗ GiB" })));
  const silent = runModel({ item: { sessionId: "s", run: { ...run, monitor: { ...run.monitor, logError: null } }, experimentName: null, experimentSlug: "baseline" }, monitor: null, shown: true, agent: "Claude", now, nextRetryMs: null });
  assert.equal(silent.note, null);
});

test("a floor counted down to nothing reads as no estimate, naming the stages no earlier run timed", () => {
  const { now, monitor } = states.planned;
  for (const full of [true, false]) {
    const later = runModel({
      item: { sessionId: "s", run: runOf("planned"), experimentName: null, experimentSlug: "baseline" },
      monitor: full ? monitor : null, shown: true, agent: "Claude", now: now + monitor.estimate.loMs + 500, nextRetryMs: null,
    });
    assert.match(bare(later.etaDetail), /eval.*not timed/, `full: ${full}`);
    assert.equal(later.leftMs, null, `full: ${full}`);
  }
});

test("a timed floor names untimed stages and leaves one stage-local bar", async () => {
  for (const full of [true, false]) {
    const view = model("planned", full);
    assert.ok(view.leftMs > 0);
    assert.match(bare(view.eta), /At least .* left/);
    assert.match(bare(view.etaDetail), /eval.*not timed/);
    assert.equal(view.bar.segments.length, 1);
    assert.equal(view.bar.whole, null);
  }
  const { container } = await banner("planned");
  assert.equal(container.querySelector("ol"), null);
});
