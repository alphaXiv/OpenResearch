import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
import * as markdownTarget from "../src/markdownTarget.ts";

const source = readFileSync(new URL("../src/components/HtmlPreview.tsx", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
}).outputText + "\nexports.assembleDocument = assembleDocument;";
const exports = {};
new Function("require", "exports", compiled)((name) => name === "../markdownTarget" ? markdownTarget : {}, exports);

// Just the DOM surface assembleDocument touches: elements selected by `tag[attribute]`.
function documentOf(elements) {
  const element = (tag, attributes) => ({
    attributes,
    matches: (selector) => selector === `${tag}[${Object.keys(attributes)[0]}]`,
    getAttribute: (name) => attributes[name] ?? null,
    setAttribute: (name, value) => { attributes[name] = value; },
  });
  const nodes = elements.map(([tag, attributes]) => element(tag, attributes));
  return {
    nodes,
    querySelectorAll: (selector) => selector === "a[href]" ? [] : nodes,
    querySelector: () => null,
    createElement: (tag) => element(tag, {}),
    head: { prepend() {} },
    doctype: null,
    documentElement: { outerHTML: "" },
  };
}

test("external assets bypass the local resolver; relative ones keep the MIME gate", async (t) => {
  const doc = documentOf([
    ["img", { src: "https://cdn.example/a.png" }],
    ["img", { src: "//cdn.example/b.png" }],
    ["script", { src: "https://cdn.example/x.js" }],
    ["img", { src: "chart.png" }],
    ["img", { src: "key.txt" }],
  ]);
  globalThis.DOMParser = class { parseFromString() { return doc; } };
  globalThis.FileReader = class { readAsDataURL(blob) { this.result = `data:${blob.type};stub`; this.onload(); } };
  t.after(() => { delete globalThis.DOMParser; delete globalThis.FileReader; });
  const types = { "/root/chart.png": "image/png", "/root/key.txt": "text/plain" };
  const fetched = [];
  t.mock.method(globalThis, "fetch", async (url) => {
    fetched.push(url);
    const headers = { "content-type": types[url], "content-length": "4" };
    return { ok: true, headers: { get: (name) => headers[name] }, blob: async () => new Blob(["data"], { type: types[url] }), body: { cancel: async () => {} } };
  });
  const resolved = [];
  await exports.assembleDocument("", (src) => { resolved.push(src); return `/root/${src}`; }, new AbortController().signal);
  assert.deepEqual(resolved, ["chart.png", "key.txt"]);
  assert.deepEqual(fetched, ["/root/chart.png", "/root/key.txt"]);
  assert.deepEqual(doc.nodes.map((node) => Object.values(node.attributes)[0]), [
    "https://cdn.example/a.png", "https://cdn.example/b.png", "https://cdn.example/x.js", "data:image/png;stub", "key.txt",
  ]);
});
