import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
import * as react from "react";
import * as jsxRuntime from "react/jsx-runtime";
import * as reactDom from "react-dom";
import { renderToStaticMarkup } from "react-dom/server";
import rehypeKatex from "rehype-katex";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { unified } from "unified";
import * as markdownTarget from "../src/markdownTarget.ts";
import * as markdownNormalization from "../src/markdownNormalization.ts";
import * as remarkFigures from "../src/remarkFigures.ts";
import * as tabPreview from "../src/tabPreview.ts";

// Keep Md's processor, link renderer and click handlers real. Capture the
// streaming renderer's props so these tests need neither a browser nor timers.
let markdown;
const dependencies = {
  react,
  "react/jsx-runtime": jsxRuntime,
  "react-dom": reactDom,
  "@clo/react-markdown": { Markdown: (props) => { markdown = props; return null; } },
  "lucide-react": { FileCode: () => null, PanelRight: () => null },
  "../paraglide/messages.js": { m: { a11y_open_file_in_panel: ({ path }) => path } },
  "../i18n": { ltr: (text) => text },
  "../locale": { useLocale: () => {} },
  "rehype-katex": { default: rehypeKatex },
  "remark-gfm": { default: remarkGfm },
  "remark-math": { default: remarkMath },
  "remark-parse": { default: remarkParse },
  "remark-rehype": { default: remarkRehype },
  unified: { unified },
  "../markdownTarget": markdownTarget,
  "../markdownNormalization": markdownNormalization,
  "../remarkFigures": remarkFigures,
  "../tabPreview": tabPreview,
  "../syntaxLanguage": {},
  "../syntaxHighlight": {},
  "./ui": {},
  "../api": {},
  "./InlineHtmlFigure": {},
  "./ImageCarousel": {},
  "../imageZoom": {},
};
const source = readFileSync(new URL("../src/components/Md.tsx", import.meta.url), "utf8");
const code = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
}).outputText;
const exports = {};
new Function("require", "exports", code)((id) => {
  assert.ok(Object.hasOwn(dependencies, id), `Unexpected Md dependency: ${id}`);
  return dependencies[id];
}, exports);

function link(text, resolveFilePath) {
  const opened = [];
  renderToStaticMarkup(react.createElement(exports.Md, { text, resolveFilePath, onOpenFile: (...args) => opened.push(args) }));
  const tree = markdown.processor.runSync(markdown.processor.parse(markdown.content));
  const anchors = [];
  const visit = (node) => {
    if (node.tagName === "a") anchors.push(node);
    node.children?.forEach(visit);
  };
  visit(tree);
  const anchor = anchors.find((node) => node.properties["data-figure-src"]) ?? anchors[0];
  assert.ok(anchor, "Markdown should contain a link");
  let element = markdown.components.a({ ...anchor.properties, children: "link" });
  if (typeof element.type === "function") element = element.type(element.props);
  return { element, opened };
}

const inDocs = (target) => markdownTarget.resolveMarkdownTarget("docs", target)?.path ?? null;
const click = ({ element, opened }) => {
  assert.equal(element.type, "button");
  element.props.onClick({});
  return opened[0];
};

test("file links decode their filenames once when opened from a Markdown file", () => {
  for (const [target, filename] of [
    ["chart%231.png", "chart#1.png"],
    ["chart%3F1.png", "chart?1.png"],
    ["chart%25231.png", "chart%231.png"],
    ["chart%26draft.png", "chart&draft.png"],
    ["chart%3A42.png", "chart:42.png"],
  ]) {
    assert.deepEqual(click(link(`[file](${target})`, inDocs)), [`docs/${filename}`, undefined, undefined, undefined, "preview"]);
  }
});

test("figure references pass the encoded image target to the file resolver", () => {
  for (const [target, filename] of [
    ["figures/chart%231.png", "figures/chart#1.png"],
    ["figures/chart%3F1.png", "figures/chart?1.png"],
    ["figures/chart%25231.png", "figures/chart%231.png"],
    ["artifacts/chart%231.png?download=1#zoom", "artifacts/chart#1.png"],
  ]) {
    const text = `[Figure 1](https://example.com/paper)\n\n![Plot](${target} "Figure 1. Plot.")`;
    assert.deepEqual(click(link(text, inDocs)), [`docs/${filename}`, undefined, undefined, undefined, "preview"]);
  }
});

test("chat file links decode filenames once without a file resolver", () => {
  for (const [target, filename] of [
    ["chart%231.png", "chart#1.png"],
    ["chart%3F1.png", "chart?1.png"],
    ["chart%25231.png", "chart%231.png"],
    ["chart%26draft.png", "chart&draft.png"],
    ["chart%3A42.py", "chart:42.py"],
  ]) {
    assert.equal(click(link(`[file](${target})`))[0], filename);
  }
});

test("cited line numbers remain separate from encoded filename punctuation", () => {
  assert.deepEqual(click(link("[file](chart%3A42.py#L7)", inDocs)), ["docs/chart:42.py", 7, undefined, undefined, "preview"]);
});

test("invalid targets cannot open files and external links stay external", () => {
  for (const target of ["bad%escape.png", "%00.png", "%2e%2e/%2e%2e/outside.png"]) {
    const result = link(`[file](${target})`, inDocs);
    assert.equal(result.element.type, "span");
    assert.deepEqual(result.opened, []);
  }
  for (const target of ["https://example.com/chart%231.png", "//example.com/chart.png", "#section"]) {
    const { element, opened } = link(`[file](${target})`, inDocs);
    assert.equal(element.type, "a");
    assert.equal(element.props.href, target);
    assert.deepEqual(opened, []);
  }
});
