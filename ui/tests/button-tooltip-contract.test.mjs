import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";

const require = createRequire(import.meta.url);
function load(name) {
  const source = readFileSync(new URL(`../src/components/ui/${name}.tsx`, import.meta.url), "utf8");
  const compiled = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX },
  }).outputText;
  const exports = {};
  new Function("require", "exports", compiled)((id) => id === "./cn" ? { cn: (...values) => values.filter(Boolean).join(" ") } : require(id), exports);
  return exports;
}

const controls = [
  load("Button").Button,
  load("Button").ButtonLink,
  load("IconButton").IconButton,
  load("IconButton").IconButtonLink,
  load("MenuItem").MenuItem,
  load("Switch").Switch,
];

test("standard controls suppress native titles, including untyped spreads and ancestor titles", () => {
  for (const Control of controls) {
    const html = renderToStaticMarkup(React.createElement("div", { title: "ancestor" },
      React.createElement(Control, { title: "leaked", "aria-label": "Action", "data-tip": "Custom hint" })));
    assert.match(html, /title=""/);
    assert.doesNotMatch(html, /title="leaked"/);
    assert.match(html, /aria-label="Action"/);
    assert.match(html, /data-tip="Custom hint"/);
  }
});
