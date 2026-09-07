import assert from "node:assert/strict";
import test from "node:test";
import { mergeText } from "../src/textMerge.ts";

test("keeps the buffer when only one side changed", () => {
  assert.equal(mergeText("abc", "abc", "abcd"), "abcd");
  assert.equal(mergeText("abc", "abXc", "abc"), "abXc");
  assert.equal(mergeText("abc", "abc", "abc"), "abc");
  assert.equal(mergeText("abc", "aXc", "aXc"), "aXc");
});

test("folds a disk change into a buffer edited elsewhere", () => {
  assert.equal(mergeText("hello world", "HELLO world", "hello world!"), "HELLO world!");
  assert.equal(mergeText("hello world", "hello world!", "HELLO world"), "HELLO world!");
  assert.equal(mergeText("one two three", "one 2 three", "one two 3"), "one 2 3");
  assert.equal(mergeText("abc", "abc typed", ">abc"), ">abc typed");
  assert.equal(mergeText("abcdef", "abef", "abcdefg"), "abefg");
});

test("refuses when both sides touched the same span", () => {
  assert.equal(mergeText("abc", "aXc", "aYc"), null);
  assert.equal(mergeText("abcdef", "abXYef", "abcZef"), null);
});
