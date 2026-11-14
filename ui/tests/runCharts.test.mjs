import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { act, createElement } from "react";
import { Block } from "../src/components/run/blocks/Block.tsx";
import { m } from "../src/paraglide/messages.js";
import { runModel } from "../src/runMonitor/model.ts";
import { render, serveApp, settle } from "./render.mjs";

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
for (const size of ["clientHeight", "clientWidth", "offsetHeight", "offsetWidth"]) {
  Object.defineProperty(HTMLElement.prototype, size, { configurable: true, get: () => 800 });
}

/** What a reader sees, without the marks that keep numbers and names whole in either direction. */
const text = (element) => element.textContent.replace(/[⁦-⁩]/g, "");
const chartOf = (container) => container.querySelector('svg[role="img"]');
const labels = (container) => [...chartOf(container).querySelectorAll("text")].map(text);
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
  return box.querySelector('[aria-hidden="true"]');
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

  const readout = await point(page, 38 + 714 / 6 / 2);
  assert.match(text(readout), /^algebraParent81\.1%Accuracy84\.2%$/);

  const strip = await draw(monitor, block, "strip");
  assert.equal(chartOf(strip).querySelectorAll("rect").length, 6);
  assert.equal(strip.querySelectorAll("svg text").length, 0);
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
  assert.equal(nothing.querySelector("p"), null, "with nothing drawn, the reason is the chart's own words");
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

  const headline = { ...monitor, view: { ...monitor.view, headline: ["valid"] } };
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
