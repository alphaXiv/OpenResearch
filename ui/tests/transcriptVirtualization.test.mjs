import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
import { defaultRangeExtractor } from "@tanstack/react-virtual";

const file = ts.createSourceFile("ChatPanel.tsx", readFileSync(new URL("../src/components/ChatPanel.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
function find(predicate) {
  let found;
  function visit(node) {
    if (predicate(node)) found = node;
    ts.forEachChild(node, visit);
  }
  visit(file);
  assert.ok(found);
  return found;
}
function evaluate(code, bindings) {
  const js = ts.transpileModule(code, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  return new Function(...Object.keys(bindings), js)(...Object.values(bindings));
}

test("virtual history retains interacted rows outside the viewport and drops absent branch IDs", () => {
  const expression = find(node => ts.isVariableDeclaration(node) && node.name.getText(file) === "rangeExtractor").initializer;
  const retainedRows = { current: new Set(["a", "deleted"]) };
  const extract = evaluate(`return ${expression.getText(file)}`, {
    useCallback: callback => callback,
    visibleMessages: ["a", "b", "c", "d", "e"].map(id => ({ id })),
    retainedRows,
    fullHistory: false,
    defaultRangeExtractor,
  });
  assert.deepEqual(extract({ startIndex: 3, endIndex: 4, overscan: 0, count: 5 }), [0, 3, 4]);
  retainedRows.current.add("b");
  assert.deepEqual(extract({ startIndex: 4, endIndex: 4, overscan: 0, count: 5 }), [0, 1, 4]);
});

test("tool labels reuse immutable parts but refresh after stream replacement or locale change", () => {
  const declaration = find(node => ts.isFunctionDeclaration(node) && node.name?.text === "toolActivity");
  let locale = "en", calls = 0;
  const activity = evaluate(`const toolActivities = new WeakMap(); ${declaration.getText(file)}; return toolActivity;`, {
    getLocale: () => locale,
    computeToolActivity: part => { calls++; return { label: `${locale}:${part.state.status}` }; },
  });
  const original = { id: "tool-1", state: { status: "running" } };
  assert.strictEqual(activity(original), activity(original));
  assert.equal(calls, 1);
  assert.equal(activity({ ...original, state: { status: "completed" } }).label, "en:completed");
  locale = "es";
  assert.equal(activity(original).label, "es:running");
  assert.equal(calls, 3);
});
