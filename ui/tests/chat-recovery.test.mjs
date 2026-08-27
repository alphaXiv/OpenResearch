import assert from "node:assert/strict";
import test from "node:test";

import {
  queuedRetryLabel,
  recoveryAction,
  recoveryTurnOptions,
  retryStatusLabel,
} from "../src/chatRecovery.ts";

test("formats ORX retry attempts and countdowns", () => {
  assert.equal(
    retryStatusLabel(
      { retryOwner: "orx", attempt: 2, maximum: 4, nextRetryAt: 13_000 },
      10_100,
    ),
    "Retrying · attempt 2/4 · next attempt in 3s",
  );
});

test("native retries without timing use the compact CLI label", () => {
  assert.equal(retryStatusLabel({ retryOwner: "native", attempt: 1 }, 0), "CLI is retrying…");
  assert.equal(retryStatusLabel({ retryOwner: "orx", attempt: 2 }, 0), "Retrying · attempt 2");
});

test("formats queued delivery countdowns in plain language", () => {
  assert.equal(queuedRetryLabel(13_000, 10_100), "Sending again in 3s…");
  assert.equal(queuedRetryLabel(null, 0), "Sending again…");
});

test("only the two safe recovery actions are accepted", () => {
  assert.equal(recoveryAction("retry"), "retry");
  assert.equal(recoveryAction("continue"), "continue");
  assert.equal(recoveryAction("replay"), null);
});

test("recovery sends only composer axes changed after the failed turn", () => {
  assert.deepEqual(
    recoveryTurnOptions({
      model: undefined,
      serviceTier: "priority",
      permissionMode: "auto",
      planMode: false,
    }),
    { serviceTier: "priority", permissionMode: "auto", planMode: false },
  );
  assert.deepEqual(recoveryTurnOptions({ model: null }), { model: null });
  assert.deepEqual(recoveryTurnOptions({}), {});
});
