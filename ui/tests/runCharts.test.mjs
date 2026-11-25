import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { act, createElement } from "react";
import { Block } from "../src/components/run/blocks/Block.tsx";
import { m } from "../src/paraglide/messages.js";
import { runModel } from "../src/runMonitor/model.ts";
import { press, render, serveApp, settle } from "./render.mjs";

const read = (url) => JSON.parse(readFileSync(new URL(url, import.meta.url), "utf8"));
/** The view the server parses the codec benchmark's markup to: its candidate over the parent's, the reference measured in the run drawn once. */
const frontier = read("../../tests/contracts/run-view-frontier.json");
const views = { frontier: { view: frontier.tree, markup: frontier.markup } };

/** A codec's frontier, a sweep inside one run, latency against load, an eval by category. */
function fixture(name) {
  const monitor = read(`./fixtures/runMonitor/${name}.json`);
  const view = views[name];
  return view ? { ...monitor, view: view.view, viewFrom: { ...monitor.viewFrom, markup: view.markup } } : monitor;
}
// A chart sizes itself to its box, which happy-dom never lays out.
let chartWidth = 800;
for (const size of ["clientHeight", "clientWidth", "offsetHeight", "offsetWidth"]) {
  Object.defineProperty(HTMLElement.prototype, size, { configurable: true, get: () => size.endsWith("Width") ? chartWidth : 800 });
}

/** What a reader sees, without the marks that keep numbers and names whole in either direction. */
const text = (element) => element.textContent.replace(/[⁦-⁩]/g, "");
const chartOf = (container) => container.querySelector('svg[role="img"]');
const labels = (container) => [...chartOf(container).querySelectorAll("text, foreignObject")].map(text);
const name = (container) => chartOf(container).getAttribute("aria-label").replace(/[⁦-⁩]/g, "");
const with_ = (monitor, block) => ({ ...monitor, view: { ...monitor.view, blocks: [block] } });

async function draw(monitor, block, size = "page", { live = true } = {}) {
  serveApp();
  const view = await render(createElement(Block, { monitor, block, size, projectId: "p", tone: live ? "running" : "done", live }));
  await settle();
  return view.container;
}

/** Moves the pointer over a page's chart to `x` pixels from its start. */
async function point(container, x) {
  const box = chartOf(container).parentElement;
  await act(async () => box.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: x })));
  return box.querySelector("[popover]");
}

test("a codec's frontier draws each series against its own x on a log axis, the parent's under it, and names both axes by their units", async () => {
  const monitor = fixture("frontier");
  const block = monitor.view.blocks[0];
  const page = await draw(monitor, block);
  assert.equal(text(page.querySelector("h3")), "Size against compress time");
  assert.match(text(page), /ParentReference buildCandidate/, "a legend names each series and the parent once");
  assert.equal(name(page), "Reference build: 9 points from 267 to 439; Candidate: 9 points from 264 KB to 404 KB");
  const ticks = labels(page);
  for (const tick of ["2", "5", "10", "20", "50", "100", "200", "500"]) assert.ok(ticks.includes(tick), `a log tick at ${tick}: ${ticks.join(" | ")}`);
  assert.ok(ticks.includes("ms, log") && ticks.includes("KB"), "the axes say their units and that x is log");
  const chart = chartOf(page);
  assert.equal(chart.querySelectorAll("path[stroke-dasharray]").length, 1, "the parent's candidate, dashed; the reference the run measures itself, once");
  assert.equal(chart.querySelectorAll("path:not([stroke-dasharray])").length, 2);
  assert.ok(!ticks.some((label) => /^\d+(\.\d+)?$/.test(label) && Number(label) > 250 && Number(label) < 500 && !["300", "350", "400", "450"].includes(label)), "no value at a line's end: a table has no latest point");

  const strip = await draw(monitor, block, "strip");
  assert.equal(strip.querySelectorAll("svg text").length, 0, "no axes in a strip");
  assert.equal(name(strip), name(page));
  assert.equal(chartOf(strip).querySelectorAll("path").length, 3);

  const both = await draw(monitor, { ...block, series: block.series.map(({ ghost, ...one }) => one) });
  assert.equal(chartOf(both).querySelectorAll("path[stroke-dasharray]").length, 2, "every series has the parent's under it unless it says not");
  const readout = await point(both, 380);
  assert.match(text(readout), /Reference build, parent[\d.]+/, "two of the parent's lines are told apart by name");
});

test("the readout under the pointer gives each series its value, and its own x where it isn't the nearest one's", async () => {
  const monitor = fixture("frontier");
  const page = await draw(monitor, monitor.view.blocks[0]);
  const readout = await point(page, 380);
  assert.ok(readout, "a readout shows");
  const said = text(readout);
  assert.match(said, /^Compress time [\d.]+ ms/, said);
  assert.match(said, /Candidate[\d.]+ KB/, said);
  assert.match(said, /Reference build[\d.]+( KB)? at [\d.]+/, said);
});

test("a strip's chart names its axes in one line, reads out under the pointer, and steps through its points by arrow key", async () => {
  const monitor = fixture("sweep");
  const block = monitor.view.blocks[0];
  const strip = await draw(monitor, block, "strip");
  const box = chartOf(strip).parentElement;
  assert.match(text(box.querySelector("div[aria-hidden]")), /Learning rate \S+ → \S+ log.*Val loss [\d.]+ → [\d.]+/, "what x and y are, and how far each reaches");

  await act(async () => box.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: 400 })));
  const said = text(box.querySelector("[popover]"));
  assert.match(said, /^Learning rate [\d.]+/, said);
  assert.match(said, /Batch \d+.*[\d.]+/, "a value for each line beside the pointer");
  await act(async () => box.dispatchEvent(new MouseEvent("mouseout", { bubbles: true, relatedTarget: document.body })));
  assert.ok(!box.querySelector("[popover]"), "the readout closes when the pointer leaves");

  await press("ArrowLeft", box);
  const first = text(box.querySelector("[popover]"));
  const status = box.querySelector('[role="status"]');
  const announced = text(status).replace(/: |, /g, "").replace(/\s/g, "");
  assert.equal(announced, first.replace(/\s/g, ""), "a keyboard reader hears what it shows");
  await press("Home", box);
  assert.notEqual(text(box.querySelector("[popover]")), first, "Home reads the first point, not the last");
  await press("Escape", box);
  assert.ok(!box.querySelector("[popover]"), "Escape closes the readout");
  assert.equal(text(status), "");
});

test("a readout never gives a value beside a step it wasn't recorded at", async () => {
  const monitor = fixture("finetune");
  const block = monitor.view.blocks[0];
  const strip = await draw(monitor, block, "strip");
  const box = chartOf(strip).parentElement;
  await act(async () => box.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: 300 })));
  const said = text(box.querySelector("[popover]"));
  const [, step] = said.match(/^step ([\d,.]+)/) ?? [];
  assert.ok(step, said);
  const parents = said.match(/parent[\d.]+ at [\d,.]+/g) ?? [];
  assert.equal(parents.length, 2, `the parent samples every 100 steps, so each of its values names its own step: ${said}`);
  assert.ok(parents.every((one) => !one.endsWith(` at ${step}`)), said);
});

test("elapsed chart axes and readouts say durations for elapsed points and timed stages", async (t) => {
  t.after(() => { chartWidth = 800; });
  chartWidth = 192;
  const base = fixture("running");
  const block = { chart: ["loss"] };
  for (const x of [{ name: "elapsed", max: null }, { name: "step", max: 2400 }]) {
    const phases = [{ ...base.phases[0], name: "train", unit: x.max ? "seconds" : null }];
    const monitor = { ...base, x, phases, progress: { ...base.progress, phase: 0 }, series: { loss: [[0, 3.27], [43, 2.1], [300, 1.66]] } };
    const strip = await draw(monitor, block, "strip");
    assert.match(text(strip), new RegExp(`elapsed 0s → ${x.max ? "40m" : "5m"}`, "i"));
    const path = chartOf(strip).querySelector("path").getAttribute("d");
    const [, vertex] = path.matchAll(/[ML]([\d.]+) /g);
    const pointer = await point(strip, Number(vertex[1]));
    assert.match(text(pointer), /^elapsed 43s/i);
    const box = chartOf(strip).parentElement;
    await press("ArrowRight", box);
    assert.match(text(box.querySelector('[role="status"]')), /(?:^|at )elapsed 5m:/i, "the last point, the lowest, is named");

    const page = await draw(monitor, block);
    assert.ok(labels(page).some((label) => /^elapsed$/i.test(label)));
  }
});

test("a readout uses reports under a smoothed line and a clipped parent", async () => {
  const base = fixture("finetune");
  const monitor = {
    ...base,
    x: { name: "step", max: 10 },
    view: { ...base.view, signals: { loss: { label: "Train loss", smooth: 0.5 } } },
    series: { loss: [[0, 2], [10, 6]] },
    parent: { ...base.parent, series: { loss: [[0, 4], [20, 8]] } },
  };
  for (const size of ["page", "strip"]) {
    const chart = await draw(monitor, { chart: ["loss"], ghost: "parent" }, size);
    const box = chartOf(chart).parentElement;
    await press("End", box);
    const readout = text(box.querySelector("[popover]"));
    assert.match(readout, /^step 10Train loss6(?:\.0+)?smoothed 4(?:\.0+)?$/, readout);
    assert.match(text(box.querySelector('[role="status"]')), /^step 10: Train loss 6(?:\.0+)?, smoothed 4(?:\.0+)?$/);
    assert.ok(chartOf(chart).querySelector("path[stroke-dasharray]"), "the parent curve is still drawn");
  }
});

test("a strip's keyboard entry reads the run's own latest point with every line there, and its focus ring stops above the axis", async () => {
  const monitor = fixture("finetune");
  const short = (points) => points.filter(([step]) => step <= 500);
  const behind = { ...monitor, series: { ...monitor.series, loss: short(monitor.series.loss), val_loss: short(monitor.series.val_loss) } };
  const latest = behind.series.loss.at(-1)[0];
  const strip = await draw(behind, monitor.view.blocks[0], "strip");
  const box = chartOf(strip).parentElement;
  await press("ArrowLeft", box);
  const said = text(box.querySelector("[popover]"));
  assert.match(said, new RegExp(`^step ${latest}(?!\\d)`), `the parent runs on to ${monitor.x.max}, but the run is at ${latest}: ${said}`);
  assert.equal(said.match(/parent/g)?.length, 2, said);
  assert.match(said, /Val loss[\d.]+Train loss[\d.]+(smoothed [\d.]+)?$/, `the run's own lines are there too: ${said}`);

  const ring = box.querySelector(":scope > span[aria-hidden]");
  const axis = [...chartOf(strip).querySelectorAll("line")].find((line) => line.getAttribute("stroke") === "var(--border-variant)");
  assert.ok(ring && axis, "a ring and an axis line");
  assert.ok(parseFloat(ring.style.top) + parseFloat(ring.style.height) <= Number(axis.getAttribute("y1")), "the ring ends where the axis line begins, clear of the labels under it");
});

test("a readout stays under the pointer while the run adds points", async () => {
  const monitor = fixture("finetune");
  const block = monitor.view.blocks[0];
  serveApp();
  const view = await render(createElement(Block, { monitor, block, size: "strip", projectId: "p", tone: "running", live: true }));
  const box = chartOf(view.container).parentElement;
  await act(async () => box.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: 300 })));
  assert.ok(box.querySelector("[popover]"));
  const loss = monitor.series.loss;
  const grown = { ...monitor, series: { ...monitor.series, loss: [...loss, [loss.at(-1)[0] + 1, loss.at(-1)[1]]] } };
  await view.rerender(createElement(Block, { monitor: grown, block, size: "strip", projectId: "p", tone: "running", live: true }));
  assert.ok(box.querySelector("[popover]"), "a new point doesn't close it");
});

test("one line per value of a column, and a cell that isn't a number is counted under the chart, never dropped silently", async () => {
  const monitor = fixture("sweep");
  const block = monitor.view.blocks[0];
  const page = await draw(monitor, block);
  assert.equal(text(page.querySelector("h3")), "Val loss against learning rate");
  assert.match(text(page), /Batch 32Batch 64Batch 128/);
  assert.equal(chartOf(page).querySelectorAll("path").length, 3);
  assert.match(name(page), /^Batch 32: 5 points from [\d.]+ to [\d.]+; Batch 64: 5 points .*; Batch 128: 3 points .*\. 1 point has no number and is left out\.$/);
  assert.equal(text(page.querySelector("p")), "1 point has no number and is left out.");
  assert.ok(labels(page).includes("Learning rate, log"), "with no unit, the x axis says its label");
});

test("more lines than a chart draws says how many are shown", async () => {
  const monitor = fixture("sweep");
  const rows = Array.from({ length: 10 }, (_, batch) => ({ lr: 1e-3, batch, val_loss: 2 + batch / 10 }));
  const page = await draw({ ...monitor, rows: { configs: rows } }, monitor.view.blocks[0]);
  assert.equal(chartOf(page).querySelectorAll("circle").length, 8);
  assert.equal(text(page.querySelector("p")), "The first 8 of 10 series are shown.");
});

test("points on log axes leave out what is at or below zero, and say how many", async () => {
  const monitor = fixture("latency");
  const block = monitor.view.blocks[0];
  const page = await draw(monitor, block);
  const chart = chartOf(page);
  assert.equal(chart.querySelectorAll("path").length, 0, "points, no lines");
  assert.equal(chart.querySelectorAll("circle").length, 21, "seven loads of eight, three percentiles each");
  assert.ok(labels(page).includes("ms, log") && labels(page).includes("req/s, log"));
  assert.equal(text(page.querySelector("p")), "3 points at or below zero are left off the log scale.");
  assert.match(name(page), /p99: 7 points from 21\.5 ms to 230 ms\. 3 points at or below zero are left off the log scale\.$/);

  const strip = await draw(monitor, block, "strip");
  assert.equal(chartOf(strip).querySelectorAll("circle").length, 21);
  assert.match(name(strip), /left off the log scale/, "a strip has no room for the note, so its picture's name says it");
});

test("a curve over steps on a log y keeps its line and breaks it where a value is off the scale", async () => {
  const monitor = fixture("finetune");
  const block = { chart: ["loss"], x: "step", yScale: "log" };
  const losses = { ...monitor, view: { ...monitor.view, signals: { loss: { label: "Train loss" } } }, series: { loss: [[1, 4], [2, 2], [3, 0], [4, 0.5], [5, 0.25]] }, parent: null };
  const page = await draw(losses, block);
  const path = chartOf(page).querySelector("path").getAttribute("d");
  assert.equal(path.match(/M/g).length, 2, "the line breaks at the zero");
  assert.ok(labels(page).includes("log"));
  assert.equal(text(page.querySelector("p")), "1 point at or below zero is left off the log scale.");
});

test("bars stand one per category in the table's order, with the parent's level dashed across each", async () => {
  const monitor = fixture("evalbars");
  const block = monitor.view.blocks[0];
  const page = await draw(monitor, block);
  const chart = chartOf(page);
  assert.equal(chart.querySelectorAll("rect").length, 6);
  assert.equal(chart.querySelectorAll("line[stroke-dasharray]").length, 6);
  const ticks = labels(page);
  assert.deepEqual(ticks.filter((label) => /[a-z]/.test(label)), monitor.rows.categories.map((row) => row.category));
  assert.ok(ticks.includes("80%"), `a share reads in percent: ${ticks.join(" | ")}`);
  assert.equal(name(page), "Accuracy: algebra 84.2%, geometry 71.3%, number theory 76.8%, probability 80.1%, calculus 65.5%, combinatorics 58.7%");

  const readout = await point(page, 38 + 756 / 6 / 2);
  assert.match(text(readout), /^algebraParent81\.1%Accuracy84\.2%$/);

  const strip = await draw(monitor, block, "strip");
  assert.equal(chartOf(strip).querySelectorAll("rect").length, 6);
  assert.equal(strip.querySelectorAll("svg text").length, 0);
});

test("a narrow bars chart labels all four categories", async () => {
  const base = fixture("evalbars");
  const categories = ["algebra", "geometry", "proofs", "counting"].map((category, i) => ({ category, accuracy: 0.8 - i * 0.1 }));
  const monitor = { ...base, rows: { categories }, parent: null };
  chartWidth = 303;
  try {
    const page = await draw(monitor, monitor.view.blocks[0]);
    assert.deepEqual(labels(page).filter((label) => /[a-z]/.test(label)), categories.map(({ category }) => category));
  } finally {
    chartWidth = 800;
  }
});

test("repeated categories keep each row's bar and read every value in the group", async () => {
  const monitor = fixture("sweep");
  const block = { chart: ["val_loss"], from: "configs", x: "batch", mark: "bars" };
  for (const size of ["strip", "page"]) {
    const container = await draw(monitor, block, size);
    const bars = [...chartOf(container).querySelectorAll("rect")].map((bar) => ({
      start: Number(bar.getAttribute("x")), end: Number(bar.getAttribute("x")) + Number(bar.getAttribute("width")),
    })).sort((a, b) => a.start - b.start);
    assert.equal(bars.length, 13, "each numeric report has a bar");
    assert.ok(bars.slice(1).every((bar, i) => bar.start >= bars[i].end), "bars do not overlap");
    const first = await point(container, bars[0].start + 1);
    assert.match(text(first), /2\.738.*2\.636.*2\.633.*2\.731.*2\.948/);
  }
});

test("grouped bars put each parent level over its own group's bar", async () => {
  const base = fixture("evalbars");
  const split = (rows) => rows.flatMap((row) => [{ ...row, model: "a" }, { ...row, model: "b", accuracy: row.accuracy / 2 }]);
  const monitor = {
    ...base,
    rows: { categories: split(base.rows.categories) },
    parent: { ...base.parent, rows: { categories: split(base.parent.rows.categories) } },
  };
  const block = { chart: ["accuracy"], from: "categories", x: "category", by: "model", mark: "bars", ghost: "parent" };
  const chart = chartOf(await draw(monitor, block, "page"));
  const bars = [...chart.querySelectorAll("rect")].map((bar) => Number(bar.getAttribute("x")) + Number(bar.getAttribute("width")) / 2);
  const levels = [...chart.querySelectorAll('line[stroke-dasharray="3 2"]')].map((level) => (Number(level.getAttribute("x1")) + Number(level.getAttribute("x2"))) / 2);
  assert.equal(levels.length, bars.length, "every bar has its parent's level");
  assert.deepEqual(levels.map(Math.round).sort((a, b) => a - b), bars.map(Math.round).sort((a, b) => a - b));
});

test("a chart of a table with no rows says so, with a yet only while the run is going, over the parent's when there is one", async () => {
  const monitor = { ...fixture("frontier"), rows: {} };
  const block = monitor.view.blocks[0];
  const page = await draw(monitor, block);
  assert.equal(name(page), m.run_block_no_rows());
  assert.ok(labels(page).includes(m.run_block_no_rows()));
  assert.equal(chartOf(page).querySelectorAll("path[stroke-dasharray]").length, 1, "the parent's frontier, to beat");
  assert.equal(name(await draw(monitor, block, "page", { live: false })), m.run_block_no_rows_ended());
  const strip = await draw(monitor, block, "strip");
  assert.equal(text(strip), m.run_block_no_rows());
});

test("a column the table lacks, or one that holds no numbers, is named, and the other series still draw", async () => {
  const monitor = fixture("frontier");
  const block = monitor.view.blocks[0];
  const missing = { ...monitor, rows: { levels: monitor.rows.levels.map(({ cand_kb, ...row }) => row) } };
  const lacking = await draw(missing, block);
  assert.equal(text(lacking.querySelector("p")), "The levels table has no cand_kb column.");
  assert.equal(chartOf(lacking).querySelectorAll('path:not([stroke-dasharray]):not([d=""])').length, 1, "the reference still draws");

  const words = { ...monitor, parent: null, rows: { levels: monitor.rows.levels.map((row) => ({ ...row, cand_ms: "fast", ref_ms: "slow" })) } };
  const nothing = await draw(words, block);
  assert.equal(name(nothing), "The ref_ms column of levels holds no numbers. The cand_ms column of levels holds no numbers.");
  assert.ok(!nothing.querySelector("p"), "with nothing drawn, the reason is the chart's own words");
  const strip = await draw(words, block, "strip");
  assert.match(text(strip), /The ref_ms column of levels holds no numbers\./);
});

test("a signal's labels name its raw value in stats, table cells and the headline", async () => {
  const monitor = fixture("frontier");
  const stats = await draw(monitor, monitor.view.blocks[1]);
  const valid = [...stats.querySelectorAll("dl > div")].find((stat) => stat.querySelector("dt").textContent === "Bit-exact");
  assert.equal(text(valid.querySelector("dd")).trim(), "valid");

  const rows = monitor.rows.levels.map((row, index) => ({ ...row, valid: index === 3 ? 0 : "1" }));
  const table = await draw({ ...monitor, rows: { levels: rows } }, { table: "levels", columns: ["level", "valid"], last: 9 });
  assert.deepEqual([...table.querySelectorAll("tbody tr")].map((row) => text(row.cells[1])), ["valid", "valid", "valid", "valid", "valid", "invalid", "valid", "valid", "valid"]);

  const headline = { ...monitor, view: { ...monitor.view, headline: ["valid"] }, headline: "valid" };
  const run = { id: "r-codec", experimentId: "e1", projectId: "p", status: "done", createdAt: monitor.asOf - 1_210_000, startedAt: monitor.asOf - 1_210_000, endedAt: monitor.asOf, updatedAt: monitor.asOf, contact: null, cancelling: false, lostReason: null, host: null, backend: { kind: "local" } };
  const model = runModel({ item: { sessionId: "s", run, experimentName: "Wider match window", experimentSlug: "wider-match-window" }, monitor: headline, shown: true, agent: "Claude", now: monitor.asOf, nextRetryMs: null });
  assert.equal(text({ textContent: model.headline.value }), "valid");
});

test("a chart form from a newer server draws as the line it knows", async () => {
  const monitor = fixture("sweep");
  const block = { ...monitor.view.blocks[0], mark: "area", xScale: "sqrt" };
  const page = await draw(monitor, block);
  assert.equal(chartOf(page).querySelectorAll("path").length, 3);
  assert.ok(labels(page).includes("Learning rate"));
});

test("a curve over steps can be drawn as points or as a bar per step", async () => {
  const monitor = fixture("selfplay");
  const points = await draw(monitor, { chart: ["elo"], x: "round", mark: "points" });
  const reported = monitor.series.elo.filter(([, y]) => y !== null).length;
  assert.equal(chartOf(points).querySelectorAll("path").length, 0);
  assert.equal(chartOf(points).querySelectorAll("circle").length, reported);

  const bars = await draw(monitor, { chart: ["elo"], x: "round", mark: "bars" });
  assert.equal(chartOf(bars).querySelectorAll("rect").length, reported);
  assert.match(name(bars), /^Elo gained: 1 \+182, 2 \+313/);
});

/** The furthest any line of a chart reaches across it, in pixels. */
const reach = (container) => Math.max(...[...chartOf(container).querySelectorAll("path")].flatMap((path) => [...path.getAttribute("d").matchAll(/[ML]([\d.-]+) /g)].map((move) => Number(move[1]))));

test("a short child's chart spans its own steps, the parent's line cut at the edge, at every size, unless it asks for the parent's", async () => {
  const base = fixture("finetune");
  const loss = base.series.loss.slice(0, 4);
  const child = { ...base, x: { name: "step", max: 30 }, series: { loss }, latest: { loss: loss[3][1] } };
  const block = { chart: ["loss"], ghost: "parent" };

  const page = await draw(child, block);
  const ticks = labels(page).filter((label) => /^[\d,]+$/.test(label)).map((label) => Number(label.replaceAll(",", "")));
  assert.equal(Math.max(...ticks), 30, `the axis ends at the run's 30 steps: ${ticks.join(" | ")}`);
  assert.ok(reach(page) <= 800 - 48 + 0.1, `the parent's line stops at the edge: ${reach(page)}`);
  assert.equal(chartOf(page).querySelectorAll("path[stroke-dasharray]").length, 1);

  const strip = await draw(child, block, "strip");
  assert.ok(reach(strip) <= 800 - 5 + 0.1, `nor past a strip's: ${reach(strip)}`);

  const ended = await draw({ ...child, x: { name: "step", max: null } }, block, "page", { live: false });
  assert.equal(Math.max(...labels(ended).filter((label) => /^[\d,]+$/.test(label)).map((label) => Number(label.replaceAll(",", "")))), 30, "with no total, the steps it reached");

  const whole = await draw(child, { ...block, xRange: "parent" });
  assert.ok(labels(whole).includes("5,000"), `x-range="parent" reaches the parent's end: ${labels(whole).join(" | ")}`);
});

test("every line of a chart over steps has the parent's under it", async () => {
  const monitor = fixture("finetune");
  const page = await draw(monitor, monitor.view.blocks[0]);
  assert.equal(chartOf(page).querySelectorAll("path[stroke-dasharray]").length, 2, "the parent's val loss and train loss");
  const readout = await point(page, 400);
  assert.match(text(readout), /Val loss, parent[\d.]+.*Train loss, parent[\d.]+/, text(readout));
});

test("a table against the parent pairs the same report of each key at a step, and each key's last once the run ended", async () => {
  const monitor = fixture("pipeline");
  const parentRows = [{ task: "sst2", score: 80 }, { task: "mnli", score: 70 }, { task: "sst2", score: 90 }];
  const compared = { ...monitor, rows: { eval: [{ task: "sst2", score: 85 }] }, parent: { ...monitor.parent, rows: { eval: parentRows } } };
  const cells = (container) => [...container.querySelectorAll("tbody tr")].map((row) => [...row.cells].slice(0, 3).map(text).join(" "));

  const live = await draw(compared, { table: "eval", columns: ["task", "score"], vs: "parent" });
  assert.deepEqual(cells(live), ["sst2 85.0 80.0", "mnli — 70.0", "sst2 — 90.0"], "while it goes, its first sst2 beside the parent's first");
  const ended = await draw(compared, { table: "eval", columns: ["task", "score"], vs: "parent" }, "page", { live: false });
  assert.deepEqual(cells(ended), ["sst2 85.0 90.0", "mnli — 70.0"], "once ended, beside where the parent ended");
  const asked = await draw(compared, { table: "eval", columns: ["task", "score"], vs: "final" });
  assert.deepEqual(cells(asked), cells(ended), "the table's own way wins");
});

test("a table reports signed numeric changes without a preferred direction", async () => {
  const base = fixture("pipeline");
  const monitor = {
    ...base,
    view: { ...base.view, signals: {} },
    rows: { eval: [{ task: "winogrande", score: 0.597 }] },
    parent: { ...base.parent, rows: { eval: [{ task: "winogrande", score: 0.577 }, { task: "sst2", score: 0.9 }] } },
  };
  const block = { table: "eval", columns: ["task", "score"], vs: "parent" };
  const page = await draw(monitor, block);
  assert.equal(text(page.querySelector("tbody tr td:last-child")), "+0.02");

  const pending = await draw({ ...monitor, rows: {} }, block);
  assert.ok(![...pending.querySelectorAll("th")].some((th) => text(th) === m.run_table_change()));
});

/** Where a path passes at `px`, read from the vertices it draws. */
function yAt(path, px) {
  const vertex = [...path.getAttribute("d").matchAll(/[ML]([\d.-]+) ([\d.-]+)/g)].find((one) => Math.abs(Number(one[1]) - px) < 0.1);
  assert.ok(vertex, `the path has a vertex at ${px}`);
  return Number(vertex[2]);
}
/** Moves the pointer over a chart to (`x`, `y`) pixels from its corner. */
async function hover(container, x, y) {
  const box = chartOf(container).parentElement;
  await act(async () => box.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: x, clientY: y })));
  return box.querySelector("[popover]");
}
const lossOver = (series, signal) => {
  const base = fixture("finetune");
  return { ...base, parent: null, x: { name: "step", max: series.at(-1)[0] }, latest: { loss: series.at(-1)[1] }, series: { loss: series },
    view: { ...base.view, signals: { loss: { label: "Loss", digits: 2, ...signal } } } };
};
/** Page geometry for an 800 px chart over steps: 38 px before the plot and 48 after. */
const pageX = (step, max) => 38 + (step / max) * (800 - 38 - 48);

test("a smoothed curve's hover rides the line it draws, marks its report on the faint line under it, and reads both", async () => {
  const monitor = lossOver([[0, 2], [10, 6], [20, 2], [30, 6]], { smooth: 0.5 });
  for (const size of ["page", "strip"]) {
    const chart = await draw(monitor, { chart: ["loss"] }, size);
    const svg = chartOf(chart);
    const [raw, smooth] = svg.querySelectorAll("path");
    assert.equal(raw.getAttribute("opacity"), "0.3", "the reports, faint under the smoothed curve");
    const at = Number(smooth.getAttribute("d").match(/L([\d.]+) /)[1]);
    const next = Number(smooth.getAttribute("d").match(/L[\d.]+ [\d.]+L([\d.]+) /)[1]);
    const readout = await hover(chart, at + (next - at) * 0.4, 0);
    assert.match(text(readout), /^step 10Loss6\.00smoothed 4\.00$/, `between two reports, the nearer one: ${text(readout)}`);
    const cursor = [...svg.querySelectorAll("line")].find((line) => line.getAttribute("x1") === line.getAttribute("x2") && !line.getAttribute("stroke-dasharray"));
    assert.equal(Number(cursor.getAttribute("x1")).toFixed(1), at.toFixed(1), "the cursor stands at the step it reads");
    const marker = (r) => [...svg.querySelectorAll(`g > circle[r="${r}"]`)].find((circle) => Math.abs(Number(circle.getAttribute("cx")) - at) < 0.1);
    assert.equal(Number(marker("4").getAttribute("cy")).toFixed(1), yAt(smooth, at).toFixed(1), "the marker lies on the smoothed curve");
    assert.equal(Number(marker("3").getAttribute("cy")).toFixed(1), yAt(raw, at).toFixed(1), "and the report on the faint one");
  }
});

test("a chart's start and best sit on the reports, and the pointer near either names it", async () => {
  for (const [better, word, best] of [["lower", "Lowest", 2], ["higher", "Highest", 4]]) {
    const values = better === "lower" ? [[0, 3], [1, 2], [2, 4]] : [[0, 3], [1, 4], [2, 2]];
    const monitor = lossOver(values.map(([step, value]) => [step, value]), { better, smooth: 0.5 });
    const page = await draw(monitor, { chart: ["loss"] });
    const raw = chartOf(page).querySelector('path[opacity="0.3"]');
    const landmark = chartOf(page).querySelector('circle[r="3.5"]');
    const [cx, cy] = ["cx", "cy"].map((name) => Number(landmark.getAttribute(name)));
    assert.equal(cy.toFixed(1), yAt(raw, cx).toFixed(1), "the best report sits on the line of reports");
    assert.ok(!chartOf(page).querySelector("title"), "the readout names it, not a tooltip on a dot");

    const readout = await hover(page, cx + 9, cy - 9);
    assert.equal(text(readout).match(/^.*? at step \d+/)?.[0], `${word} loss at step 1`, text(readout));
    assert.match(text(readout), new RegExp(`Loss${best}\\.00smoothed`));
    const start = chartOf(page).querySelector('circle[r="3"]');
    assert.match(text(await hover(page, Number(start.getAttribute("cx")) + 5, Number(start.getAttribute("cy")))), /^Starting loss at step 0/);
  }
});

test("the strip fits the full stroke of its step-zero marker in the plot", async () => {
  const monitor = lossOver([[0, 3], [1, 2], [2, 4]], { better: "lower" });
  const strip = await draw(monitor, { chart: ["loss"] }, "strip");
  const svg = chartOf(strip);
  const start = svg.querySelector('circle[r="3"]');
  const radius = Number(start.getAttribute("r")) + Number(start.getAttribute("stroke-width")) / 2;
  assert.ok(Number(start.getAttribute("cx")) - radius >= 0, "the first marker clears the left edge");
  assert.ok(Number(start.getAttribute("cx")) + radius <= Number(svg.getAttribute("width")));
});

test("the pointer snaps to a landmark within reach along both axes, and to the nearest step beyond it", async () => {
  const series = Array.from({ length: 101 }, (_, step) => [step, 1 + Math.abs(step - 50) / 50]);
  const monitor = lossOver(series, { better: "lower" });
  const page = await draw(monitor, { chart: ["loss"] });
  const best = chartOf(page).querySelector('circle[r="3.5"]');
  const [cx, cy] = ["cx", "cy"].map((name) => Number(best.getAttribute(name)));
  assert.equal(cx.toFixed(1), pageX(50, 100).toFixed(1));
  const titleAt = async (dx, dy) => text(await hover(page, cx + dx, cy + dy)).match(/^(.*? )?step \d+/)[0];
  assert.equal(await titleAt(11, 11), "Lowest loss at step 50", "11 px off each way still reads the lowest");
  assert.equal(await titleAt(-11, -6), "Lowest loss at step 50");
  assert.equal(await titleAt(11, -60), "step 52", "far above it, the step under the pointer");
  assert.equal(await titleAt(20, 0), "step 53", "past the reach, the step under the pointer");
});

test("hovering the lowest loss reads the finite report after NaN at the same step", async () => {
  const monitor = lossOver([[0, 3], [1, "NaN"], [1, 2], [2, 4]], { better: "lower", smooth: 0.5 });
  monitor.series.validation = [[0, 5], [1, 4], [2, 3]];
  monitor.view.signals.validation = { label: "Validation", digits: 2 };
  const page = await draw(monitor, { chart: ["loss", "validation"] });
  const best = chartOf(page).querySelector('circle[r="3.5"]');
  const [cx, cy] = ["cx", "cy"].map((name) => Number(best.getAttribute(name)));
  assert.match(text(await hover(page, cx, cy)), /^Lowest loss at step 1Loss2\.00smoothed 2\.50Validation4\.00$/);
});

test("arrow keys stop at a chart's landmarks and say what each is", async () => {
  const monitor = lossOver([[0, 3], [1, 2], [2, 4]], { better: "lower" });
  for (const size of ["page", "strip"]) {
    const chart = await draw(monitor, { chart: ["loss"] }, size);
    const box = chartOf(chart).parentElement;
    const status = () => text(box.querySelector('[role="status"]'));
    await press("Home", box);
    assert.match(status(), /^Starting loss at step 0: Loss 3\.00$/);
    await press("ArrowRight", box);
    assert.match(status(), /^Lowest loss at step 1: Loss 2\.00$/);
    await press("ArrowRight", box);
    assert.match(status(), /^step 2: Loss 4\.00$/);
  }
});

test("a chart can hide its parent curves at every size", async () => {
  const monitor = fixture("finetune");
  for (const size of ["strip", "tile", "page"]) {
    const page = await draw(monitor, { chart: ["val_loss", "loss"], ghost: "false" }, size);
    assert.equal(chartOf(page).querySelectorAll("path[stroke-dasharray]").length, 0);
  }
});

test("a stage's chart spans its own stage's steps, and says the stage hasn't started before it has", async () => {
  const base = fixture("running");
  const stage = (name, of, started, ended) => ({ name, of, unit: null, priorMs: null, startedAt: started ? base.asOf - 60_000 : null, endedAt: ended ? base.asOf - 30_000 : null });
  const block = { chart: ["loss"], phase: "train" };
  const steps = (container) => labels(container).filter((label) => /^[\d,]+$/.test(label)).map((label) => Number(label.replaceAll(",", "")));

  const labelling = { ...base, phases: [stage("label", 5000, true), stage("train", null, false)], progress: { ...base.progress, phase: 0 }, x: { name: "step", max: 5000 }, series: { loss: [] } };
  const before = await draw(labelling, block);
  assert.equal(name(before), "The train stage hasn't started yet");
  assert.deepEqual(steps(before), [], "no steps to span: not the label stage's 5,000, nor a 0 to 1");
  const counted = await draw({ ...labelling, phases: [stage("label", 5000, true), stage("train", 272, false)] }, block);
  assert.equal(Math.max(...steps(counted)), 250, `a planned count spans train's own 272: ${steps(counted).join(" | ")}`);
  assert.equal(name(await draw(labelling, block, "page", { live: false })), "The train stage never started");

  const loss = Array.from({ length: 28 }, (_, index) => [index * 10, 2 - index / 20]);
  const evaluating = { ...base, phases: [stage("label", 5000, true, true), stage("train", 272, true, true), stage("eval", 204, true)], progress: { ...base.progress, phase: 2 }, x: { name: "step", max: 204 }, series: { loss } };
  const after = await draw(evaluating, block);
  assert.equal(Math.max(...steps(after)), 250, `train's 272 steps, not eval's 204: ${steps(after).join(" | ")}`);
  const end = [...chartOf(after).querySelectorAll("line[stroke-dasharray]")].find((line) => line.getAttribute("x1") === line.getAttribute("x2"));
  assert.equal(Number(end.getAttribute("x1")).toFixed(1), (38 + 714).toFixed(1), "the stage's end marks its own total");
});
