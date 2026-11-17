import assert from "node:assert/strict";
import test from "node:test";

import {
  queuedRetryLabel,
  recoveryAction,
  recoveryTurnOptions,
  retryStatusLabel,
  usageResetCountdown,
} from "../src/chatRecovery.ts";
import { m } from "../src/paraglide/messages.js";

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

test("usage-limit countdown formats the wait through the translated durations", () => {
  const now = 1_000_000_000_000;
  const minute = 60_000;
  // The countdown is the provider's wait rendered by the same translated
  // durations the rest of the UI uses, so rewording them never breaks this.
  assert.equal(usageResetCountdown(now + minute, now), m.duration_minutes({ value: "1" }));
  assert.equal(
    usageResetCountdown(now + 61 * minute, now),
    m.duration_hours_minutes({ hours: "1", minutes: "1" }),
  );
  assert.equal(
    usageResetCountdown(now + 2_777 * minute, now),
    m.duration_days_hours({ days: "1", hours: "22" }),
  );
});

test("usage-limit countdown disappears once the limit is back or unknown", () => {
  const now = 1_000_000_000_000;
  assert.equal(usageResetCountdown(now, now), null);
  assert.equal(usageResetCountdown(now - 1, now), null);
  assert.equal(usageResetCountdown(undefined, now), null);
  assert.equal(usageResetCountdown("soon", now), null);
  assert.equal(usageResetCountdown(Number.NaN, now), null);
});
