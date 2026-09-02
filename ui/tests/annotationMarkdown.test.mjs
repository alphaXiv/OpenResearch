import assert from "node:assert/strict";
import test from "node:test";
import {
  escapeMarkdownText,
  fencedCodeMarkdown,
  formatMath,
  headingMarkdown,
  isLegacyFingerprintMatch,
  inlineCodeMarkdown,
  listItemMarkdown,
  orderedListMarkdown,
  shouldRecoverLegacyMath,
  tableMarkdown,
} from "../src/components/annotationMarkdown.ts";

test("escapes literal Markdown without changing ordinary prose", () => {
  assert.equal(escapeMarkdownText("plain *literal* [text]"), "plain \\*literal\\* \\[text\\]");
  assert.equal(escapeMarkdownText("# not a heading"), "\\# not a heading");
  assert.equal(escapeMarkdownText("$5 and $10 ~~sale~~\n-----"), "\\$5 and \\$10 \\~\\~sale\\~\\~\n\\-----");
  assert.equal(escapeMarkdownText("Result\n===\nOther\n--"), "Result\n\\===\nOther\n\\--");
});

test("code keeps raw characters and uses safe dynamic backtick fences", () => {
  assert.equal(inlineCodeMarkdown("a*b[0]"), "`a*b[0]`");
  assert.equal(inlineCodeMarkdown("use `x`"), "`` use `x` ``");
  assert.equal(inlineCodeMarkdown(" foo "), "`  foo  `");
  assert.equal(fencedCodeMarkdown("before ``` after"), "\n\n````\nbefore ``` after\n````\n\n");
});

test("list continuations and nesting align to the rendered marker width", () => {
  assert.equal(
    listItemMarkdown("100.", "First line\nSecond line\n- nested\n\nFinal paragraph"),
    "100. First line\n     Second line\n     - nested\n\n     Final paragraph",
  );
});

test("preserves semantic math and headings", () => {
  assert.equal(formatMath("x^2", false), "\\(x^2\\)");
  assert.equal(formatMath("x^2", true), "\n\n\\[\nx^2\n\\]\n\n");
  assert.equal(headingMarkdown("H3", "Result"), "### Result");
});

test("preserves ordered-list start and explicit item values", () => {
  assert.equal(
    orderedListMarkdown([
      { markdown: "Fifth" },
      { markdown: "Tenth", value: 10 },
      { markdown: "Eleventh" },
    ], 5),
    "5. Fifth\n10. Tenth\n11. Eleventh",
  );
});

test("emits valid pipe-table Markdown", () => {
  assert.equal(
    tableMarkdown([["A", "B"], ["1", "2"]], true),
    "| A | B |\n| --- | --- |\n| 1 | 2 |",
  );
});

test("legacy recovery never replaces semantic or mixed-content annotations", () => {
  assert.equal(shouldRecoverLegacyMath("The result is \\(x^2\\)."), false);
  assert.equal(isLegacyFingerprintMatch("x2", "theresultisx2"), false);
  assert.equal(isLegacyFingerprintMatch("qtheta0ct", "qtheta0ct"), true);
});
