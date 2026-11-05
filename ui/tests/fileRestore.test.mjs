import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement, useState } from "react";
import { useLatexCompile } from "../src/useLatexCompile.ts";
import { useOverleafSync } from "../src/useOverleafSync.ts";
import { render, serve, settle } from "./render.mjs";

/** Mounts both file-viewer hooks over a backend that counts compiles, syncs and status polls. */
async function viewer(t, restored) {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const calls = { compile: 0, sync: 0, status: 0, activated: 0 };
  const link = { projectId: "paper", url: "https://overleaf.com/project/paper" };
  const routes = {
    "GET /api/latex/engine": () => ({ engine: "tectonic", hint: null, installCommand: null }),
    "POST /api/projects/project/file/latex": () => { calls.compile++; return { ok: true, pdfPath: "paper.pdf", hadErrors: false, note: null }; },
    "GET /api/projects/project/file/overleaf": () => ({ hasToken: true, hasSession: false, link }),
    "POST /api/projects/project/file/overleaf": () => ({ hasToken: true, link }),
    "GET /api/projects/project/file/overleaf/status": () => { calls.status++; return { remoteChanged: true }; },
    "POST /api/projects/project/file/overleaf/sync": () => { calls.sync++; return { pulled: [], pushed: [], conflicts: [] }; },
    "POST /api/projects/project/file/overleaf/live": () => ({ key: "k", status: null }),
    "DELETE /api/projects/project/file/overleaf/live": () => ({ key: "k", status: null }),
    "POST /api/overleaf/token": () => ({ hasToken: true }),
  };
  serve((path, init) => routes[`${init.method ?? "GET"} ${path.split("?")[0]}`]?.());

  const hooks = {};
  function Viewer() {
    const [autoRun, setAutoRun] = useState(!restored);
    const common = { projectId: "project", filePath: "paper.tex", enabled: true, autoRun, onManualAction: () => { calls.activated++; setAutoRun(true); } };
    hooks.latex = useLatexCompile({ ...common, ready: true, source: "paper" });
    hooks.overleaf = useOverleafSync({ ...common, savedSource: "paper", dirty: false, onPulled: () => {} });
    return null;
  }
  await render(createElement(Viewer));
  await settle();
  return {
    calls,
    hooks,
    async act(action) {
      await act(action);
      await settle();
    },
    async focus() {
      await act(() => window.dispatchEvent(new Event("focus")));
      await settle();
    },
    async poll() {
      await act(() => t.mock.timers.tick(30_000));
      await settle();
    },
  };
}

test("restored tabs load controls but focus and polling never compile or sync", async (t) => {
  const tab = await viewer(t, true);
  assert.equal(tab.hooks.latex.engine, "tectonic");
  assert.equal(tab.hooks.latex.compiled, null);
  assert.equal(tab.hooks.latex.showPdf, false);
  assert.equal(tab.hooks.overleaf.loaded, true);
  await tab.focus();
  await tab.poll();
  assert.deepEqual(tab.calls, { compile: 0, sync: 0, status: 0, activated: 0 });
});

test("explicit compile opts both hooks in without a duplicate initial compile", async (t) => {
  const tab = await viewer(t, true);
  await tab.act(() => tab.hooks.latex.compile());
  assert.deepEqual(tab.calls, { compile: 1, sync: 1, status: 0, activated: 1 });
  await tab.poll();
  assert.equal(tab.calls.status, 1);
  assert.equal(tab.calls.sync, 2);
});

test("explicit sync opts both hooks in without a duplicate initial sync", async (t) => {
  const tab = await viewer(t, true);
  await tab.act(() => tab.hooks.overleaf.sync());
  assert.deepEqual(tab.calls, { compile: 1, sync: 1, status: 0, activated: 1 });
  await tab.poll();
  assert.equal(tab.calls.sync, 2);
});

test("link-and-sync remains an explicit opt-in; saving a token alone is passive", async (t) => {
  const tab = await viewer(t, true);
  await tab.act(() => tab.hooks.overleaf.saveToken("token"));
  assert.equal(tab.calls.sync, 0);
  await tab.act(() => tab.hooks.overleaf.linkProject("paper"));
  assert.deepEqual(tab.calls, { compile: 1, sync: 1, status: 0, activated: 1 });
});

test("intentionally opened tabs retain automatic compile and sync", async (t) => {
  const tab = await viewer(t, false);
  assert.deepEqual(tab.calls, { compile: 1, sync: 1, status: 0, activated: 0 });
  await tab.focus();
  assert.equal(tab.calls.compile, 1);
  assert.equal(tab.calls.sync, 1);
  await tab.poll();
  assert.equal(tab.calls.sync, 2);
});
