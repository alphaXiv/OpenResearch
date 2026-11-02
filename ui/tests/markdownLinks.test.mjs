import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { createElement } from "react";
import ts from "typescript";
import { unified } from "unified";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import * as targets from "../src/markdownTarget.ts";

const source = readFileSync(new URL("../src/components/Md.tsx", import.meta.url), "utf8");
const tree = ts.createSourceFile("Md.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
let anchor;
let isFile;
function visit(node) {
  if (ts.isFunctionDeclaration(node) && node.name?.text === "isFileHref") isFile = node.getText(tree);
  if (ts.isPropertyAssignment(node) && node.name.getText(tree) === "a") anchor = node.initializer.getText(tree);
  ts.forEachChild(node, visit);
}
visit(tree);
assert.ok(anchor && isFile);
const compiled = ts.transpileModule(`${isFile}\nconst render = ${anchor};`, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.React, jsxFactory: "createElement" },
}).outputText;
const FileReference = () => null;
const InsideLinkContext = () => null;
const renderer = new Function("targets", "createElement", "FileReference", "InsideLinkContext", "onOpenFile", "resolveFilePath", `const { nativeMarkdownPath, splitLineSuffix, citedFilePath, chatImageTarget, encodeMarkdownPath } = targets; ${compiled}\nreturn render;`);
const processor = unified().use(remarkParse).use(remarkRehype).use(targets.rehypeSafeUrls);
const link = (target, { onOpenFile, resolveFilePath } = {}) => renderer(targets, createElement, FileReference, InsideLinkContext, onOpenFile, resolveFilePath)({
  ...processor.runSync(processor.parse(`[label](${target})`)).children[0].children[0].properties,
  children: "label",
});

test("local links dispatch native paths for PDF and other file types", () => {
  for (const target of ["C:/report.pdf", "file:///C:/notes.txt", "file:///tmp/data.csv", "file:///tmp/plot.png"]) {
    const element = link(target, { onOpenFile() {}, resolveFilePath: (path) => `docs/${path}` });
    assert.equal(element.type, FileReference);
    assert.equal(element.props.path, targets.nativeMarkdownPath(target));
  }
});

test("relative links keep document context and source-line citations", () => {
  const element = link("src/main.rs:42", { onOpenFile() {}, resolveFilePath: (path) => `docs/${path}` });
  assert.equal(element.type, FileReference);
  assert.equal(element.props.path, "docs/src/main.rs");
  assert.equal(element.props.line, 42);
  const windows = link("C:/src/main.rs:42", { onOpenFile() {} });
  assert.equal(windows.props.path, "C:/src/main.rs");
  assert.equal(windows.props.line, 42);
});

test("rejected, empty and handlerless file links cannot open duplicate tabs", () => {
  for (const target of ["", "javascript:alert(1)", "vscode://file/a.txt", "file://server/share/a.pdf", "file:///tmp/%00", "file:///tmp/%ZZ"]) {
    assert.equal(link(target, { onOpenFile() {} }).type, "span", target);
  }
  for (const target of ["C:/report.pdf", "file:///tmp/a.txt", "relative.txt"]) {
    assert.equal(link(target).type, "span", target);
  }
  for (const target of ["https://example.com/a.pdf", "mailto:test@example.com", "//example.com/a.pdf"]) {
    const element = link(target);
    assert.equal(element.type, "a");
    assert.equal(element.props.href, target);
  }
});
