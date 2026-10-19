import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";

const source = ts.createSourceFile("ChatPanel.tsx", readFileSync(new URL("../src/components/ChatPanel.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
const declaration = source.statements.find((node) => ts.isFunctionDeclaration(node) && node.name?.text === "PromptCard");
assert.ok(declaration);
const compiled = ts.transpileModule(declaration.getText(source), { compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX } }).outputText;
const Card = new Function("require", "useState", "m", "Button", "TriangleAlert", "inputString", "permissionActivityLabel", `const exports = {}; ${compiled}; return PromptCard;`)(
  createRequire(import.meta.url), React.useState,
  new Proxy({}, { get: (_, name) => () => String(name) }),
  ({ children, onClick }) => React.createElement("button", { onClick }, children),
  () => null, () => "", () => "Native tool",
);

test("native permission cards render exact agent labels in their original order", () => {
  const part = { id: "permission", prompt: { kind: "permission", header: "Inspect the file", nativeChoices: [
    { id: "opaque/reject", label: "Skip this operation", kind: "reject_once" },
    { id: "opaque/allow", label: "Allow exactly once", kind: "allow_once" },
  ] } };
  const html = renderToStaticMarkup(React.createElement(Card, { part, onRespond: () => {} }));
  assert.ok(html.includes("Inspect the file"));
  assert.ok(html.indexOf("Skip this operation") < html.indexOf("Allow exactly once"));
  assert.equal((html.match(/<button/g) ?? []).length, 2);
  assert.equal(renderToStaticMarkup(React.createElement(Card, { part: { ...part, prompt: { ...part.prompt, resolved: true } } })), "");
});
