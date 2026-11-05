import assert from "node:assert/strict";
import test from "node:test";
import * as fileSync from "../src/fileSync.ts";

const {
  conflictAfterRefresh,
  confirmFileDiscard,
  createFileBuffer,
  fileBufferContent,
  FileBufferSession,
  updateFileDraft,
} = fileSync;

let dialogOptions;
let dialogError;
const showConfirmDialog = (options) => {
  if (dialogError) throw dialogError;
  dialogOptions = options;
};

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

test("discard confirmation protects blur-save until confirmed or canceled", async () => {
  for (const answer of [false, true]) {
    const pending = confirmFileDiscard("Discard?", showConfirmDialog);
    assert.equal(fileSync.confirmingFileDiscard, true);
    assert.equal(dialogOptions.description, "Discard?");
    assert.equal(dialogOptions.destructive, true);
    if (answer) dialogOptions.onConfirm();
    else dialogOptions.onCancel();
    assert.equal(await pending, answer);
    assert.equal(fileSync.confirmingFileDiscard, false);
  }

  dialogError = new Error("dialog unavailable");
  await assert.rejects(confirmFileDiscard("Discard?", showConfirmDialog), dialogError);
  assert.equal(fileSync.confirmingFileDiscard, false);
  dialogError = null;
});

test("undoing to the baseline clears the conflict without changing the saved version", () => {
  const clean = createFileBuffer("notes.txt", "baseline", "v1");
  const dirty = { ...clean, draft: "draft", conflict: { currentVersion: "v2", exists: true } };
  assert.deepEqual(updateFileDraft(dirty, "baseline"), clean);
  assert.deepEqual(updateFileDraft(dirty, "new draft").conflict, dirty.conflict);
});


test("a pending save survives viewer unmount and updates the remounted buffer", () => {
  const session = new FileBufferSession();
  session.set(updateFileDraft(createFileBuffer("paper.tex", "original", "v1"), "saved edit"));
  let firstNotifications = 0;
  const unmount = session.subscribe(() => firstNotifications++);
  session.setSaving(true);
  const revisionDuringSave = session.saveRevision;
  unmount();
  let displayed;
  const remount = session.subscribe(() => { displayed = session.getSnapshot(); });
  session.saved("saved edit", "v2");
  session.setSaving(false);
  assert.equal(firstNotifications, 1);
  assert.equal(displayed.draft, "saved edit");
  assert.equal(displayed.version, "v2");
  assert.equal(session.needsProtection, false);
  assert.equal(session.saving, false);
  assert.equal(revisionDuringSave, 1);
  remount();
});

test("save completion preserves edits made after returning to the tab", () => {
  const session = new FileBufferSession();
  session.set(updateFileDraft(createFileBuffer("paper.tex", "original", "v1"), "first edit"));
  session.setSaving(true);
  session.set(updateFileDraft(session.getSnapshot(), "second edit"));
  session.saved("first edit", "v2");
  session.setSaving(false);
  assert.equal(session.getSnapshot().draft, "second edit");
  assert.equal(session.getSnapshot().baseline, "first edit");
  assert.equal(session.getSnapshot().version, "v2");
  assert.equal(session.needsProtection, true);
  session.set(null);
  session.saved("second edit", "v3");
  assert.equal(session.getSnapshot(), null);
  assert.equal(session.needsProtection, false);
});


test("undo during a pending save remains protected until the saved baseline is known", () => {
  const session = new FileBufferSession();
  session.set(updateFileDraft(createFileBuffer("paper.tex", "original", "v1"), "pending"));
  session.setSaving(true);
  session.set(updateFileDraft(session.getSnapshot(), "original"));
  assert.equal(session.needsProtection, true);
  session.saved("pending", "v2");
  session.setSaving(false);
  assert.equal(session.needsProtection, true);
  assert.equal(session.getSnapshot().draft, "original");
  assert.equal(session.getSnapshot().baseline, "pending");
});

test("a save error is retained and delivered to the current viewer", () => {
  const session = new FileBufferSession();
  let displayed;
  const unsubscribe = session.subscribe(() => { displayed = session.saveError; });
  session.setSaveError("Disk full");
  assert.equal(displayed, "Disk full");
  unsubscribe();
  assert.equal(session.saveError, "Disk full");
  session.setSaveError(null);
  assert.equal(session.saveError, null);
});
