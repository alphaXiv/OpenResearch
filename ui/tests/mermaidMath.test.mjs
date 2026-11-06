import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
import { MERMAID_SOURCE_LIMIT } from "../src/mermaidLimits.ts";

const source = readFileSync(new URL("../src/mermaidMath.ts", import.meta.url), "utf8");
const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
const exports = {};
const rendered = [];
new Function("require", "exports", code)((name) => name === "katex" ? { default: { renderToString: (tex) => { rendered.push(tex); return tex; } } } : { MERMAID_SOURCE_LIMIT }, exports);

test("math in an accepted large diagram is measured, with work bounded", (t) => {
  const math = { outerHTML: "<math/>", getBoundingClientRect: () => ({ width: 20, height: 10 }) };
  globalThis.document = { createElement: () => ({ style: {}, querySelector: () => math, remove() {} }), body: { append() {} } };
  t.after(() => { delete globalThis.document; });
  const expressions = Array.from({ length: 70 }, (_, i) => `$$x_${i}$$`).join(" ");
  const diagram = " ".repeat(50_001) + expressions;
  const measured = exports.measureMermaidMath(diagram);
  assert.equal(Object.keys(measured).length, 64);
  assert.equal(rendered.length, 64);
  assert.equal(measured.x_0.width, 20);
  assert.equal(measured.x_64, undefined);
  assert.deepEqual(Object.keys(exports.measureMermaidMath(" ".repeat(MERMAID_SOURCE_LIMIT) + "$$x$$")), []);
});
