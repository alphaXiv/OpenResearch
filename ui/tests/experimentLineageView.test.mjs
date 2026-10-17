import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";

const require = createRequire(import.meta.url);
const en = JSON.parse(readFileSync(new URL("../messages/en.json", import.meta.url), "utf8"));
const messages = { m: new Proxy({}, { get: (_, key) => (values = {}) => en[key].replace(/\{(\w+)\}/g, (_, name) => String(values[name])) }) };
const api = { fmtNumber: String, timeAgo: () => "1d ago", runDisplayStatus: (run) => run.status };
const runtime = { getLocale: () => "en" };

function load(file, mocks) {
  const source = readFileSync(new URL(`../src/components/${file}`, import.meta.url), "utf8");
  const code = ts.transpileModule(source, {
    fileName: file,
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
  }).outputText;
  const exports = {};
  new Function("require", "exports", code)((name) => mocks[name] ?? require(name), exports);
  return exports;
}

const model = load("experimentLineageModel.ts", {
  "../api": api,
  "../i18n": { ltr: (text) => text },
  "../paraglide/messages.js": messages,
});

function lineage(expanded) {
  return load("ExperimentLineage.tsx", {
    react: { ...React, useState: () => [expanded, () => {}], useEffect: () => {} },
    "lucide-react": { ChevronRight: () => null },
    "../api": api,
    "../paraglide/messages.js": messages,
    "../paraglide/runtime.js": runtime,
    "./experimentLineageModel": model,
  });
}

const experiments = Array.from({ length: 8 }, (_, index) => ({
  id: String(index),
  title: `Experiment ${index}`,
  slug: `experiment-${index}`,
  createdAt: index,
  parentExperimentId: index ? String(index - 1) : null,
}));
const nodes = model.buildHistoryNodes(experiments, []);

test("lineage shows four ancestors and the current experiment before folding earlier ancestors", () => {
  const { ExperimentLineage } = lineage(false);
  const html = renderToStaticMarkup(React.createElement(ExperimentLineage, { node: nodes[6], onSelect: () => {} }));
  assert.match(html, /aria-expanded="false"/);
  assert.match(html, /2 earlier:/);
  for (let index = 2; index <= 6; index++) assert.match(html, new RegExp(`>Experiment ${index}<`));
  assert.doesNotMatch(html, />Experiment 0</);
  assert.match(html, /Children/);
  assert.match(html, />Experiment 7</);
  assert.match(html, /aria-current="true"/);
});

test("expanded lineage retains a collapse control and shows every ancestor", () => {
  const { ExperimentLineage } = lineage(true);
  const html = renderToStaticMarkup(React.createElement(ExperimentLineage, { node: nodes[6], onSelect: () => {} }));
  assert.match(html, /aria-expanded="true"/);
  assert.match(html, /Show less/);
  for (let index = 0; index <= 6; index++) assert.match(html, new RegExp(`>Experiment ${index}<`));
});

test("short lineage has no unnecessary expand control", () => {
  const { ExperimentLineage } = lineage(false);
  const html = renderToStaticMarkup(React.createElement(ExperimentLineage, { node: nodes[4], onSelect: () => {} }));
  assert.doesNotMatch(html, /aria-expanded/);
  for (let index = 0; index <= 4; index++) assert.match(html, new RegExp(`>Experiment ${index}<`));
});
