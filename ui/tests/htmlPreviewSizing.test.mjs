import assert from "node:assert/strict";
import test from "node:test";
import { inlineHtmlHeight } from "../src/htmlPreviewSizing.ts";

test("inline figures keep natural heights and reject malformed messages", () => {
  assert.equal(inlineHtmlHeight(418.2), 419);
  for (const value of [null, "418", 0, -1, NaN, Infinity]) {
    assert.equal(inlineHtmlHeight(value), null);
  }
});

test("viewport height plus body margins converges instead of growing indefinitely", () => {
  let height = 360;
  for (let i = 0; i < 100; i++) height = inlineHtmlHeight(height + 16);
  assert.equal(inlineHtmlHeight(height + 16), height);
  assert.ok(height <= 1200);
});
