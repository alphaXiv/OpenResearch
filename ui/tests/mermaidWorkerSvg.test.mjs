import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import ts from "typescript";

const require = createRequire(import.meta.url);
const source = readFileSync(new URL("../src/mermaidWorkerSvg.ts", import.meta.url), "utf8");
const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
const exports = {};
new Function("require", "exports", code)(require, exports);
const svg = (body) => `<svg xmlns="http://www.w3.org/2000/svg">${body}</svg>`;

test("worker preparation preserves ordinary SVG and declines URL rewriting cases", () => {
  assert.match(exports.prepareMermaidSvg(svg('<text>ordinary &amp; safe</text>')), /ordinary &amp; safe/);
  for (const input of [svg('<image href="local.png"/>'), svg('<a href="local.md"><text>link</text></a>'), svg('<s:image xmlns:s="http://www.w3.org/2000/svg"/>'), svg('<s:a xmlns:s="http://www.w3.org/2000/svg"/>')]) {
    assert.equal(exports.prepareMermaidSvg(input), null);
  }
});

test("worker preparation falls back for malformed, wrong-root and DTD documents", () => {
  for (const input of ['<svg><image></svg>', '<html/>', '<!DOCTYPE svg><svg xmlns="http://www.w3.org/2000/svg"/>', svg('<parsererror/>')]) {
    assert.equal(exports.prepareMermaidSvg(input), null);
  }
});
