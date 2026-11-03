import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { noticeRun, runAbandonable, runActive, runDisplay, runNotice, runResult } from "../src/runDisplay.ts";

const unreachable = { type: "unreachable", since: 100, cause: "Operation timed out" };
const needsAuth = { type: "needs_auth", since: 200, cause: "Duo approval required" };
const unreconciled = (detail) => ({ type: "unreconciled", since: 300, detail });

function run(status, extra = {}) {
  return { id: `run-${status}`, status, cancelling: false, contact: null, lostReason: null, host: "gpu-box", backend: null, createdAt: 1, ...extra };
}

describe("runDisplay", () => {
  test("live contact shows the phase, pulsing only while active", () => {
    for (const status of ["submitting", "starting", "running", "done", "failed", "cancelled", "lost"]) {
      assert.deepEqual(runDisplay(run(status)), { status, stale: false, since: null });
    }
    assert.equal(runDisplay(run("running", { cancelling: true })).status, "cancelling");
    assert.equal(runDisplay(run("submitting", { cancelling: true })).status, "cancellation_pending");
    assert.deepEqual(runNotice(run("submitting", { cancelling: true })), { kind: "cancel_pending_launch" });
  });

  test("lost contact replaces the phase with what the user must know", () => {
    const cases = [
      [run("running", { contact: unreachable }), "unreachable", 100],
      [run("starting", { contact: needsAuth }), "needs_auth", 200],
      [run("submitting", { contact: unreconciled("No sandbox id") }), "unconfirmed", 300],
      [run("running", { contact: unreconciled("Job not found") }), "unconfirmed", 300],
      [run("running", { cancelling: true, contact: unreachable }), "cancellation_pending", 100],
      [run("running", { cancelling: true, contact: needsAuth }), "cancellation_pending", 200],
      [run("submitting", { cancelling: true, contact: unreconciled("No sandbox id") }), "cancellation_pending", 300],
      [run("running", { cancelling: true, contact: unreconciled("Cancel not verified: 2 survivors") }), "cancelling", 300],
    ];
    for (const [input, status, since] of cases) {
      assert.deepEqual(runDisplay(input), { status, stale: true, since }, `${input.status} ${input.contact.type} cancelling=${input.cancelling}`);
    }
  });

  test("an abandoned run stays hollow while its kill is pending", () => {
    assert.deepEqual(runDisplay(run("lost", { lostReason: "abandoned", contact: unreconciled("Abandoned") })), { status: "abandoned", stale: true, since: 300 });
    assert.deepEqual(runDisplay(run("lost", { lostReason: "abandoned" })), { status: "abandoned", stale: false, since: null });
    assert.equal(runDisplay(run("lost", { lostReason: "host rebooted" })).status, "lost");
  });

  test("session rows without engine fields fall back to cancelRequested", () => {
    assert.equal(runDisplay({ status: "running", cancelRequested: true }).status, "cancelling");
    assert.equal(runDisplay({ status: "done", cancelRequested: true }).status, "done");
    assert.equal(runDisplay({ status: "running", cancelRequested: false }).stale, false);
  });
});

describe("runNotice", () => {
  test("legacy runs keep their monitoring error while active", () => {
    const legacy = run("running", { backend: { monitoringError: "Monitoring unavailable" } });
    assert.deepEqual(runNotice(legacy), { kind: "monitoring", error: "Monitoring unavailable" });
    assert.equal(runNotice({ ...legacy, status: "done" }), null);
    assert.equal(runNotice(run("running")), null);
  });

  test("each contact maps to one notice with its host and time", () => {
    assert.deepEqual(runNotice(run("running", { contact: unreachable })), { kind: "unreachable", host: "gpu-box", since: 100, cause: "Operation timed out" });
    assert.deepEqual(runNotice(run("running", { contact: needsAuth })), { kind: "needs_auth", host: "gpu-box", since: 200, cause: "Duo approval required" });
    assert.deepEqual(runNotice(run("submitting", { contact: unreconciled("No sandbox id") })), { kind: "unconfirmed_launch", detail: "No sandbox id" });
    assert.deepEqual(runNotice(run("running", { contact: unreconciled("Job not found") })), { kind: "unreconciled", detail: "Job not found" });
  });

  test("a pending cancel says why it is pending", () => {
    assert.deepEqual(runNotice(run("running", { cancelling: true, contact: unreachable })), { kind: "cancel_pending", host: "gpu-box", since: 100, contact: "unreachable" });
    assert.deepEqual(runNotice(run("running", { cancelling: true, contact: needsAuth })), { kind: "cancel_pending", host: "gpu-box", since: 200, contact: "needs_auth" });
    assert.deepEqual(runNotice(run("submitting", { cancelling: true, contact: unreconciled("No sandbox id") })), { kind: "cancel_pending_launch" });
    assert.deepEqual(runNotice(run("running", { cancelling: true, contact: unreconciled("2 survivors") })), { kind: "cancel_unconfirmed", detail: "2 survivors" });
  });

  test("terminal notices distinguish abandoned work and survivor proof", () => {
    assert.deepEqual(runNotice(run("lost", { lostReason: "abandoned", contact: unreconciled("Abandoned") })), { kind: "abandoned", host: "gpu-box" });
    assert.equal(runNotice(run("lost", { lostReason: "abandoned" })), null);
    assert.deepEqual(runNotice(run("cancelled", { contact: { type: "stopping", since: 300, count: 1 } })), { kind: "not_fully_stopped", count: 1, host: "gpu-box" });
  });

  test("the experiment notice comes from its newest run that has one", () => {
    const older = run("running", { id: "older", createdAt: 1, contact: unreachable });
    const newer = run("done", { id: "newer", createdAt: 2 });
    assert.equal(noticeRun([newer, older])?.id, "older");
    assert.equal(noticeRun([newer]), null);
  });
});

describe("log collection", () => {
  test("a finished run notes its log being saved, and a failure with its cause", () => {
    assert.deepEqual(runNotice(run("done", { logCollection: "pending" })), { kind: "log_pending", host: "gpu-box" });
    assert.deepEqual(runNotice(run("failed", { logCollection: "failed", logCollectionError: "disk full" })), { kind: "log_failed", host: "gpu-box", cause: "disk full" });
    assert.equal(runNotice(run("done", { logCollection: null })), null);
  });

  test("lost contact outranks log collection", () => {
    assert.equal(runNotice(run("lost", { lostReason: "abandoned", contact: unreconciled("Abandoned"), logCollection: "pending" })).kind, "abandoned");
  });
});

test("abandon is offered only for active runs we can't observe", () => {
  assert.equal(runAbandonable(run("running", { contact: unreachable })), true);
  assert.equal(runAbandonable(run("submitting", { contact: unreconciled("x") })), true);
  assert.equal(runAbandonable(run("running")), false);
  assert.equal(runAbandonable(run("lost", { contact: unreconciled("Abandoned") })), false);
});

test("submitting counts as active", () => {
  assert.deepEqual(["submitting", "starting", "running", "done", "lost"].map(runActive), [true, true, true, false, false]);
});

test("an abandoned run's result text is left to its notice", () => {
  const abandoned = run("lost", { lostReason: "abandoned", resultMarkdown: "Abandoned; the cancel stays pending until the job is confirmed gone." });
  assert.equal(runResult(abandoned), null);
  assert.equal(runResult(run("lost", { lostReason: "host rebooted", resultMarkdown: "host rebooted" })), "host rebooted");
  assert.equal(runResult(run("failed", { resultMarkdown: "exit 1" })), "exit 1");
  assert.equal(runResult(run("done", { resultMarkdown: "" })), null);
});

test("a cancelled tombstone renders its survivor proof until the latest proof is empty", () => {
  const pending = run("cancelled", { contact: { type: "stopping", since: 300, count: 2 } });
  assert.deepEqual(runDisplay(pending), { status: "not_fully_stopped", stopping: { count: 2, host: "gpu-box" }, stale: true, since: 300 });
  assert.deepEqual(runNotice(pending), { kind: "not_fully_stopped", count: 2, host: "gpu-box" });
  assert.equal(runAbandonable(pending), false);
  assert.deepEqual(runDisplay({ ...pending, contact: null }), { status: "cancelled", stale: false, since: null });
  assert.equal(runNotice({ ...pending, contact: null }), null);
});

test("unverifiable cancellation uses structured proof independently of editable result text", () => {
  const unconfirmed = run("cancelled", { cancelUnconfirmed: [], resultMarkdown: "Container processes couldn't be checked." });
  assert.equal(runDisplay(unconfirmed).status, "cancelled_unconfirmed");
  assert.deepEqual(runNotice(unconfirmed), { kind: "cancelled_unconfirmed", unreadable: [] });
  assert.equal(runDisplay({ ...unconfirmed, resultMarkdown: "Edited" }).status, "cancelled_unconfirmed");
  assert.equal(runDisplay({ ...unconfirmed, cancelUnconfirmed: null }).status, "cancelled");
});
