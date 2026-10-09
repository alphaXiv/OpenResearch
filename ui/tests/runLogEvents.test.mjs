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
  new Function("require", "exports", code)(() => ({
    useRef: (current) => ({ current }),
    useEffect: (effect) => { cleanup = effect(); },
  }), exports);
  const logs = [];
  exports.useOrxEvents({ onRun() {}, onRunLog: (event) => logs.push(event) });
  const event = { runId: "fresh-run", dataBase64: "bG9n", offset: 0 };
  exports.emitEntity.onRunLog(event);
  assert.deepEqual(logs, [event]);
  cleanup();
  exports.emitEntity.onRunLog(event);
  assert.equal(logs.length, 1);
});
