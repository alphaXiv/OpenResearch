import assert from "node:assert/strict";
import test from "node:test";

import { findMathTokens } from "../src/markdownLivePreview.ts";

// The editor's math scan must agree with remark-math on what renders as math.
const latex = (text) => findMathTokens(text).map(({ latex, block }) => ({ latex, block }));

test("inline and block spans read as math", () => {
  assert.deepEqual(latex("$x + y$"), [{ latex: "x + y", block: false }]);
  assert.deepEqual(latex("$$\nx^2\n$$"), [{ latex: "x^2", block: true }]);
  assert.deepEqual(latex("before $$x$$ after"), [{ latex: "x", block: false }]);
});

test("currency, escaped dollars and unterminated spans stay prose", () => {
  assert.deepEqual(latex("costs $5 and $10 total"), []);
  assert.deepEqual(latex("price \\$5 then \\$10"), []);
  assert.deepEqual(latex("half $x + y"), []);
});

test("boundary whitespace rejects a span but still consumes its delimiters", () => {
  assert.deepEqual(latex("$ x$"), []);
  assert.deepEqual(latex("$ x$ $y$"), [{ latex: "y", block: false }]);
});

test("a selection covering a span's text grows over its marks", async () => {
  const { EditorSelection, EditorState } = await import("@codemirror/state");
  const { markdown } = await import("@codemirror/lang-markdown");
  const { ensureSyntaxTree } = await import("@codemirror/language");
  const { selectionOverMarks } = await import("../src/markdownLivePreview.ts");
  const grow = (anchor, head, doc = "say **bold** and [link](https://x.org)") => {
    const state = EditorState.create({ doc, selection: EditorSelection.single(anchor, head), extensions: markdown() });
    ensureSyntaxTree(state, doc.length);
    const { anchor: a, head: h } = selectionOverMarks(state).main;
    return doc.slice(Math.min(a, h), Math.max(a, h)) + (a > h ? " (backward)" : "");
  };

  assert.equal(grow(4, 10), "**bold**");
  assert.equal(grow(0, 10), "say **bold**");
  assert.equal(grow(10, 4), "**bold** (backward)");
  assert.equal(grow(6, 9), "bol");
  assert.equal(grow(18, 22), "[link](https://x.org)");
  assert.equal(grow(3, 7, "***bold***"), "***bold***");
});
