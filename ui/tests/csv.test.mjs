import assert from "node:assert/strict";
import test from "node:test";

import { detectDelimiter, parseCsv } from "../src/csv.ts";

test("quoted fields keep delimiters, newlines, and escaped quotes", () => {
  assert.deepEqual(parseCsv('a,"b,c","d\ne","say ""hi"""\n1,2,3,4\n', ","), [
    ["a", "b,c", "d\ne", 'say "hi"'],
    ["1", "2", "3", "4"],
  ]);
});

test("CRLF, lone CR, a BOM, and blank lines do not add rows or fields", () => {
  assert.deepEqual(parseCsv("﻿a,b\r\n\r\n1,2\r3,4", ","), [["a", "b"], ["1", "2"], ["3", "4"]]);
});

test("a trailing delimiter keeps its empty field", () => {
  assert.deepEqual(parseCsv("a,b,\n1,,", ","), [["a", "b", ""], ["1", "", ""]]);
});

test("an unterminated quote runs to the end of the input", () => {
  assert.deepEqual(parseCsv('a,"b\nc', ","), [["a", "b\nc"]]);
});

test("the delimiter comes from the extension, then the header line", () => {
  assert.equal(detectDelimiter("x.tsv", "a,b\tc"), "\t");
  assert.equal(detectDelimiter("x.csv", "a;b;c\n1,2,3"), ";");
  assert.equal(detectDelimiter("x.csv", '"a,b,c";d\n'), ";");
  assert.equal(detectDelimiter("x.csv", "only\n1"), ",");
});
