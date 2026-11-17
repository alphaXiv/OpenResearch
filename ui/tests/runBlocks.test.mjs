import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { act, createElement } from "react";
import { Block } from "../src/components/run/blocks/Block.tsx";
import { m } from "../src/paraglide/messages.js";
import { render, serveApp, settle } from "./render.mjs";

const fixture = (name) => JSON.parse(readFileSync(new URL(`./fixtures/runMonitor/${name}.json`, import.meta.url), "utf8"));
const blockOf = (monitor, kind) => monitor.view.blocks.find((block) => kind in block);
// A page's table virtualizes against its box, which happy-dom never lays out.
for (const size of ["clientHeight", "clientWidth", "offsetHeight", "offsetWidth"]) {
  Object.defineProperty(HTMLElement.prototype, size, { configurable: true, get: () => 800 });
}

/** What a reader sees, without the marks that keep numbers and names whole in either direction. */
const text = (element) => element.textContent.replace(/[⁦-⁩]/g, "");
const props = (monitor, block, size, live) => ({ monitor, block, size, projectId: "p", tone: live ? "running" : "done", live });

async function draw(monitor, block, size, { live = true } = {}) {
  serveApp();
  const view = await render(createElement(Block, props(monitor, block, size, live)));
  await settle();
  return view.container;
}

/** Draws a block that can be drawn again with new data, as events change the run's monitor. */
async function live(monitor, block, size = "page") {
  serveApp();
  const view = await render(createElement(Block, props(monitor, block, size, true)));
  await settle();
  return { container: view.container, update: async (next) => { await view.rerender(createElement(Block, props(next, block, size, true))); await settle(); } };
}

const button = (container, words) => [...container.querySelectorAll("button")].find((element) => text(element) === words);
/** Whether a better-or-worse mark points down, the way the number moved. */
const pointsDown = (element) => element.querySelector("svg path").getAttribute("d").includes("L4 7.5");

test("a chart on a page has axes, its latest value at the end and the parent dashed behind; in a strip, the lines alone", async () => {
  const monitor = fixture("finetune");
  const block = blockOf(monitor, "chart");
  const page = await draw(monitor, block, "page");
  const chart = page.querySelector('svg[role="img"]');
  assert.match(chart.getAttribute("aria-label"), /^Val loss [\d.]+, [\d.]+ better than parent; Train loss [\d.]+$/);
  assert.equal(text(page.querySelector("h3")), "Val loss and train loss");
  assert.ok([...chart.querySelectorAll("text")].some((label) => label.textContent === "step"), "the x axis is named");
  assert.ok(chart.querySelector("path[stroke-dasharray]"), "the parent is dashed");
  assert.match(text(page), /Parent.*Val loss.*Train loss/, "a legend names each line");
  const evals = monitor.series.val_loss.filter(([, y]) => y !== null).length;
  assert.equal(chart.querySelectorAll("circle").length, evals, "each eval is marked; train loss's points are too close together to");

  const strip = await draw(monitor, block, "strip");
  assert.equal(strip.querySelector("h3"), null);
  assert.equal(strip.querySelectorAll("svg text").length, 0, "no axes in a strip");
  assert.equal(strip.querySelector('svg[role="img"]').getAttribute("aria-label"), chart.getAttribute("aria-label"));
});

test("a chart of a few points marks each one, and ends each line with its latest value and the parent's with its name", async () => {
  const monitor = fixture("selfplay");
  const page = await draw(monitor, blockOf(monitor, "chart"), "page");
  const chart = page.querySelector('svg[role="img"]');
  const points = monitor.series.elo.filter(([, y]) => y !== null);
  assert.equal(chart.querySelectorAll("circle").length, points.length);
  const labels = [...chart.querySelectorAll("text")].map((label) => label.textContent.replace(/[⁦-⁩]/g, ""));
  assert.ok(labels.includes(`+${Math.round(points.at(-1)[1])}`), labels.join(" "));
  assert.ok(labels.includes(m.run_chart_parent_end()));
});

test("a chart of a very small signal labels its ticks by their size, never as a row of zeros", async () => {
  const base = fixture("finetune");
  const points = Array.from({ length: 12 }, (_, i) => [i * 100, 5e-5 - i * 3e-6]);
  const monitor = { ...base, view: { ...base.view, signals: { lr: { label: "lr" } }, headline: ["lr"], blocks: [{ chart: ["lr"], x: "step" }] }, series: { lr: points }, latest: { lr: points.at(-1)[1] }, parent: null, versus: { step: {}, final: {} } };
  const page = await draw(monitor, monitor.view.blocks[0], "page");
  const labels = [...page.querySelectorAll('svg[role="img"] text')].map((label) => text(label));
  const ticks = labels.filter((label) => /e-\d/.test(label));
  assert.ok(ticks.length >= 2, `ticks read their size: ${labels.join(" | ")}`);
  assert.ok(new Set(ticks).size === ticks.length, `no two ticks read alike: ${ticks.join(" | ")}`);
});

test("a chart with nothing to draw says so instead of an empty axis, in words for whether the run is still going", async () => {
  const monitor = { ...fixture("finetune"), series: {}, parent: null };
  const page = await draw(monitor, blockOf(monitor, "chart"), "page");
  assert.equal(page.querySelector('svg[role="img"]').getAttribute("aria-label"), m.run_block_no_points());
  assert.ok(text(page).includes(m.run_block_no_points()));
  const ended = await draw(monitor, blockOf(monitor, "chart"), "page", { live: false });
  assert.ok(text(ended).includes(m.run_block_no_points_ended()));
  assert.ok(!text(ended).includes(m.run_block_no_points()));
});

test("stats show each latest value with its course, and say in words whether it is better than the parent, on a page only", async () => {
  const monitor = fixture("selfplay");
  const block = blockOf(monitor, "stats");
  const page = await draw(monitor, block, "page");
  assert.deepEqual([...page.querySelectorAll("dt")].map((term) => term.textContent), ["Elo gained", "Win vs previous", "Round time"]);
  const compared = block.stats.filter((name) => monitor.versus.step[name]);
  assert.equal(page.querySelectorAll("dd").length, block.stats.length + compared.length, "a stat with no parent to compare with says nothing more");
  const elo = page.querySelector("dl > div");
  assert.match(text(elo), /Elo gained\+\d+.*\d+ better than the parent at step 14/);
  assert.equal(pointsDown(elo), monitor.versus.step.elo.d < 0);
  assert.match(text(page), /Win vs previous[\d.]+%/);

  const worse = { ...monitor, versus: { ...monitor.versus, step: { ...monitor.versus.step, elo: { d: -40, good: false, flat: false, mag: 40, parent: 936 } } } };
  const behind = (await draw(worse, block, "page")).querySelector("dl > div");
  assert.match(text(behind), /40 worse than where the parent ended/, "a comparison at no step is with the parent's final value");
  assert.ok(pointsDown(behind));

  const versus = { step: monitor.versus.step, final: { elo: { d: -12, good: false, flat: false, mag: 12, parent: 908 } } };
  const asked = (await draw({ ...monitor, versus }, { ...block, vs: "final" }, "page")).querySelector("dl > div");
  assert.match(text(asked), /12 worse than where the parent ended/, "the block's own way wins over the signal's");

  assert.equal((await draw(monitor, block, "strip")).textContent, "");
});

test("a stat asked for its best shows the best the server names, and the latest, labelled so, when it names none", async () => {
  const monitor = fixture("ddpm");
  const block = blockOf(monitor, "stats");
  const best = await draw({ ...monitor, best: { fid: 12.34 } }, block, "page");
  assert.equal(text(best.querySelector("dl > div")).replace(/\s/g, ""), "BestFID12.3");
  const latest = await draw({ ...monitor, best: undefined }, block, "page");
  assert.equal(latest.querySelector("dt").textContent, "FID");
  assert.ok(text(latest.querySelector("dd")).startsWith(String(Math.round(monitor.latest.fid * 10) / 10)));

  const finetune = fixture("finetune");
  finetune.view.signals.val_loss.pick = "best";
  const named = await draw({ ...finetune, best: { val_loss: 1.5 } }, blockOf(finetune, "stats"), "page");
  assert.equal(named.querySelector("dt").textContent, "Best val loss");
});

test("a page's table shows its newest rows newest first, and every row on request in a box a keyboard reaches", async () => {
  const monitor = fixture("selfplay");
  const block = blockOf(monitor, "table");
  const rounds = monitor.rows.rounds.map((row) => row.round);
  const page = await draw(monitor, block, "page");
  assert.equal(text(page.querySelector("h3")), "Latest rounds");
  assert.deepEqual([...page.querySelectorAll('th[scope="col"]')].map((head) => head.textContent), ["Round", "Win vs previous", "Elo gained", "Game length", "Round time"]);
  assert.deepEqual([...page.querySelectorAll("tbody tr")].map((row) => Number(text(row.cells[0]))), rounds.slice(-block.last).reverse());
  assert.equal(page.querySelector('[role="region"]'), null, "no box until every row is asked for");

  await act(async () => button(page, `Show all ${rounds.length} rows`).click());
  const box = page.querySelector('[role="region"]');
  assert.equal(box.getAttribute("aria-label").replace(/[⁦-⁩]/g, ""), "Latest rounds, every row");
  assert.equal(box.tabIndex, 0);
  assert.equal(box.querySelector("table").getAttribute("aria-rowcount"), String(rounds.length + 1));
  assert.deepEqual([...box.querySelectorAll("tbody tr:not([aria-hidden])")].slice(0, 3).map((row) => Number(text(row.cells[0]))), [...rounds].reverse().slice(0, 3));
  await act(async () => button(page, `Show the newest ${block.last}`).click());
  assert.equal(page.querySelectorAll("tbody tr").length, block.last);

  const unbounded = await draw(monitor, { table: "rounds", columns: ["round", "elo"] }, "page");
  assert.equal(unbounded.querySelectorAll("tbody tr").length, Math.min(10, rounds.length), "ten rows when the view gives no number");
});

test("every row of a long table draws only the rows in sight", async () => {
  const monitor = { ...fixture("selfplay"), rows: { rounds: Array.from({ length: 2048 }, (_, round) => ({ round, elo: round })) } };
  const page = await draw(monitor, { table: "rounds", columns: ["round", "elo"] }, "page");
  await act(async () => button(page, "Show all 2,048 rows").click());
  const table = page.querySelector('[role="region"] table');
  assert.equal(table.getAttribute("aria-rowcount"), "2049");
  const drawn = table.querySelectorAll("tbody tr:not([aria-hidden])").length;
  assert.ok(drawn > 0 && drawn < 100, `${drawn} rows drawn`);
});

test("a strip's table shows the newest two rows, the ones that fit, and its first three columns", async () => {
  const monitor = fixture("selfplay");
  const strip = await draw(monitor, blockOf(monitor, "table"), "strip");
  assert.deepEqual([...strip.querySelectorAll("tbody tr")].map((row) => Number(text(row.cells[0]))), monitor.rows.rounds.slice(-2).map((row) => row.round).reverse());
  assert.equal(strip.querySelectorAll("thead th").length, 3);
});

test("a table with no rows says so, with a yet only while the run is going", async () => {
  const monitor = { ...fixture("selfplay"), rows: {} };
  const block = blockOf(monitor, "table");
  assert.equal(text(await draw(monitor, block, "page")), `Latest rounds${m.run_block_no_rows()}`);
  assert.equal(text(await draw(monitor, block, "page", { live: false })), `Latest rounds${m.run_block_no_rows_ended()}`);
});

test("rows arriving while the reader is away from the newest are held and counted, and the count returns to them", async () => {
  const monitor = fixture("selfplay");
  const row = (round) => ({ round, elo: round * 10 });
  const rows = Array.from({ length: 40 }, (_, i) => row(i));
  const block = { table: "rounds", columns: ["round", "elo"] };
  const view = await live({ ...monitor, rows: { rounds: rows } }, block);
  await act(async () => button(view.container, "Show all 40 rows").click());
  const box = view.container.querySelector('[role="region"]');
  const shown = () => [...box.querySelectorAll("tbody tr:not([aria-hidden])")].map((tr) => text(tr.cells[0]));
  box.scrollTop = 300;
  await act(async () => box.dispatchEvent(new Event("scroll")));
  const before = shown();

  const arrived = [...rows, row(40), row(41), row(42)];
  await view.update({ ...monitor, rows: { rounds: arrived } });
  assert.deepEqual(shown(), before, "nothing in the box moved");
  assert.equal(box.scrollTop, 300);
  assert.ok(button(view.container, "3 new rows"));

  // Past the cap the oldest rows leave as new ones come; the reader still sees what they saw.
  const slid = [...arrived.slice(2), row(43), { ...row(44) }];
  await view.update({ ...monitor, rows: { rounds: slid.map((r) => ({ ...r })) } });
  assert.deepEqual(shown(), before);
  assert.ok(button(view.container, "5 new rows"), "rows read again as new objects are still counted once");

  await act(async () => button(view.container, "5 new rows").click());
  const fresh = view.container.querySelector('[role="region"]');
  assert.equal(fresh.scrollTop, 0);
  assert.equal(document.activeElement, fresh, "the keyboard stays in the box");
  assert.equal(text(fresh.querySelector("tbody tr:not([aria-hidden])").cells[0]), "44");
  assert.equal(button(view.container, "5 new rows"), undefined);
});

test("a table against its parent lists every row the parent has, unfilled until reached, and says better or worse in words", async () => {
  const monitor = fixture("pipeline");
  const block = blockOf(monitor, "table");
  monitor.rows.eval = [{ task: "sst2", score: 93.1 }, { task: "mnli", score: 83.0 }];
  const page = await draw(monitor, block, "page");
  const rows = [...page.querySelectorAll("tbody tr")];
  const cells = rows.map((row) => [...row.cells].map(text));
  assert.deepEqual(cells.map((row) => row.slice(0, 3)), [["sst2", "93.1", "92.4"], ["mnli", "83.0", "83.9"], ["qqp", "—", "88.3"], ["squad", "—", "80.2"], ["rte", "—", "67.5"]]);
  assert.equal(cells[0][3], "0.7 better");
  assert.ok(!pointsDown(rows[0].cells[3]));
  assert.equal(cells[1][3], "0.9 worse");
  assert.ok(pointsDown(rows[1].cells[3]));
  assert.equal(cells[2][3], "");
  assert.equal(text(page.querySelector("h3")), "Eval");
});

const outcomeView = JSON.parse(readFileSync(new URL("../../tests/contracts/run-view-outcomes.json", import.meta.url), "utf8")).tree;

test("bars use the selected column's classified counts and leave a missing rate at zero classified", async () => {
  const block = outcomeView.blocks.find((entry) => "bars" in entry);
  const monitor = {
    ...fixture("running"),
    view: outcomeView,
    groups: {
      items: {
        category: [{ key: "algebra", ok: 3, n: 4 }, { key: "proofs", ok: 0, n: 0 }],
        split: [{ key: "test", ok: 1, n: 1 }],
      },
    },
  };
  const view = await live(monitor, block);
  assert.equal(text(view.container.querySelector("h3")), "Accuracy by category");
  assert.match(text(view.container), /algebra75%3 of 4proofs—0 of 0/);
  assert.equal(view.container.querySelectorAll("i").length, 1);
  const next = { ...monitor, groups: { items: { category: [{ key: "algebra", ok: 3, n: 5 }, { key: "proofs", ok: 0, n: 0 }] } } };
  await view.update(next);
  assert.match(text(view.container), /algebra60%3 of 5/);
  const strip = await draw(monitor, block, "strip");
  assert.ok(!text(strip).includes("3 of 4"));
});

test("bars with nothing counted say so while running and after finishing", async () => {
  const monitor = { ...fixture("running"), view: outcomeView, groups: {} };
  const block = outcomeView.blocks.find((entry) => "bars" in entry);
  assert.ok(text(await draw(monitor, block, "page")).endsWith(m.run_bars_none({ key: "category" })));
  assert.ok(text(await draw(monitor, block, "page", { live: false })).endsWith(m.run_bars_none_ended({ key: "category" })));
});

test("cells draw and count the newest 256 retained outcomes, with distinguishable marks", async () => {
  const block = outcomeView.blocks.find((entry) => "cells" in entry);
  const rows = [
    { outcome: "wrong" },
    ...Array.from({ length: 252 }, () => ({ outcome: "correct" })),
    { outcome: "wrong" }, { outcome: "error" }, { outcome: "unexpected" }, {},
  ];
  const monitor = { ...fixture("running"), view: outcomeView, rows: { items: rows } };
  const view = await live({ ...monitor, rows: {} }, block);
  await view.update(monitor);
  const image = () => view.container.querySelector('svg[role="img"]');
  const accessibleName = () => image().getAttribute("aria-label").replace(/[⁦-⁩]/g, "");
  assert.match(accessibleName(), /256 shown.*252 correct.*1 wrong.*1 error.*2 not known/);
  assert.equal(image().children.length, 256);
  assert.ok(Number(image().getAttribute("width")) > 12, "items wrap into the measured box after the first report");
  assert.deepEqual([...image().children].slice(-5).map((mark) => [mark.localName, mark.getAttribute("fill")]), [
    ["circle", "var(--mark-blue)"],
    ["rect", "var(--subtext)"],
    ["path", "var(--mark-amber)"],
    ["rect", "none"],
    ["rect", "none"],
  ]);
  await view.update({ ...monitor, rows: { items: [...rows.slice(1), { outcome: "error" }] } });
  assert.match(accessibleName(), /256 shown.*251 correct.*1 wrong.*2 errors.*2 not known/);
});

test("cells with no items say so instead of an empty picture", async () => {
  const monitor = { ...fixture("running"), view: outcomeView, rows: {} };
  const block = outcomeView.blocks.find((entry) => "cells" in entry);
  const page = await draw(monitor, block, "page");
  assert.equal(page.querySelector("svg"), null);
  assert.ok(text(page).endsWith(m.run_cells_none()));
  assert.ok(text(await draw(monitor, block, "page", { live: false })).endsWith(m.run_cells_none_ended()));
});

test("media shows the newest files under their names, each at its version, and says when none match or one can't be opened", async () => {
  const monitor = fixture("ddpm");
  const block = blockOf(monitor, "media");
  const files = monitor.media[block.media];
  const page = await draw(monitor, block, "page");
  const images = [...page.querySelectorAll("img")];
  assert.deepEqual(images.map((image) => image.alt), files.slice(-block.last).reverse().map((file) => file.caption));
  const url = new URL(images[0].src);
  assert.equal(url.pathname, "/api/projects/p/files/file");
  assert.equal(url.searchParams.get("path"), "samples/step-80k.png");
  assert.equal(url.searchParams.get("v"), String(files.at(-1).at), "a file written again in place is read again");

  await act(async () => images[0].dispatchEvent(new Event("error")));
  assert.ok(text(page).includes("Couldn't open step-80k.png"));

  const strip = await draw(monitor, block, "strip");
  assert.equal(strip.querySelectorAll("img").length, 3);

  const none = { ...monitor, media: {} };
  assert.ok(text(await draw(none, block, "page")).includes("No files match artifacts/samples/*.png yet"));
  assert.ok(text(await draw(none, block, "page", { live: false })).includes("No files matched artifacts/samples/*.png"));
});

test("text shows a dash until its latest value arrives and keeps the author's bold treatment", async () => {
  const monitor = { ...fixture("running"), view: outcomeView, latest: {} };
  const block = outcomeView.blocks.find((entry) => "text" in entry);
  const view = await live(monitor, block);
  assert.equal(text(view.container), "Loss is —. Lower is better.");
  assert.equal(text(view.container.querySelector("strong")), "—");
  await view.update({ ...monitor, latest: { val_loss: 2.3 } });
  assert.equal(text(view.container), "Loss is 2.30. Lower is better.");
  assert.equal(text(view.container.querySelector("strong")), "2.30");
  assert.equal((await draw(monitor, block, "strip")).textContent, "");
});

test("a block, an attribute or a value from a newer server is skipped and breaks nothing", async (t) => {
  const failed = t.mock.method(console, "error", () => {});
  const monitor = fixture("finetune");
  for (const size of ["page", "strip"]) assert.equal((await draw(monitor, { heatmap: "loss", title: "Loss by layer" }, size)).textContent, "");
  const banded = await draw(monitor, { chart: ["loss"], band: "p90" }, "page");
  assert.ok(banded.querySelector('svg[role="img"]').getAttribute("aria-label").startsWith("Train loss"));
  const media = fixture("ddpm");
  const huge = await draw(media, { media: "artifacts/samples/*.png", size: "xxl" }, "page");
  assert.ok(huge.querySelector("ul").style.gridTemplateColumns.includes("5.5rem"));
  assert.equal(failed.mock.callCount(), 0);
});

test("a block that can't be drawn says so in its place in plain words, and tries again at the next revision", async (t) => {
  const failed = t.mock.method(console, "error", () => {});
  const monitor = fixture("selfplay");
  const block = { table: "rounds", columns: ["round", "elo"] };
  const view = await live({ ...monitor, rows: { rounds: "not rows" } }, block);
  assert.equal(text(view.container), "Couldn't draw Latest rounds");
  assert.equal(view.container.querySelector('[role="alert"]'), null);
  assert.ok(failed.mock.callCount() > 0, "the cause goes to the console");

  await view.update({ ...monitor, rev: monitor.rev + 1 });
  assert.ok(view.container.querySelector("table"));
});
