import assert from "node:assert/strict";
import test from "node:test";
import { normalizeMarkdownForRendering } from "../src/markdownNormalization.ts";

test("renders citation URLs and removes unresolved internal references", () => {
  assert.equal(
    normalizeMarkdownForRendering("结果。\uE200cite\uE202turn421719view1\uE201"),
    "结果。",
  );
  assert.equal(
    normalizeMarkdownForRendering("Result. \uE200cite\uE202https://www.alphaxiv.org/pdf/2604.26417\uE201"),
    "Result. <https://www.alphaxiv.org/pdf/2604.26417>",
  );
  assert.equal(
    normalizeMarkdownForRendering("\uE200cite\uE202turn1view0\uE202https://example.org/a\uE202https://example.org/b\uE201"),
    "<https://example.org/a> <https://example.org/b>",
  );
});

test("hides incomplete citation markers while streaming", () => {
  const citation = "\uE200cite\uE202https://example.org/paper\uE201";
  for (let end = 1; end < citation.length; end += 1) {
    assert.equal(normalizeMarkdownForRendering(`Result. ${citation.slice(0, end)}`, { predictMath: true }), "Result. ");
  }
  assert.equal(normalizeMarkdownForRendering(`Result. ${citation}`, { predictMath: true }), "Result. <https://example.org/paper>");
  assert.equal(normalizeMarkdownForRendering("\uE200cider", { predictMath: true }), "\uE200cider");
});

test("leaves citation syntax in code and mention tags untouched", () => {
  const citation = "\uE200cite\uE202turn1view0\uE201";
  const markdown = `\`${citation}\`\n\n\`\`\`\n${citation}\n\`\`\`\n\n<file path="${citation}">`;
  assert.equal(normalizeMarkdownForRendering(markdown), markdown);
});
