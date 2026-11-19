import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { act, createElement } from "react";
import { ChatImageScope, Md } from "../src/components/Md.tsx";
import { m } from "../src/paraglide/messages.js";
import { createWorkspace } from "../src/workspace.tsx";
import { find, press, render, serveApp, settle } from "./render.mjs";

const fixture = JSON.parse(readFileSync(new URL("./fixtures/runMonitor/running.json", import.meta.url), "utf8"));

async function message({ monitor = fixture, visible = true, figure = "figure", projectId = "p", failure, id = "r1", edit } = {}) {
  const calls = [];
  let refused = !!failure;
  const run = { id: "r1", projectId, experimentId: "e1", status: "running", contact: null, monitor: { rev: monitor.rev, generation: monitor.generation } };
  serveApp({
    [`GET /api/projects/${projectId}/runs`]: () => failure === "runs" && refused ? Response.json({ error: "Could not read runs" }, { status: 500 }) : ({ runs: visible ? [run] : [] }),
    "GET /api/runs/r1/monitor": () => { calls.push("monitor"); return failure === "monitor" && refused ? Response.json({ error: "Could not read monitor" }, { status: 500 }) : monitor; },
    ...(edit && { "PATCH /api/runs/r1/view/chart": (init) => {
      const next = edit(JSON.parse(init.body), monitor);
      if (next instanceof Response) return next;
      monitor = next;
      return {};
    } }),
  });
  const workspace = createWorkspace("test");
  const opened = [];
  const view = await render(createElement(ChatImageScope, { projectId }, createElement(Md, {
    text: `Loss fell. <run id="${id}" label="Training result" ${figure}/>`,
    onOpenRun: (...args) => opened.push(args),
  })), workspace);
  await settle();
  return { ...view, calls, opened, recover: () => { refused = false; } };
}

test("a figure citation draws the page chart and keeps the chip's open gesture", async () => {
  const { container, opened } = await message();
  assert.ok(container.querySelector('svg[role="img"][aria-label*="Loss"]'));
  assert.match(container.querySelector("h3").textContent, /Loss/);
  const chip = container.querySelector("button.run-chip");
  assert.equal(chip.textContent, "Training result");
  await act(() => chip.click());
  assert.equal(opened[0][0], "r1");
  assert.ok(!container.querySelector("p div"), "block content does not nest in a paragraph");
});

test("the chart menu opens with mouse or keyboard and redraws after a saved axis change", async () => {
  const markup = '<view><chart y="loss"/></view>';
  const monitor = { ...fixture, viewFrom: { ...fixture.viewFrom, markup, isDefault: false } };
  const { container } = await message({ monitor, edit: (body, current) => {
    assert.deepEqual(body, { markup, isDefault: false, block: 0, edit: { kind: "y", log: true } });
    return { ...current, rev: current.rev + 1, view: { ...current.view, blocks: [{ ...current.view.blocks[0], yScale: "log" }] } };
  } });
  const trigger = container.querySelector('[aria-haspopup="menu"]');
  await act(() => trigger.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, button: 2 })));
  assert.ok(find('[role="menu"]'));
  await press("Escape");
  await settle();
  assert.equal(find('[role="menu"]'), null);
  await act(() => trigger.focus());
  await press("F10", trigger, { shiftKey: true });
  const y = find('[role="menu"] [role="group"]', m.run_chart_y_axis());
  const log = y.querySelectorAll('[role="menuitemradio"]')[1];
  assert.equal(log.getAttribute("aria-checked"), "false");
  await act(() => log.click());
  await settle();
  assert.match(container.querySelector('svg[role="img"]').textContent, /log/);
  await press("ContextMenu", trigger);
  assert.equal(find('[role="menu"] [role="group"]', m.run_chart_y_axis()).querySelectorAll('[role="menuitemradio"]')[1].getAttribute("aria-checked"), "true");
});

test("a refused chart edit keeps the prior scale and explains the failure", async () => {
  const reason = "The experiment's view changed. Choose the chart setting again.";
  const { container } = await message({
    edit: () => Response.json({ error: reason }, { status: 409 }),
  });
  const trigger = container.querySelector('[aria-haspopup="menu"]');
  await press("ContextMenu", trigger);
  const y = find('[role="menu"] [role="group"]', m.run_chart_y_axis());
  const [linear, log] = y.querySelectorAll('[role="menuitemradio"]');
  await act(() => log.click());
  await settle();
  assert.equal(linear.getAttribute("aria-checked"), "true");
  assert.ok(find('[role="alert"]', reason));
  assert.doesNotMatch(container.querySelector('svg[role="img"]').textContent, /log/);
});

test("a plotted zero disables log on its axis and a table chart disables smoothing", async () => {
  const block = { chart: ["loss"], from: "training", x: "step" };
  const { container } = await message({
    monitor: {
      ...fixture,
      view: { ...fixture.view, blocks: [block] },
      rows: { training: [{ step: 0, loss: 2.33 }, { step: 1, loss: 2.31 }] },
    },
  });
  const trigger = container.querySelector('[aria-haspopup="menu"]');
  await press("ContextMenu", trigger);
  const x = find('[role="menu"] [role="group"]', m.run_chart_x_axis());
  const log = x.querySelectorAll('[role="menuitemradio"]')[1];
  assert.equal(log.getAttribute("aria-disabled"), "true");
  assert.match(x.textContent, /above zero/);
  assert.equal(find('[role="menuitem"]', m.run_chart_smoothing()).getAttribute("aria-disabled"), "true");
});

test("a table citation tells the reader its most recent reports come first", async () => {
  const block = { table: "eval", columns: ["task", "score"], title: "Eval" };
  const monitor = {
    ...fixture,
    view: { ...fixture.view, blocks: [block] },
    rows: { eval: [{ task: "arc", score: 0.7 }, { task: "piqa", score: 0.6 }, { task: "boolq", score: 0.5 }] },
  };
  const { container } = await message({ monitor });
  const table = container.querySelector("table");
  assert.equal(table.caption?.textContent, m.run_table_newest_first());
  assert.deepEqual([...table.querySelectorAll("tbody tr")].map((row) => row.cells[0].textContent.replace(/[⁦-⁩]/g, "")), ["boolq", "piqa", "arc"]);
});

test("stats are the first block and update with the shared live query", async () => {
  const monitor = { ...fixture, view: { ...fixture.view, blocks: [{ stats: ["loss"] }, ...fixture.view.blocks] } };
  const { container, workspace } = await message({ monitor });
  assert.ok(container.querySelector("dl"));
  assert.ok(!container.querySelector('svg[role="img"]'), "only the first block is cited");
  const next = { ...monitor, rev: monitor.rev + 1, latest: { loss: 1.11 } };
  await act(async () => workspace.queryClient.setQueryData(workspace.queries.getRunMonitorQuery("r1").queryKey, next));
  await settle();
  assert.match(container.querySelector("dl").textContent, /1\.11/);
});

for (const [name, options] of [
  ["no view", { monitor: { ...fixture, view: { ...fixture.view, blocks: [] } } }],
  ["no data", { monitor: { ...fixture, series: {}, latest: {} } }],
  ["unseen run", { visible: false }],
  ["ordinary chip", { figure: "" }],
]) test(`${name} leaves only the chip`, async () => {
  const { container, calls } = await message(options);
  assert.ok(container.querySelector("button.run-chip"));
  assert.ok(!container.querySelector('svg[role="img"], dl'), "no figure is drawn");
  if (options.visible === false || options.figure === "") assert.deepEqual(calls, []);
});

test("quoted figure attributes use the message project", async () => {
  const { container } = await message({ figure: 'figure="true"', projectId: "remote-project" });
  assert.ok(container.querySelector('svg[role="img"]'));
});

for (const failure of ["runs", "monitor"]) test(`${failure} failure stays visible and can be retried`, async () => {
  const { container, recover } = await message({ failure });
  const failureNotice = () => find('[role="status"]', m.run_monitor_load_failed());
  assert.ok(failureNotice(), "the failure is announced");
  assert.ok(!container.querySelector('svg[role="img"]'), "the chart waits for its data");
  recover();
  const retry = [...container.querySelectorAll("button")].find((button) => !button.classList.contains("run-chip"));
  await act(() => retry.click());
  await settle();
  assert.ok(!failureNotice(), "the failure notice clears after retry");
  assert.ok(container.querySelector('svg[role="img"]'));
});

test("a unique short run id resolves like its chip", async () => {
  const { container } = await message({ id: "r" });
  assert.ok(container.querySelector('svg[role="img"]'));
});
