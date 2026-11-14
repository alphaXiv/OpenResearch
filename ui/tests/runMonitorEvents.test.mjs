import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { act, createElement } from "react";
import { QueryEvents } from "../src/queries/Events.tsx";
import { createWorkspace } from "../src/workspace.tsx";
import { render, serve, streams } from "./render.mjs";

const running = JSON.parse(readFileSync(new URL("./fixtures/runMonitor/running.json", import.meta.url), "utf8"));
/** Answers the oldest request a test holds open, once the page has made it. */
async function reply(replies, body) {
  await act(async () => {
    for (let ticks = 0; replies.length === 0; ticks++) {
      assert.ok(ticks < 50, "the page never asked");
      await new Promise((resolve) => setTimeout(resolve));
    }
    replies.shift()(Response.json(body));
  });
}
const emit = (type, data) => act(async () => streams[0].dispatchEvent(new MessageEvent(type, { data: JSON.stringify(data) })));
const event = (extra = {}) => ({
  runId: "r1", rev: running.rev + 1, generation: running.generation, asOf: running.asOf + 1000, seriesRev: running.seriesRev + 1, appendFrom: running.seriesRev,
  append: { loss: [[1250, 2.3]] }, latest: { loss: 2.3 }, counts: {}, progress: running.progress, estimate: running.estimate, cost: null, sentence: running.sentence, ...extra,
});

/** A page with the event stream open and run r1's monitor read once. */
async function page() {
  serve((path) => path === "/api/runs/r1/monitor" ? running : undefined);
  const workspace = createWorkspace("test");
  const query = workspace.queries.getRunMonitorQuery("r1");
  await render(createElement(QueryEvents), workspace);
  await act(async () => { await workspace.queryClient.fetchQuery(query); });
  return {
    read: () => workspace.queryClient.getQueryData(query.queryKey),
    invalidated: () => workspace.queryClient.getQueryCache().find({ queryKey: query.queryKey }).state.isInvalidated,
  };
}

test("a monitor event extends the chart in place and ignores a repeat", async () => {
  const { read, invalidated } = await page();
  await emit("run.monitor", event());
  assert.deepEqual(read().series.loss.at(-1), [1250, 2.3]);
  assert.equal(read().latest.loss, 2.3);
  await emit("run.monitor", event());
  assert.equal(read().series.loss.filter(([x]) => x === 1250).length, 1);
  assert.equal(invalidated(), false);
});

test("a monitor event carries the newest line of a run still setting up", async () => {
  const { read } = await page();
  const setup = { line: "Computing initial pair counts", stage: null, until: running.asOf + 60_000 };
  await emit("run.monitor", event({ append: {}, seriesRev: running.seriesRev, patch: { setup } }));
  assert.deepEqual(read().setup, setup);
});

test("rows a table gained extend it in place, and a whole table never arrives in an event", async () => {
  const { read, invalidated } = await page();
  const rows = (from, to) => Array.from({ length: to - from }, (_, i) => ({ task: `t${from + i}`, score: from + i }));
  await emit("run.monitor", event({ append: {}, appendRows: { eval: rows(0, 2) } }));
  await emit("run.monitor", event({ rev: running.rev + 2, seriesRev: running.seriesRev + 2, appendFrom: running.seriesRev + 1, append: {}, appendRows: { eval: rows(2, 3) } }));
  assert.deepEqual(read().rows.eval, rows(0, 3));
  assert.equal(invalidated(), false);
});

test("past the row cap a table slides: its oldest rows leave as new ones come, and nothing is read again", async () => {
  const { read, invalidated } = await page();
  const rows = (from, to) => Array.from({ length: to - from }, (_, i) => ({ task: `t${from + i}` }));
  let rev = running.rev;
  let seriesRev = running.seriesRev;
  const next = (extra) => emit("run.monitor", event({ rev: ++rev, seriesRev: ++seriesRev, appendFrom: seriesRev - 1, append: {}, ...extra }));
  await next({ appendRows: { eval: rows(0, 2048) } });
  for (let from = 2048; from < 3000; from += 8) await next({ dropRows: { eval: 8 }, appendRows: { eval: rows(from, from + 8) } });
  await next({ dropRows: { eval: 2 } });
  assert.deepEqual(read().rows.eval, rows(954, 3000));
  await next({ dropRows: { train: 3 } });
  assert.deepEqual(Object.keys(read().rows), ["eval"], "a table the page never held is not made up");
  assert.equal(invalidated(), false);
});

test("a gap, a reset or a new generation reads the run again instead of appending", async () => {
  for (const change of [{ appendFrom: running.seriesRev + 5 }, { reset: true }, { generation: "g2" }]) {
    const { read, invalidated } = await page();
    await emit("run.monitor", event(change));
    assert.ok(invalidated(), JSON.stringify(change));
    assert.equal(read().series.loss.length, running.series.loss.length);
    streams.length = 0;
  }
});

test("a run update refreshes the run above its chat's composer, never with an older copy", async () => {
  serve(() => undefined);
  const workspace = createWorkspace("test");
  const shown = workspace.queries.listChatShownRunsQuery();
  const run = { id: "r1", projectId: "p", status: "running", updatedAt: 10, monitor: null };
  workspace.queryClient.setQueryData(shown.queryKey, { source: "shown", runs: [{ sessionId: "s", run, experimentName: null }] });
  await render(createElement(QueryEvents), workspace);
  await emit("run.updated", { run: { ...run, updatedAt: 9, status: "done" } });
  assert.equal(workspace.queryClient.getQueryData(shown.queryKey).runs[0].run.status, "running");
  await emit("run.updated", { run: { ...run, updatedAt: 11, monitor: { tone: "flag" } } });
  assert.equal(workspace.queryClient.getQueryData(shown.queryKey).runs[0].run.monitor.tone, "flag");
});

test("a shown-runs reply older than a run update never undoes it", async () => {
  const run = { id: "r1", projectId: "p", status: "running", updatedAt: 10, monitor: null };
  const replies = [];
  serve((path) => path === "/api/chat/shown-runs" ? new Promise((resolve) => replies.push(resolve)) : undefined);
  const workspace = createWorkspace("test");
  const shown = workspace.queries.listChatShownRunsQuery();
  workspace.queryClient.setQueryData(shown.queryKey, { source: "shown", runs: [{ sessionId: "s", run, experimentName: null }] });
  await render(createElement(QueryEvents), workspace);
  const refreshed = workspace.queryClient.fetchQuery({ ...shown, staleTime: 0 });
  await emit("run.updated", { run: { ...run, status: "done", updatedAt: 11 } });
  await reply(replies, [{ sessionId: "s", run, experimentName: null }]);
  await reply(replies, [{ sessionId: "s", run: { ...run, status: "done", updatedAt: 11 }, experimentName: null }]);
  await act(() => refreshed);
  assert.equal(workspace.queryClient.getQueryData(shown.queryKey).runs[0].run.status, "done");
});

test("a monitor reply that raced an event keeps the event's points", async () => {
  const replies = [];
  serve((path) => path === "/api/runs/r1/monitor" ? new Promise((resolve) => replies.push(resolve)) : undefined);
  const workspace = createWorkspace("test");
  const query = workspace.queries.getRunMonitorQuery("r1");
  workspace.queryClient.setQueryData(query.queryKey, running);
  await render(createElement(QueryEvents), workspace);
  const refreshed = workspace.queryClient.fetchQuery({ ...query, staleTime: 0 });
  await emit("run.monitor", event());
  await reply(replies, running);
  const caughtUp = { ...running, rev: running.rev + 1, seriesRev: running.seriesRev + 1, series: { loss: [...running.series.loss, [1250, 2.3]] }, latest: { loss: 2.3 } };
  await reply(replies, caughtUp);
  await act(() => refreshed);
  const cached = workspace.queryClient.getQueryData(query.queryKey);
  assert.equal(cached.rev, running.rev + 1);
  assert.deepEqual(cached.series.loss.at(-1), [1250, 2.3]);
});

test("each shown run keeps its newer copy: a repair or a refetch never puts back an older one, and never keeps an older cached one", async () => {
  const run = { id: "r1", projectId: "p", status: "running", updatedAt: 10, monitor: null };
  const row = (extra) => [{ sessionId: "s", run: { ...run, ...extra }, experimentName: null }];
  const replies = [];
  serve((path) => path === "/api/chat/shown-runs" ? new Promise((resolve) => replies.push(resolve)) : undefined);
  const workspace = createWorkspace("test");
  const shown = workspace.queries.listChatShownRunsQuery();
  const cached = () => workspace.queryClient.getQueryData(shown.queryKey).runs[0].run;
  workspace.queryClient.setQueryData(shown.queryKey, { source: "shown", runs: row({}) });
  await render(createElement(QueryEvents), workspace);

  const repaired = workspace.queryClient.fetchQuery({ ...shown, staleTime: 0 });
  await emit("run.updated", { run: { ...run, updatedAt: 11 } });
  await reply(replies, row({}));
  await emit("run.updated", { run: { ...run, updatedAt: 12 } });
  await reply(replies, row({ status: "done", updatedAt: 13 }));
  await act(() => repaired);
  assert.deepEqual([cached().status, cached().updatedAt], ["done", 13], "the repair's newer row wins");

  workspace.queryClient.setQueryData(shown.queryKey, { source: "shown", runs: row({ status: "failed", updatedAt: 20 }) });
  const refetched = workspace.queryClient.fetchQuery({ ...shown, staleTime: 0 });
  await reply(replies, row({ status: "running", updatedAt: 19 }));
  await act(() => refetched);
  assert.deepEqual([cached().status, cached().updatedAt], ["failed", 20], "an older reply never replaces a newer cached row");
});
