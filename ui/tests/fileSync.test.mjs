import assert from "node:assert/strict";
import test from "node:test";

import {
  conflictAfterRefresh,
  createFileBuffer,
  fileBufferContent,
} from "../src/fileSync.ts";

test("file buffers preserve line endings and only conflict after a dirty disk change", () => {
  const clean = createFileBuffer("notes.txt", "one\r\ntwo\r\n", "v1");
  assert.equal(fileBufferContent(clean), "one\r\ntwo\r\n");
  assert.equal(conflictAfterRefresh(clean, "v2", true), null);

  const dirty = { ...clean, draft: "one\nchanged\n" };
  assert.equal(conflictAfterRefresh(dirty, "v1", true), null);
  assert.deepEqual(conflictAfterRefresh(dirty, "v2", true), {
    currentVersion: "v2",
    exists: true,
  });
  assert.deepEqual(conflictAfterRefresh(dirty, null, false), {
    currentVersion: null,
    exists: false,
  });
});
