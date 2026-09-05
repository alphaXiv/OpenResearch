import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

// Execute the hooks with deterministic effect turns and timers, without a browser.
function viewerHooks(restored) {
  const slots = [];
  const timers = new Map();
  const calls = { compile: 0, sync: 0, status: 0, activated: 0 };
  let cursor = 0;
  let changed = true;
  let effects = [];
  let timerId = 0;
  let autoRun = !restored;
  const same = (a, b) => a && a.length === b.length && a.every((v, i) => Object.is(v, b[i]));
  const react = {
    useState(initial) {
      const index = cursor++;
      if (!(index in slots)) slots[index] = initial;
      return [slots[index], (update) => {
        const next = typeof update === "function" ? update(slots[index]) : update;
        if (!Object.is(next, slots[index])) { slots[index] = next; changed = true; }
      }];
    },
    useRef(initial) {
      const index = cursor++;
      slots[index] ??= { current: initial };
      return slots[index];
    },
    useCallback(callback, deps) {
      const index = cursor++;
      if (!same(slots[index]?.deps, deps)) slots[index] = { deps, callback };
      return slots[index].callback;
    },
    useEffect(effect, deps) {
      const index = cursor++;
      if (same(slots[index]?.deps, deps)) return;
      const previous = slots[index];
      slots[index] = { deps };
      effects.push(() => {
        previous?.cleanup?.();
        slots[index].cleanup = effect();
      });
    },
  };
  const link = { projectId: "paper", url: "https://overleaf.com/project/paper" };
  const api = {
    getLatexEngine: async () => ({ engine: "tectonic", hint: null, installCommand: null }),
    compileLatex: async () => {
      calls.compile++;
      return { ok: true, pdfPath: "paper.pdf", hadErrors: false, note: null };
    },
    getOverleafState: async () => ({ hasToken: true, link }),
    getOverleafStatus: async () => { calls.status++; return { remoteChanged: true }; },
    syncOverleaf: async () => {
      calls.sync++;
      return { pulled: [], pushed: [], conflicts: [] };
    },
    overleafUploadUrl: () => "https://overleaf.com/upload",
    linkOverleaf: async () => ({ hasToken: true, link }),
    saveOverleafToken: async () => ({ hasToken: true }),
  };
  function loadHook(name) {
    const source = readFileSync(new URL(`../src/${name}.ts`, import.meta.url), "utf8");
    const compiled = ts.transpileModule(source, {
      compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
    }).outputText;
    const exports = {};
    new Function("require", "exports", "setInterval", "clearInterval", compiled)(
      (id) => {
        if (id === "react") return react;
        if (id === "./api") return api;
        if (id === "./paraglide/messages.js") return { m: {} };
        throw new Error(`Unexpected dependency: ${id}`);
      },
      exports,
      (callback) => { timers.set(++timerId, callback); return timerId; },
      (id) => timers.delete(id),
    );
    return exports[name];
  }
  const useLatexCompile = loadHook("useLatexCompile");
  const useOverleafSync = loadHook("useOverleafSync");
  const onManualAction = () => { calls.activated++; autoRun = true; changed = true; };
  const result = {
    calls,
    async flush() {
      for (let turn = 0; turn < 20; turn++) {
        if (changed) {
          changed = false;
          cursor = 0;
          const common = { projectId: "project", filePath: "paper.tex", enabled: true, autoRun, onManualAction };
          result.latex = useLatexCompile({ ...common, ready: true, source: "paper" });
          result.overleaf = useOverleafSync({ ...common, savedSource: "paper", dirty: false, onPulled: () => {} });
          const pending = effects;
          effects = [];
          pending.forEach((effect) => effect());
        }
        await Promise.resolve();
      }
    },
    async focus() { changed = true; await result.flush(); },
    async poll() { [...timers.values()].forEach((callback) => callback()); await result.flush(); },
  };
  return result;
}

test("restored tabs load controls but focus and polling never compile or sync", async () => {
  const viewer = viewerHooks(true);
  await viewer.flush();
  assert.equal(viewer.latex.engine, "tectonic");
  assert.equal(viewer.latex.compiled, null);
  assert.equal(viewer.latex.showPdf, false);
  assert.equal(viewer.overleaf.loaded, true);
  await viewer.focus();
  await viewer.poll();
  assert.deepEqual(viewer.calls, { compile: 0, sync: 0, status: 0, activated: 0 });
});

test("explicit compile opts both hooks in without a duplicate initial compile", async () => {
  const viewer = viewerHooks(true);
  await viewer.flush();
  viewer.latex.compile();
  await viewer.flush();
  assert.deepEqual(viewer.calls, { compile: 1, sync: 1, status: 0, activated: 1 });
  await viewer.poll();
  assert.equal(viewer.calls.status, 1);
  assert.equal(viewer.calls.sync, 2);
});

test("explicit sync opts both hooks in without a duplicate initial sync", async () => {
  const viewer = viewerHooks(true);
  await viewer.flush();
  viewer.overleaf.sync();
  await viewer.flush();
  assert.deepEqual(viewer.calls, { compile: 1, sync: 1, status: 0, activated: 1 });
  await viewer.poll();
  assert.equal(viewer.calls.sync, 2);
});

test("link-and-sync remains an explicit opt-in; saving a token alone is passive", async () => {
  const viewer = viewerHooks(true);
  await viewer.flush();
  await viewer.overleaf.saveToken("token");
  await viewer.flush();
  assert.equal(viewer.calls.sync, 0);
  await viewer.overleaf.linkProject("paper");
  await viewer.flush();
  assert.deepEqual(viewer.calls, { compile: 1, sync: 1, status: 0, activated: 1 });
});

test("intentionally opened tabs retain automatic compile and sync", async () => {
  const viewer = viewerHooks(false);
  await viewer.flush();
  assert.deepEqual(viewer.calls, { compile: 1, sync: 1, status: 0, activated: 0 });
  await viewer.focus();
  assert.equal(viewer.calls.compile, 1);
  assert.equal(viewer.calls.sync, 1);
  await viewer.poll();
  assert.equal(viewer.calls.sync, 2);
});
