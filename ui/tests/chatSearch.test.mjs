import assert from "node:assert/strict";
import test from "node:test";
import { findTextMatches } from "../src/chatSearch.ts";

test("chat find treats regex punctuation literally", () => {
  const text = "Use a.*[b](c)? + $ ^ {x} | \\ here";
  for (const query of [".*", "[b]", "(c)?", "+", "$", "^", "{x}", "|", "\\"]) {
    const start = text.indexOf(query);
    assert.deepEqual(findTextMatches(text, query), [{ start, end: start + query.length }]);
  }
});

test("case-insensitive Unicode matching preserves UTF-16 range offsets", () => {
  assert.deepEqual(findTextMatches("🔍 Привет привет ПРИВЕТ", "привет"), [
    { start: 3, end: 9 }, { start: 10, end: 16 }, { start: 17, end: 23 },
  ]);
  assert.deepEqual(findTextMatches("İ foo FOO", "foo"), [{ start: 2, end: 5 }, { start: 6, end: 9 }]);
  assert.deepEqual(findTextMatches("你好世界你好", "你好"), [{ start: 0, end: 2 }, { start: 4, end: 6 }]);
});

test("empty and missing queries have no matches, repeated matches do not overlap", () => {
  assert.deepEqual(findTextMatches("hello", ""), []);
  assert.deepEqual(findTextMatches("hello", "world"), []);
  assert.deepEqual(findTextMatches("aaaa", "aa"), [{ start: 0, end: 2 }, { start: 2, end: 4 }]);
});
