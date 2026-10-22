import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const file = ts.createSourceFile("SubagentTab.tsx", readFileSync(new URL("../src/components/SubagentTab.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
function evaluate(code, bindings) {
  const js = ts.transpileModule(code, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  return new Function(...Object.keys(bindings), js)(...Object.values(bindings));
}

test("a transcript that finishes loading after its messages attaches its growth observer", () => {
  let effect, loading;
  function visit(node) {
    if (ts.isVariableDeclaration(node) && node.name.getText(file) === "loading") loading = node.initializer;
    if (ts.isCallExpression(node) && node.expression.getText(file) === "useEffect" && node.getText(file).includes("new ResizeObserver")) effect = node;
    ts.forEachChild(node, visit);
  }
  visit(file);
  assert.ok(effect && loading);
  const scrollRef = { current: null }, innerRef = { current: null };
  const observed = [];
  let resize, previousDependencies;
  const bindings = {
    scrollRef, innerRef, stickToBottom: { current: true },
    ResizeObserver: class {
      constructor(callback) { resize = callback; }
      observe(element) { observed.push(element); }
      disconnect() {}
    },
    useEffect: (setup, dependencies) => {
      if (!previousDependencies || dependencies.some((value, i) => value !== previousDependencies[i])) setup();
      previousDependencies = dependencies;
    },
  };
  const render = (pending) => evaluate(`${effect.getText(file)};`, {
    ...bindings,
    loading: evaluate(`return ${loading.getText(file)};`, { messages: [], nativeTranscript: { isPending: pending } }),
  });
  render(true);
  assert.deepEqual(observed, []);
  const scroll = { scrollTop: 0, scrollHeight: 500 }, inner = {};
  scrollRef.current = scroll;
  innerRef.current = inner;
  render(false);
  assert.deepEqual(observed, [inner, scroll]);
  scroll.scrollHeight = 1000;
  resize();
  assert.equal(scroll.scrollTop, 1000);
});
