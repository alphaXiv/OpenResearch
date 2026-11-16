import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
import * as markdownTarget from "../src/markdownTarget.ts";

const source = readFileSync(new URL("../src/mermaidImages.ts", import.meta.url), "utf8");
const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
const moduleExports = {};
new Function("require", "exports", code)((name) => name === "./markdownTarget" ? markdownTarget : {}, moduleExports);

function element(localName, href) {
  const attrs = { href };
  return {
    localName,
    attrs,
    getAttribute: (name) => attrs[name] ?? null,
    getAttributeNS: () => null,
    removeAttribute: (name) => { delete attrs[name]; },
    removeAttributeNS: () => {},
    setAttribute: (name, value) => { attrs[name] = value; },
  };
}

test("native diagram images and links keep SVG fragments", (t) => {
  const nodes = [element("image", "file:///C:/figures/chart.svg#plot"), element("a", "file:///C:/figures/chart.svg#section")];
  globalThis.DOMParser = class { parseFromString() { return { documentElement: { localName: "svg" }, querySelector: () => null, querySelectorAll: () => nodes }; } };
  globalThis.XMLSerializer = class { serializeToString() { return "<svg/>"; } };
  t.after(() => { delete globalThis.DOMParser; delete globalThis.XMLSerializer; });
  moduleExports.resolveMermaidImages("<svg/>", (path) => `/api/raw?path=${encodeURIComponent(path)}`, "/api/");
  assert.equal(nodes[0].attrs.href, "/api/raw?path=C%3A%2Ffigures%2Fchart.svg#plot");
  assert.equal(nodes[1].attrs.href, "/api/raw?path=C%3A%2Ffigures%2Fchart.svg#section");
  assert.equal(nodes[1].attrs["data-orx-file-target"], "file:///C:/figures/chart.svg#section");
});

test("external diagram links cannot masquerade as local file links", (t) => {
  const node = element("a", "https://example.com/");
  node.attrs["data-orx-file-target"] = "local.md";
  globalThis.DOMParser = class { parseFromString() { return { documentElement: { localName: "svg" }, querySelector: () => null, querySelectorAll: () => [node] }; } };
  globalThis.XMLSerializer = class { serializeToString() { return "<svg/>"; } };
  t.after(() => { delete globalThis.DOMParser; delete globalThis.XMLSerializer; });
  moduleExports.resolveMermaidImages("<svg/>", () => "", "/api/");
  assert.equal(node.attrs["data-orx-file-target"], undefined);
  assert.equal(node.attrs.href, "https://example.com/");
});

test("diagram image fallback advances only through scoped candidates", (t) => {
  const node = element("image", "artifacts/chart.png");
  node.attrs["data-orx-fallbacks"] = '["https://untrusted.invalid/image"]';
  globalThis.DOMParser = class { parseFromString() { return { documentElement: { localName: "svg" }, querySelector: () => null, querySelectorAll: () => [node] }; } };
  globalThis.XMLSerializer = class { serializeToString() { return "<svg/>"; } };
  t.after(() => { delete globalThis.DOMParser; delete globalThis.XMLSerializer; });
  const resolve = () => ["/api/missing", "https://untrusted.invalid/image", "/api/found"].map(url => ({ url, resolveAsset: () => null }));
  moduleExports.resolveMermaidImages("<svg/>", () => "", "/api/", resolve);
  assert.equal(node.attrs.href, "/api/missing");
  moduleExports.advanceMermaidImage(node, "/api/");
  assert.equal(node.attrs.href, "/api/found");
  moduleExports.advanceMermaidImage(node, "/api/");
  assert.equal(node.attrs.href, "/api/found");
});
