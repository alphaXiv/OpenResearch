import assert from "node:assert/strict";
import test from "node:test";

import {
  conflictAfterRefresh,
  confirmFileDiscard,
  confirmingFileDiscard,
  createFileBuffer,
  fileBufferContent,
  updateFileDraft,
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

test("discard confirmation suppresses synchronous blur-save and always releases the guard", () => {
  const previous = globalThis.window;
  try {
    for (const answer of [false, true]) {
      let saved = false;
      globalThis.window = { confirm: () => {
        if (!confirmingFileDiscard) saved = true;
        return answer;
      } };
      assert.equal(confirmFileDiscard("Discard?"), answer);
      assert.equal(saved, false);
      assert.equal(confirmingFileDiscard, false);
    }
    globalThis.window = { confirm: () => { throw new Error("dialog unavailable"); } };
    assert.throws(() => confirmFileDiscard("Discard?"));
    assert.equal(confirmingFileDiscard, false);
  } finally {
    if (previous === undefined) delete globalThis.window;
    else globalThis.window = previous;
  }
});

test("undoing to the baseline clears the conflict without changing the saved version", () => {
  const clean = createFileBuffer("notes.txt", "baseline", "v1");
  const dirty = { ...clean, draft: "draft", conflict: { currentVersion: "v2", exists: true } };
  assert.deepEqual(updateFileDraft(dirty, "baseline"), clean);
  assert.deepEqual(updateFileDraft(dirty, "new draft").conflict, dirty.conflict);
});
