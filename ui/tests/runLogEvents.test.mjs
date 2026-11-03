import { withWorkspaceMocks } from "./workspaceMocks.mjs";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

test("run logs reach entity observers once and unsubscribe on unmount", () => {
  let cleanup;
  const exports = {};
  const code = ts.transpileModule(readFileSync(new URL("../src/events.ts", import.meta.url), "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  new Function("require", "exports", code)(withWorkspaceMocks(() => ({
    useRef: (current) => ({ current }),
    useEffect: (effect) => { cleanup = effect(); },
  })), exports);
  Object.assign(exports, exports.createEvents({ url: (value) => value }));
  const logs = [];
  exports.useOrxEvents({ onRun() {}, onRunLog: (event) => logs.push(event) });
  const event = { runId: "fresh-run", dataBase64: "bG9n", offset: 0 };
  exports.emitEntity.onRunLog(event);
  assert.deepEqual(logs, [event]);
  cleanup();
  exports.emitEntity.onRunLog(event);
  assert.equal(logs.length, 1);
});

test("returning to a retained workspace repairs events missed while its stream was closed", (t) => {
  const refs = [];
  let index = 0, cleanup;
  const sources = [];
  globalThis.window = { clearTimeout };
  globalThis.EventSource = class {
    constructor(url) { this.url = url; sources.push(this); }
    addEventListener() {}
    close() { this.closed = true; }
  };
  t.after(() => { cleanup?.(); delete globalThis.window; delete globalThis.EventSource; });
  const exports = {};
  const code = ts.transpileModule(readFileSync(new URL("../src/events.ts", import.meta.url), "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  new Function("require", "exports", code)(() => ({
    useRef: (current) => refs[index++] ??= { current },
    useEffect: (effect) => { cleanup = effect(); },
  }), exports);
  const events = exports.createEvents({ url: (path) => `/machine/alpha${path}` });
  let repairs = 0, authProbes = 0;
  events.onHarnessAuth(() => authProbes++);
  const show = () => { index = 0; events.useOrxEventStream({ onRun() {}, onReconnect: () => repairs++ }); };
  show();
  sources[0].onopen();
  assert.equal(repairs, 0);
  cleanup();
  assert.equal(sources[0].closed, true);
  show();
  assert.equal(sources[1].url, "/machine/alpha/api/events");
  sources[1].onopen();
  assert.equal(repairs, 1);
  assert.equal(authProbes, 0);
});
