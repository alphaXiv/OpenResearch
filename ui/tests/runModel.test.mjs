import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { fmtDuration } from "../src/api.ts";
import { fmtClock } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { historySentence, runModel, sessionRunMark } from "../src/runMonitor/model.ts";

const fixture = (name) => JSON.parse(readFileSync(new URL(`./fixtures/runMonitor/${name}.json`, import.meta.url), "utf8"));
const running = fixture("running");
const flagged = fixture("flagged");
const now = running.asOf;
const bare = (text) => text.replace(/[⁦-⁩]/g, "");
const run = (extra = {}) => ({
  id: "r1", experimentId: "e1", projectId: "p", status: "running", createdAt: now - 1_800_000, startedAt: now - 1_800_000, updatedAt: now,
  contact: null, cancelling: false, lostReason: null, host: null, backend: { kind: "modal_job", flavor: "A10G" }, ...extra,
});
const summary = (extra = {}) => ({
  tone: "running", whole: 0.248, headline: { name: "loss", label: "Loss", value: 2.31 }, sentence: { ...running.sentence, t: now },
  bindings: { loss: 2.31 }, signals: running.view.signals, estimate: { loMs: 4_800_000, midMs: 5_400_000, hiMs: 6_000_000, at: now, pending: false, floor: false }, flag: null, rev: 1, generation: "g1", asOf: now, ...extra,
});
const model = (value, { monitor = null, shown = true, at = now, retry = null } = {}) => {
  const shaped = runModel({ item: { sessionId: "s", run: value, experimentName: "Rank 16", experimentSlug: "lr-3e-4" }, monitor, shown, agent: "Claude", now: at, nextRetryMs: retry });
  return { ...shaped, text: bare(shaped.sentence.text), eta: bare(shaped.eta), etaDetail: bare(shaped.etaDetail), progress: bare(shaped.progress), cost: bare(shaped.cost) };
};

test("against an older server a running run reads from its status alone", () => {
  const line = model(run(), { shown: false });
  assert.equal(line.tone, "running");
  assert.equal(line.text, bare(m.status_running()));
  assert.equal(line.eta, bare(m.run_elapsed({ duration: fmtDuration(1_800_000) })));
  assert.equal(line.lead, null);
  assert.ok(line.canStop && !line.canDismiss);
  assert.deepEqual(line.ticks, []);
  assert.equal(model(run({ status: "starting" }), { shown: false }).tone, "starting");
});

test("a watched run fills the watcher's words and counts down from the server's estimate", () => {
  const line = model(run({ monitor: summary() }), { monitor: { ...running, sentence: { ...running.sentence, t: now + 600_000 } }, at: now + 600_000 });
  assert.equal(line.text, "train (2/2) · Step **1,240** of **5,000**. Loss **2.31**, falling steadily.");
  assert.equal(bare(line.sentence.watcher), "Loss **2.31**, falling steadily.");
  assert.equal(line.eta, m.run_eta_left({ duration: fmtDuration(4_800_000) }));
  assert.equal(line.progress, "25%", "the count stays on the status line");
  assert.deepEqual(line.ticks.map((tick) => [tick.at, tick.flag]), [[0.16, false]]);
  assert.ok(line.headline.inSentence);

  const fromSummary = model(run({ monitor: summary({ bindings: { loss: 2.2 } }) }));
  assert.equal(fromSummary.text, "Running. Loss **2.20**, falling steadily.");
  assert.equal(fromSummary.eta, m.run_eta_left({ duration: fmtDuration(5_400_000) }));
});

test("a run with no total and a time limit counts to the limit", () => {
  const open = { ...running, phases: [{ ...running.phases[0], of: null }], estimate: null, limit: { endsAt: now + 3_600_000 }, rules: [{ id: "r", name: "plateau", text: "", does: "stop" }] };
  const line = model(run({ monitor: summary({ estimate: null }) }), { monitor: open });
  assert.equal(line.eta, m.run_eta_to_limit({ duration: fmtDuration(3_600_000) }));
  assert.equal(line.etaDetail, m.run_eta_ends_by_rules({ clock: fmtClock(now + 3_600_000) }));
});

test("an estimate that needs more steps, or a stalled run, says so", () => {
  const pending = model(run(), { monitor: { ...running, estimate: { ...running.estimate, pending: true } } });
  assert.equal(pending.eta, m.run_eta_estimating());
  const stalled = model(run(), { monitor: { ...running, progress: { ...running.progress, stallMs: 180_000 } } });
  assert.equal(stalled.etaDetail, m.run_eta_stalled({ duration: fmtDuration(180_000) }));
});

test("a run that reports in chunks is almost done past its midpoint, and late only once it stalls or outlasts the high end", () => {
  const estimate = { ...running.estimate, loMs: 1_000, midMs: 1_700, hiMs: 4_000, pending: false, floor: false, pastLimitMs: null };
  const at = (ago, stallMs = 0) => model(run(), { monitor: { ...running, estimate: { ...estimate, at: now - ago }, progress: { ...running.progress, stallMs } } });
  const between = at(2_000);
  assert.equal(between.eta, m.run_eta_almost_done());
  assert.equal(between.etaDetail, "");
  assert.equal(between.leftMs, null);
  const stalled = at(2_000, 30_000);
  assert.equal(stalled.eta, m.run_eta_silent_for({ duration: fmtDuration(2_000) }));
  const overdue = at(5_000);
  assert.equal(overdue.eta, m.run_eta_silent_for({ duration: fmtDuration(5_000) }));
  assert.equal(overdue.etaDetail, m.run_eta_was_due({ clock: fmtClock(now - 5_000 + 1_700) }));
});

test("a flag waiting on the user opens the choice while one the agent handles keeps its state", () => {
  const waiting = model(run({ monitor: summary({ tone: "flag", sentence: flagged.sentence, bindings: { loss: "NaN" }, flag: { id: "f1", state: "waiting" } }) }), { monitor: flagged });
  assert.equal(waiting.tone, "flag");
  assert.equal(waiting.text, "Loss is **NaN**.");
  assert.ok(waiting.needsChoice);
  assert.deepEqual(waiting.ticks.map((tick) => tick.flag), [false, true]);
  const looking = model(run({ monitor: summary({ tone: "flag", flag: { id: "f1", state: "agent_looking", mode: "agentic" } }) }));
  assert.equal(looking.flag.state, "agent_looking");
  assert.ok(!looking.needsChoice);
});

test("an open flag reads the report that raised it while an ordinary sentence still refills", () => {
  const template = "Train rising to **{loss}**.";
  const saved = { ...flagged.flags[0], template, detail: template, text: "Train rising to **6.04**.", bindings: { loss: 6.04 } };
  const flag = { id: saved.id, state: "waiting", template, text: saved.text, bindings: saved.bindings };
  const sentence = { template: "Train is **{loss}** now.", t: now };
  const read = (loss, status, withFlag, full) => {
    const data = { ...flagged, latest: { loss }, sentence, flags: withFlag ? [saved] : [], flagId: withFlag ? saved.id : null };
    const runSummary = summary({ rev: flagged.rev, bindings: { loss }, sentence, flag: withFlag ? flag : null });
    return model(run({ status, monitor: runSummary }), { monitor: full ? data : null });
  };
  for (const full of [false, true]) {
    for (const status of ["running", "done"]) {
      const first = read(6.04, status, true, full);
      const after = read(5.438, status, true, full);
      assert.equal(after.text, "Train rising to **6.04**.");
      assert.equal(after.text, first.text);
      if (full) assert.equal(bare(after.flagDetail), after.text);
    }
    assert.match(read(5.438, "running", false, full).text, /Train is \*\*5\.44\*\* now\./);
  }
});

test("an ended run leads with how it ended and offers Dismiss", () => {
  const ended = (extra, monitorTone) => model(run({ status: "cancelled", endedAt: now, ...extra, monitor: monitorTone ? summary({ tone: monitorTone, sentence: null, estimate: null }) : null }), { monitor: { ...running, revising: monitorTone === "revising" } });
  const rule = { id: "nan", name: "nan-loss", text: "Loss is NaN.", does: "stop" };
  const auto = ended({ outcome: { by: "rule", rule, at: now } }, "stopped");
  assert.equal(auto.lead, m.status_auto_stopped());
  assert.equal(bare(auto.note), m.run_rule_detail({ text: rule.text, name: rule.name }));
  assert.equal(auto.eta, m.run_eta_stopped_at({ duration: fmtDuration(1_800_000) }));
  assert.ok(auto.canDismiss && !auto.canStop);

  const byAgent = ended({ outcome: { by: "agent", at: now, reason: "Loss went NaN at step 410." } }, "stopped");
  assert.equal(byAgent.lead, m.status_auto_stopped());
  assert.equal(bare(byAgent.note), m.run_stop_reason({ agent: "Claude", reason: "Loss went NaN at step 410." }));
  assert.equal(ended({ outcome: { by: "agent", at: now, reason: null } }, "stopped").note, m.run_rule_reason_in_chat());

  const byYou = ended({ outcome: { by: "user", at: now } }, "stopped");
  assert.equal(byYou.lead, m.run_lead_stopped());
  assert.equal(byYou.text, bare(m.run_auto_by_you({ progress: bare(m.run_progress_steps({ done: "1,240", of: "5,000" })) })));

  const revising = ended({ outcome: { by: "agent", at: now } }, "revising");
  assert.equal(revising.lead, m.status_revising());
  assert.equal(revising.note, m.run_flag_revising({ agent: "⁨Claude⁩" }));
  assert.ok(!revising.canStop && !revising.canDismiss);

  const done = model(run({ status: "done", endedAt: now }));
  assert.equal(done.lead, null);
  assert.equal(done.text, m.run_done_in({ duration: fmtDuration(1_800_000) }));
  assert.equal(done.eta, "");
  assert.deepEqual(done.bar.segments, [{ weight: 1, fill: 1 }]);

  const failed = model(run({ status: "failed", endedAt: now }));
  assert.equal(failed.lead, m.status_failed());
  assert.equal(failed.eta, m.run_failed_after({ duration: fmtDuration(1_800_000) }));
  assert.equal(model(run({ status: "done" }), { shown: false }).canDismiss, false);
});

test("a reporting error takes the lost tone, like lost contact", () => {
  const failing = model(run({ backend: { kind: "ssh_job", monitoringError: "The run's log directory cannot be read." } }), { monitor: running });
  assert.equal(failing.tone, "lost");
  assert.equal(failing.eta, "");
});

test("lost contact speaks in the contact notice's words and freezes the bar", () => {
  const since = now - 60_000;
  const away = model(run({ host: "lab-a40", contact: { type: "unreachable", since, cause: "timed out" } }), { monitor: { ...running, cost: { soFar: 1.06, projected: 4, spend: null, budget: null, at: now, uncertain: false } }, retry: now + 180_000 });
  assert.equal(away.tone, "lost");
  assert.ok(away.text.startsWith(bare(m.run_notice_unreachable({ host: "lab-a40", time: `**${fmtClock(since)}**` }))), away.text);
  assert.ok(away.text.includes(bare(m.run_progress_last_seen({ progress: bare(m.run_progress_steps({ done: "1,240", of: "5,000" })) }))));
  assert.ok(away.text.includes(m.run_notice_retry_at({ time: fmtClock(now + 180_000) })));
  assert.equal(away.eta, "");
  assert.ok(away.bar.frozen);
  assert.equal(away.cost, m.run_cost_last_seen({ cost: "$1.06" }));

  const auth = model(run({ host: "lab-a40", backend: { kind: "slurm_job" }, contact: { type: "needs_auth", since, cause: "key" } }));
  assert.ok(auth.text.includes("**"), "the host is the bold figure");

  assert.equal(model(run({ cancelling: true })).text, m.status_cancelling());
  assert.equal(model(run({ status: "cancelled", contact: { type: "stopping", since, count: 2 } })).canDismiss, false);
});

test("a chat's sidebar row warns while a flag is unsettled, else shows the flask while a run runs", () => {
  const row = (sessionId, value) => ({ sessionId, run: value, experimentName: null });
  const flag = (state) => run({ monitor: summary({ flag: { id: "f", state } }) });
  assert.equal(sessionRunMark([row("a", run())], "a"), "running");
  assert.equal(sessionRunMark([row("a", run({ status: "done" }))], "a"), null);
  assert.equal(sessionRunMark([row("a", run()), row("a", flag("agent_looking"))], "a"), "flagged");
  assert.equal(sessionRunMark([row("a", flag("waiting"))], "a"), "waiting");
  assert.equal(sessionRunMark([row("b", flag("waiting"))], "a"), null);
});

test("a summary newer than the monitor speaks alone, never beside the older revision's numbers", () => {
  const ahead = summary({
    rev: running.rev + 1, whole: 0.3, sentence: { template: "Loss **{loss}**, still falling.", t: now }, bindings: { loss: 1 },
    headline: { name: "loss", label: "Loss", value: 1 }, estimate: { loMs: 60_000, midMs: 120_000, hiMs: 180_000, at: now, pending: false, floor: false },
  });
  const line = model(run({ monitor: ahead }), { monitor: running });
  assert.equal(line.text, "Running. Loss **1.00**, still falling.");
  assert.equal(bare(line.headline.value), "1.00");
  assert.equal(line.eta, m.run_eta_left({ duration: fmtDuration(120_000) }));
  assert.equal(line.progress, "", "progress waits for the monitor of this revision");
  assert.deepEqual([line.bar.whole, line.ticks.length, line.cost], [0.3, 0, ""]);

  const rebuilt = model(run({ monitor: summary({ rev: running.rev + 1, generation: "g2", asOf: running.asOf, bindings: { loss: 1.5 }, headline: { name: "loss", label: "Loss", value: 1.5 } }) }), { monitor: running });
  assert.equal(rebuilt.text, "Running. Loss **1.50**, falling steadily.", "another generation is never mixed in either");
  assert.equal(bare(rebuilt.headline.value), "1.50");
  assert.equal(rebuilt.progress, "");
});

test("a monitor newer than the summary states every fact, the flag and the tone included", () => {
  const kept = { ...flagged, flagId: null, flags: [{ ...flagged.flags[0], state: "kept" }] };
  const older = summary({ rev: kept.rev - 1, tone: "flag", sentence: flagged.sentence, bindings: { loss: "NaN" }, flag: { id: "f1", state: "waiting" } });
  const line = model(run({ monitor: older }), { monitor: kept });
  assert.equal(line.needsChoice, false, "the kept flag needs no choice");
  assert.equal(line.flag, null);
  assert.equal(line.tone, "running");

  const waiting = model(run({ monitor: older }), { monitor: { ...flagged, rev: kept.rev } });
  assert.ok(waiting.needsChoice && waiting.tone === "flag");
  assert.equal(waiting.flag.record.evidence.byteFrom, flagged.flags[0].evidence.byteFrom);

  const otherFlag = model(run({ monitor: summary({ rev: kept.rev + 1, tone: "flag", flag: { id: "f2", state: "waiting" } }) }), { monitor: flagged });
  assert.equal(otherFlag.flag.id, "f2");
  assert.equal(otherFlag.flag.record, null, "evidence shows only for the newest source's flag");
});

test("a view that is stale, rebuilding or unavailable reads like lost contact, each in its own words", () => {
  const reason = "The cluster hasn't answered.";
  const cost = { soFar: 1.06, projected: 4, spend: null, budget: null, at: now, uncertain: false };
  const words = new Set();
  for (const state of ["stale", "rebuilding", "unavailable"]) {
    const old = { ...running, cost, availability: { state, reason, omitted: {} } };
    const first = model(run(), { monitor: old, at: now + 60_000 });
    assert.equal(first.eta, "", `${state} says no time left`);
    assert.equal(first.etaDetail, "", `${state} says no finish time`);
    assert.equal(first.etaStrong, false);
    assert.equal(first.progress, bare(m.run_progress_last_seen({ progress: bare(m.run_progress_steps({ done: "1,240", of: "5,000" })) })));
    assert.equal(first.cost, m.run_cost_last_seen({ cost: "$1.06" }));
    assert.ok(first.bar.frozen, state);
    assert.equal(bare(first.stale.age), m.status_last_seen({ time: fmtClock(running.asOf) }));
    assert.ok(bare(first.stale.why).endsWith(reason), state);
    words.add(first.stale.why);
  }
  assert.equal(words.size, 3, "each state has its own words");
  assert.equal(model(run(), { monitor: running }).stale, null);
  const failed = runModel({ item: { sessionId: "s", run: run(), experimentName: null }, monitor: running, monitorFailed: true, shown: true, agent: "Claude", now: now + 60_000, nextRetryMs: null });
  assert.ok(failed.bar.frozen && failed.stale && failed.eta === "", "a failed refresh is old data too");
  assert.ok(!words.has(failed.stale.why));
});

test("finalizing an ended run is quiet during grace and never says when it was last seen", () => {
  const ended = run({ status: "done", endedAt: now });
  const pending = { state: "stale", reason: null };
  const delayed = { ...pending, reason: "The final log is still being read." };
  const read = (availability, full) => model({ ...ended, monitor: summary({ availability }) }, {
    monitor: full ? { ...running, availability } : null,
  });
  for (const full of [true, false]) {
    assert.equal(read(pending, full).stale, null);
    const slow = read(delayed, full);
    assert.equal(bare(slow.stale.why), delayed.reason);
    assert.equal(slow.stale.age, null);
    assert.match(slow.text, /^Done in /);
  }
});

test("a run with no total and no limit shows elapsed time, says why there is no estimate and fills nothing", () => {
  const open = { ...running, phases: [{ ...running.phases[0], of: null }], estimate: null, limit: null, progress: { ...running.progress, whole: null, segments: [{ weight: 1, fill: null }] } };
  const line = model(run(), { monitor: open, at: now });
  assert.equal(line.eta, bare(m.run_elapsed({ duration: fmtDuration(1_800_000) })));
  assert.equal(line.etaDetail, "");
  assert.deepEqual(line.bar.segments, [{ weight: 1, fill: null }]);
});

test("a line without its monitor counts down to the time limit the summary carries", () => {
  const line = model(run({ monitor: summary({ estimate: null, limitEndsAt: now + 3 * 3_600_000 }) }));
  assert.equal(line.eta, m.run_eta_to_limit({ duration: fmtDuration(3 * 3_600_000) }));
});

test("a run whose stop couldn't be confirmed can't be dismissed until it is cleaned up", () => {
  const unconfirmed = model(run({ status: "cancelled", endedAt: now, cancelUnconfirmed: [] }));
  assert.equal(unconfirmed.canDismiss, false);
  assert.equal(model(run({ status: "cancelled", endedAt: now })).canDismiss, true);
});

test("a run that hasn't started says why it waits, in its backend's words, until contact is lost or a stop is sent", () => {
  const starting = { ...running, progress: null, estimate: null };
  for (const [status, backend, reason, monitor] of [
    ["starting", { kind: "slurm_job" }, "Resources", starting],
    ["starting", { kind: "k8s_job" }, "0/4 nodes are available: 4 Insufficient nvidia.com/gpu.", null],
    ["submitting", { kind: "openresearch_job" }, "Provisioning box", null],
    ["submitting", { kind: "modal_job", flavor: "A10G" }, "Waiting for a GPU", null],
  ]) {
    const value = run({ status, backend, host: "lab-a40", startedAt: null, waitingReason: reason });
    const line = model(value, { monitor });
    assert.ok(line.pre);
    assert.equal(line.text, `${status === "starting" ? m.status_starting() : m.status_submitting()} · ${reason}`);
    assert.ok(line.canStop, "Stop is offered while it waits");

    const lost = model({ ...value, contact: { type: "unreachable", since: now - 60_000, cause: "timed out" } }, { monitor });
    assert.ok(!lost.text.includes(reason), lost.text);
    const stopping = model({ ...value, cancelling: true }, { monitor });
    assert.ok(!stopping.text.includes(reason), stopping.text);
    assert.equal(stopping.canStop, false, "the pending cancel is said in Stop's place");
  }
  assert.equal(model(run({ status: "starting", startedAt: null })).text, m.status_starting());
  const startingRun = run({ status: "starting", startedAt: null, host: "lab-a40" });
  const waiting = model(startingRun, { monitor: starting });
  assert.equal(waiting.text, m.status_starting());
  assert.equal(waiting.eta, m.run_eta_none());
  assert.equal(waiting.progress, m.run_not_started());
});

test("a kept flag retains its record and falls through to the pace note", () => {
  const sentence = { template: "Test accuracy **{loss}**, far below the parent.", t: now - 60_000 };
  const kept = { ...running, sentence, pace: { than: "parent", times: 2.5 }, flags: [{ id: "f1", at: now - 60_000, whole: 0.2, template: sentence.template, detail: sentence.template, evidence: null,
    state: "kept", source: "look", lookId: "4", choice: { by: "user", c: "keep", at: now - 30_000 } }] };
  const line = model(run({ monitor: summary({ sentence }) }), { monitor: kept });
  assert.equal(line.name, "Rank 16");
  assert.equal(line.note, m.run_pace_slower_parent({ times: "⁦2.5⁩" }));
  assert.equal(kept.flags[0].state, "kept");
  const rewritten = { ...kept, sentence: { template: "Loss **{loss}**, falling.", t: now } };
  assert.equal(model(run({ monitor: summary({ sentence: rewritten.sentence }) }), { monitor: rewritten }).note, line.note);
});

test("a plan the run reports only as rows says the plan and the run disagree, live and once ended", () => {
  const disagrees = { stage: "bench", of: 16, unit: "cases", table: "cases", rows: 20 };
  const said = m.run_plan_disagrees({ of: "16", unit: "cases", stage: "bench", rows: "20" });
  assert.equal(bare(model(run(), { monitor: { ...running, disagrees } }).note), said);
  assert.equal(bare(model(run({ status: "done", endedAt: now }), { monitor: { ...running, disagrees } }).note), said);
  assert.equal(model(run(), { monitor: running }).note, null);
});

test("watcher words expire after three minutes while completion always names its duration", () => {
  const fresh = { ...running, sentence: { template: "Loss **{loss}**, falling steadily.", t: now } };
  assert.equal(model(run(), { monitor: fresh, at: now + 180_000 }).sentence.at, now);
  const expired = model(run(), { monitor: fresh, at: now + 180_001 });
  assert.equal(expired.sentence.at, null);
  assert.ok(expired.text.includes("1,240"));
  assert.ok(model(run({ status: "done", endedAt: now }), { monitor: fresh }).text.startsWith("Done in"));
});

test("an unresolved flag keeps its warning when the run ends, even after ordinary watcher text expires", () => {
  const ended = model(run({ status: "done", endedAt: now }), { monitor: flagged, at: now + 180_001 });
  assert.equal(ended.tone, "flag");
  assert.equal(bare(ended.text), "Loss is **NaN**.");
  assert.equal(bare(ended.sentence.watcher), bare(ended.text));
  assert.equal(ended.eta, m.run_done_in({ duration: fmtDuration(1_800_000) }));
});

test("a live run's subsecond estimate never reads zero seconds left", () => {
  const estimate = { ...running.estimate, loMs: 500, midMs: 500, hiMs: 500, at: now, pending: false, floor: false };
  const line = model(run(), { monitor: { ...running, estimate } });
  assert.equal(line.eta, bare(m.run_eta_left({ duration: fmtDuration(1000) })));
  assert.equal(line.leftMs, 500);
});

test("a run still exiting after its last reported step reads Finishing", () => {
  const estimate = { ...running.estimate, loMs: 0, midMs: 0, hiMs: 0, at: now, pending: false, floor: false };
  const stage = { ...running.phases[0], name: "", done: 5000, of: 5000 };
  const stageOrder = { ...running.stageOrder, count: null };
  const monitor = { ...running, estimate, stageOrder, phases: [stage], progress: { ...running.progress, done: 5000 } };
  const row = run({ monitor: summary({ estimate, stageOrder, stage }) });
  for (const full of [null, monitor]) {
    const line = model(row, { monitor: full });
    assert.match(line.text, /^Finishing · Step \*\*5,000\*\* of \*\*5,000\*\*/);
    assert.match(line.eta, /elapsed$/);
    assert.ok(line.canStop && !line.canDismiss);
  }
  const moreStages = { ...monitor, stageOrder: { ...stageOrder, count: 2 } };
  assert.equal(model(run(), { monitor: moreStages }).status, m.status_running());
});

test("setup keeps the declared stage name and reads as starting as the estimate passes", () => {
  const data = { ...running, setup: { line: null, startsAt: now + 500 }, sentence: null, progress: null,
    stageOrder: { index: 0, count: 2, first: 1 }, phases: [{ name: "preTrain", of: null, unit: null }] };
  const before = model(run(), { monitor: data });
  assert.match(before.text, /preTrain started/);
  assert.equal(before.eta, "Run starts in about 1s");
  assert.equal(model(run(), { monitor: data, at: now + 500 }).eta, "Run is starting");
});

test("setup keeps the declared stage ahead of log chatter and rejects structured fallbacks", () => {
  for (const line of ['"window_pattern": "L"', 'batch_size=16', '{"model": "tiny"}', 'Downloading weights']) {
    const setup = { line, startsAt: null };
    const data = { ...running, setup, sentence: null, progress: null, stageOrder: { index: 0, count: 2, first: 1 }, phases: [{ name: "install", of: null, unit: null }] };
    assert.match(model(run(), { monitor: data }).text, /install started/);
    const counted = { ...data, progress: { ...running.progress, phase: 0, done: 1 }, phases: [{ ...data.phases[0], of: 3, unit: "packages" }] };
    const words = model(run(), { monitor: counted }).text;
    assert.match(words, /install .*1.*of.*3.*packages/);
    assert.ok(!words.includes(line));
    const unnamed = { ...data, phases: [], stageOrder: { index: null, count: null, first: null } };
    assert.equal(model(run(), { monitor: unnamed }).text.includes(line), line === "Downloading weights");
    const short = summary({ setup, sentence: null, stage: { name: "install", of: null, done: null }, stageOrder: data.stageOrder });
    assert.match(model(run({ monitor: short })).text, /install started/);
  }
});

test("a failed run says why in the words the CLI and the agent's notice use, never a blank", () => {
  const ended = { status: "failed", endedAt: now, monitor: summary({ tone: "failed", sentence: null, stage: { name: "label", done: 2543, of: 5086, unit: "clips" } }) };
  const logged = model(run({ ...ended, exitCode: 1, monitor: { ...ended.monitor, logError: "ImportError: cannot import name 'Parakeet'" } }));
  assert.equal(logged.text, "ImportError: cannot import name 'Parakeet'");
  assert.equal(logged.note, null, "the reason is said once");
  assert.equal(logged.eta, m.run_failed_after({ duration: fmtDuration(1_800_000) }));

  const said = model(run({ ...ended, exitCode: 1, resultMarkdown: "Killed by   signal 9." }));
  assert.equal(said.text, "Killed by signal 9.");

  const quiet = model(run({ ...ended, exitCode: 1 }));
  assert.equal(quiet.text, bare(`${m.run_failed_exit_code({ code: "1" })} ${m.run_auto_at({ progress: "**2,543** of **5,086** clips" })}`));
  assert.equal(model(run({ status: "failed", endedAt: now, startedAt: null })).text, m.run_failed_no_reason());

  for (const failed of [run({ ...ended, exitCode: 1 }), run({ ...ended, exitCode: 1, monitor: { ...ended.monitor, logError: "ValueError: bad shape" } })]) {
    assert.equal(bare(historySentence(failed, now)), model(failed).text.replaceAll("**", ""), "history says what the line says");
  }
});

test("a run that never started runs no clock: no time since it was created, and no stage it failed in", () => {
  const reason = "There was no L40S free to rent. Try another GPU, or launch again.";
  const empty = summary({ tone: "failed", sentence: null, whole: null, headline: null, bindings: {}, stageOrder: { index: null, count: 4, first: null } });
  const failed = { status: "failed", startedAt: null, createdAt: now - 13_000, endedAt: now, resultMarkdown: reason, monitor: empty };
  const never = model(run(failed));
  assert.ok(never.neverStarted);
  assert.equal(never.text, reason);
  assert.equal(never.eta, m.run_not_started());
  assert.ok(!model(run({ ...failed, resultMarkdown: null })).neverStarted, "no recorded failure reason");
  assert.ok(!model(run({ ...failed, monitor: { ...empty, logError: "Worker exited" } })).neverStarted, "the log recorded an error");
  assert.ok(!model(run({ ...failed, monitor: { ...empty, bindings: { loss: 0 } } })).neverStarted, "the run reported a metric");
  assert.ok(!model(run({ status: "failed", startedAt: null, endedAt: now, exitCode: 1 })).neverStarted, "a run that exited ran, whether or not it reported a start");
  assert.ok(!model(run({ status: "starting", startedAt: null })).neverStarted);
});
