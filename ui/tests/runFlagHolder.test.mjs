import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { act, createElement } from "react";
import { RunBanner } from "../src/components/run/RunBanner.tsx";
import { m } from "../src/paraglide/messages.js";
import { flagOutcome } from "../src/runMonitor/journal.ts";
import { createWorkspace } from "../src/workspace.tsx";
import { render, serve, settle } from "./render.mjs";

const flagged = JSON.parse(readFileSync(new URL("./fixtures/runMonitor/flagged.json", import.meta.url), "utf8"));
const now = Date.now();

/** Chat s's banner over run r1, its flag in `state`, served by today's server; every flag request is kept. */
async function strip(state, extra = {}) {
  const choices = [];
  const monitor = { ...flagged, flags: [{ ...flagged.flags[0], state }] };
  serve((path, init) => {
    const request = `${init.method ?? "GET"} ${path}`;
    if (request === "GET /api/settings/ui-state") return { onboardingCompleted: true };
    if (request === "GET /api/compute/hosts") return [];
    if (request === "GET /api/runs/r1/monitor") return monitor;
    if (request === "POST /api/runs/r1/flag") {
      choices.push(JSON.parse(init.body));
      return { flag: { ...flagged.flags[0], state: "kept" }, run: run };
    }
  });
  const run = {
    id: "r1", experimentId: "e-r1", projectId: "p", chatSessionId: "s", status: "running", createdAt: now - 60_000, startedAt: now - 60_000,
    updatedAt: now, contact: null, cancelling: false, lostReason: null, host: null, backend: { kind: "local_job" },
    monitor: { tone: "flag", whole: 0.25, headline: null, sentence: flagged.sentence, bindings: { loss: "NaN" }, signals: [], estimate: null,
      flag: { id: "f1", state }, rev: flagged.rev, generation: "g1", asOf: now },
    ...extra,
  };
  const workspace = createWorkspace("test");
  workspace.queryClient.setQueryData(workspace.queries.listChatShownRunsQuery().queryKey, {
    source: "shown", runs: [{ sessionId: "s", run, experimentName: "r1", experimentSlug: "lr-r1", runNo: 1, runCount: 1 }],
  });
  const view = await render(createElement(RunBanner, { sessionId: "s", agent: "Claude", onOpenRunPage: () => {} }), workspace);
  await settle();
  return { ...view, choices };
}

const plain = (text) => text.replace(/[⁦-⁩]/g, "");
const named = (label) => [...document.querySelectorAll("button")].find((element) => plain(element.textContent) === plain(label)) ?? null;
const click = async (element) => { await act(async () => element.click()); await settle(); };

test("while the agent has a flag, its strip says so and the user can still keep or stop the run quietly", async () => {
  for (const [state, status] of [
    ["raised", m.chat_queued()],
    ["waking", m.run_flag_waking({ agent: "Claude" })],
    ["agent_looking", m.run_flag_reading({ agent: "Claude" })],
  ]) {
    const { container, choices, unmount } = await strip(state);
    assert.ok(plain(container.textContent).includes(plain(status)), state);
    assert.equal(named(m.run_flag_ask({ agent: "Claude" })), null, `${state}: the agent already has it`);
    assert.ok(!plain(container.textContent).includes(m.run_flag_until_you_choose()), state);
    assert.ok(named(m.experiments_stop_run()), state);
    await click(named(m.run_flag_keep_going()));
    assert.deepEqual(choices.map((choice) => choice.choice), ["keep"], state);
    await unmount();
  }
  const { choices } = await strip("agent_looking");
  await click(named(m.experiments_stop_run()));
  assert.deepEqual(choices, [], "Stop asks first");
  const confirm = [...document.querySelectorAll('[role="dialog"] button')].find((element) => plain(element.textContent) === plain(m.experiments_stop_run()));
  await click(confirm);
  assert.deepEqual(choices.map((choice) => choice.choice), ["stop"]);
});

test("while the agent reads an ended run's flag, the user can keep its result, with nothing to stop", async () => {
  const { container } = await strip("agent_looking", { status: "done", endedAt: now });
  assert.ok(plain(container.textContent).includes(plain(m.run_flag_reading({ agent: "Claude" }))));
  assert.ok(named(m.run_flag_keep_result()));
  assert.equal(named(m.experiments_stop_run()), null);
});

test("a Copilot flag queued for review only shows the status", async () => {
  const { container } = await strip("asking");
  assert.equal(container.querySelector('[data-run-frame] [role="status"]').textContent, m.chat_queued());
  assert.equal(named(m.run_flag_keep_going()), null);
  assert.equal(named(m.experiments_stop_run()), null);
  assert.equal(named(m.run_flag_ask({ agent: "Claude" })), null);
});

test("a flag that waits for the user offers Stop, Keep and Ask", async () => {
  const { container, choices } = await strip("waiting");
  assert.ok(plain(container.textContent).includes(m.run_flag_until_you_choose()));
  assert.ok(named(m.experiments_stop_run()));
  assert.ok(named(m.run_flag_keep_going()));
  await click(named(m.run_flag_ask({ agent: "Claude" })));
  assert.equal(choices[0].choice, "ask");
});

test("a flag the agent left unsettled says it was kept for that reason", () => {
  const kept = { ...flagged.flags[0], state: "kept", choice: { by: "agent", c: "keep", at: now, reason: null, unsettled: true } };
  assert.equal(plain(flagOutcome(kept, "Claude")), m.run_watch_kept_unsettled({ agent: "Claude" }));
  assert.equal(plain(flagOutcome({ ...kept, choice: { ...kept.choice, unsettled: undefined } }, "Claude")), m.run_watch_kept_by({ agent: "Claude" }));
});
