import assert from "node:assert/strict";
import test from "node:test";
import { Activity, act, createElement } from "react";
import { createWorkspace } from "../src/workspace.tsx";
import { render, streams } from "./render.mjs";

const emit = (source, type, data) => act(async () => source.dispatchEvent(new MessageEvent(type, { data: JSON.stringify(data) })));

test("run logs reach entity observers once and unsubscribe on unmount", async () => {
  const workspace = createWorkspace("local");
  const logs = [];
  function Observer() {
    workspace.events.useOrxEvents({ onRun() {}, onRunLog: (event) => logs.push(event) });
    return null;
  }
  const view = await render(createElement(Observer), workspace);
  const event = { runId: "fresh-run", dataBase64: "bG9n", offset: 0 };
  workspace.events.emitEntity.onRunLog(event);
  assert.deepEqual(logs, [event]);

  await view.unmount();
  workspace.events.emitEntity.onRunLog(event);
  assert.equal(logs.length, 1);
});

test("returning to a retained workspace repairs events missed while its stream was closed", async () => {
  const workspace = createWorkspace("alpha", "/_orx/workspaces/alpha");
  let repairs = 0, authProbes = 0;
  workspace.events.onHarnessAuth(() => authProbes++);
  function Stream() {
    workspace.events.useOrxEventStream({ onRun() {}, onReconnect: () => repairs++ });
    return null;
  }
  const page = (mode) => createElement(Activity, { mode }, createElement(Stream));
  const { rerender } = await render(page("visible"), workspace);
  await act(async () => streams[0].onopen());
  assert.equal(repairs, 0);
  await rerender(page("hidden"));
  assert.equal(streams[0].readyState, 2);

  await rerender(page("visible"));
  assert.equal(new URL(streams[1].url, location.href).pathname, "/_orx/workspaces/alpha/api/events");
  await act(async () => streams[1].onopen());
  assert.equal(repairs, 1);
  assert.equal(authProbes, 0);
});

test("the stream fans engine log growth to its run and host updates to its own subscribers", async () => {
  const [local, remote] = [createWorkspace("local"), createWorkspace("remote", "/_orx/workspaces/remote")];
  await render(createElement(function Stream() { local.events.useOrxEventStream({ onRun() {} }); return null; }), local);
  const [source] = streams;
  const remoteHosts = [];
  remote.events.onEngineHost((host) => remoteHosts.push(host));
  local.events.requestHostConnect("gpu-box");
  assert.equal(local.events.takeHostConnect(), "gpu-box");
  assert.equal(remote.events.pendingHostConnect(), null);

  const grown = [];
  const stopLog = local.events.onRunLog("engine-run", (event) => grown.push(event));
  const growth = { runId: "engine-run", dataBase64: "", offset: 4096 };
  await emit(source, "run.log", growth);
  await emit(source, "run.log", { ...growth, runId: "other-run" });
  assert.deepEqual(grown, [growth]);
  stopLog();

  const hosts = [];
  const stopHosts = local.events.onEngineHost((host) => hosts.push(host));
  const host = { alias: "gpu-box", status: "backoff", class: "key", sinceMs: 1, nextRetryMs: 2, cause: "timed out" };
  await emit(source, "host.updated", host);
  await emit(source, "host.updated", { status: "live" });
  assert.deepEqual(hosts, [host]);
  assert.deepEqual(remoteHosts, []);
  stopHosts();
  await emit(source, "host.updated", host);
  assert.equal(hosts.length, 1);
});
