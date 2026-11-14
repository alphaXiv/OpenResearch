import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { fmtDuration } from "../src/api.ts";
import { fmtClock } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { runModel, sessionRunMark } from "../src/runMonitor/model.ts";

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
  tone: "running", whole: 0.248, headline: { name: "loss", label: "Loss", value: 2.31, vs: null }, sentence: running.sentence,
  bindings: { loss: 2.31 }, signals: running.view.signals, etaMs: { lo: 4_800_000, mid: 5_400_000, hi: 6_000_000, at: now }, pending: false, flag: null, rev: 1, generation: "g1", asOf: now, ...extra,
});
const model = (value, { monitor = null, shown = true, at = now, retry = null } = {}) => {
  const shaped = runModel({ item: { sessionId: "s", run: value, experimentName: "Rank 16", experimentSlug: "lr-3e-4" }, monitor, shown, agent: "Claude", now: at, nextRetryMs: retry });
  return { ...shaped, text: bare(shaped.sentence.text), eta: bare(shaped.eta), etaDetail: bare(shaped.etaDetail), progress: bare(shaped.progress), cost: bare(shaped.cost) };
};

test("against an older server a running run reads from its status alone", () => {
  const line = model(run(), { shown: false });
  assert.equal(line.tone, "running");
  assert.equal(line.text, bare(m.run_running_on({ where: "⁨Modal⁩ · ⁨A10G⁩" })));
  assert.equal(line.eta, bare(m.run_elapsed({ duration: fmtDuration(1_800_000) })));
  assert.equal(line.lead, null);
  assert.ok(line.canStop && !line.canDismiss);
  assert.deepEqual(line.ticks, []);
  assert.equal(model(run({ status: "starting" }), { shown: false }).tone, "starting");
});

test("a watched run fills the watcher's words and counts down from the server's estimate", () => {
  const line = model(run({ monitor: summary() }), { monitor: running, at: now + 600_000 });
  assert.equal(line.text, "Loss **2.31**, falling steadily.");
  assert.equal(line.sentence.key, `look:${running.sentence.template}`);
  assert.equal(line.eta, m.run_eta_left({ duration: fmtDuration(4_800_000) }));
  assert.equal(line.progress, `train · ${bare(m.run_progress_steps({ done: "1,240", of: "5,000" }))} · 25%`, "a named stage leads its progress");
  assert.deepEqual(line.ticks.map((tick) => [tick.at, tick.flag]), [[0.16, false]]);
  assert.ok(line.headline.inSentence);

  const fromSummary = model(run({ monitor: summary({ bindings: { loss: 2.2 } }) }));
  assert.equal(fromSummary.text, "Loss **2.20**, falling steadily.");
  assert.equal(fromSummary.eta, m.run_eta_left({ duration: fmtDuration(5_400_000) }));
});

test("a run with no total and a time limit counts to the limit", () => {
  const open = { ...running, phases: [{ ...running.phases[0], of: null }], estimate: null, limit: { endsAt: now + 3_600_000 }, rules: [{ id: "r", name: "plateau", text: "", does: "stop" }] };
  const line = model(run({ monitor: summary({ etaMs: null }) }), { monitor: open });
  assert.equal(line.eta, m.run_eta_to_limit({ duration: fmtDuration(3_600_000) }));
  assert.equal(line.etaDetail, m.run_eta_ends_by_rules({ clock: fmtClock(now + 3_600_000) }));
});

test("an estimate that needs more steps, or a stalled run, says so", () => {
  const pending = model(run(), { monitor: { ...running, estimate: { ...running.estimate, pending: true } } });
  assert.equal(pending.eta, m.run_eta_estimating());
  const stalled = model(run(), { monitor: { ...running, progress: { ...running.progress, stallMs: 180_000 } } });
  assert.equal(stalled.etaDetail, m.run_eta_stalled({ duration: fmtDuration(180_000) }));
});

test("a flag waiting on the user opens the choice; one the agent handles says it is looking", () => {
  const waiting = model(run({ monitor: summary({ tone: "flag", sentence: flagged.sentence, bindings: { loss: "NaN" }, flag: { id: "f1", state: "waiting", mode: "copilot" } }) }), { monitor: flagged });
  assert.equal(waiting.tone, "flag");
  assert.equal(waiting.text, "Loss is **NaN**.");
  assert.ok(waiting.needsChoice && !waiting.looking);
  assert.deepEqual(waiting.ticks.map((tick) => tick.flag), [false, true]);
  const looking = model(run({ monitor: summary({ tone: "flag", flag: { id: "f1", state: "agent_looking", mode: "agentic" } }) }));
  assert.ok(looking.looking && !looking.needsChoice);
});

test("an ended run leads with how it ended and offers Dismiss", () => {
  const ended = (extra, monitorTone) => model(run({ status: "cancelled", endedAt: now, ...extra, monitor: monitorTone ? summary({ tone: monitorTone, sentence: null, etaMs: null }) : null }), { monitor: { ...running, revising: monitorTone === "revising" } });
  const rule = { id: "nan", name: "nan-loss", text: "Loss is NaN.", does: "stop" };
  const auto = ended({ outcome: { by: "rule", rule, at: now } }, "stopped");
  assert.equal(auto.lead, m.status_auto_stopped());
  assert.equal(bare(auto.note), m.run_rule_detail({ text: rule.text, name: rule.name }));
  assert.equal(auto.eta, m.run_eta_stopped_at({ duration: fmtDuration(1_800_000) }));
  assert.ok(auto.canDismiss && !auto.canStop);

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

test("lost contact speaks in the contact notice's words and freezes the bar", () => {
  const since = now - 60_000;
  const away = model(run({ host: "lab-a40", contact: { type: "unreachable", since, cause: "timed out" } }), { monitor: { ...running, cost: { soFar: 1.06, projected: 4, atLimit: null, spend: null, budget: null, gpuHours: null, at: now, projectedRange: null, uncertain: false } }, retry: now + 180_000 });
  assert.equal(away.tone, "lost");
  assert.ok(away.text.startsWith(bare(m.run_notice_unreachable({ host: "lab-a40", time: `**${fmtClock(since)}**` }))), away.text);
  assert.ok(away.text.includes(bare(m.run_progress_last_seen({ progress: bare(m.run_progress_steps({ done: "1,240", of: "5,000" })) }))));
  assert.equal(away.eta, m.run_notice_retry_at({ time: fmtClock(now + 180_000) }));
  assert.ok(away.bar.frozen);
  assert.equal(away.cost, m.run_cost_last_seen({ cost: "$1.06" }));

  const auth = model(run({ host: "lab-a40", backend: { kind: "slurm_job" }, contact: { type: "needs_auth", since, cause: "key" } }));
  assert.equal(auth.connectHost, "lab-a40");
  assert.ok(auth.text.includes("**"), "the host is the bold figure");

  assert.equal(model(run({ cancelling: true })).text, m.status_cancelling());
  assert.equal(model(run({ status: "cancelled", contact: { type: "stopping", since, count: 2 } })).canDismiss, false);
});

test("a chat's sidebar row warns while a flag is unsettled, else shows the flask while a run runs", () => {
  const row = (sessionId, value) => ({ sessionId, run: value, experimentName: null });
  const flag = (state, mode) => run({ monitor: summary({ flag: { id: "f", state, mode } }) });
  assert.equal(sessionRunMark([row("a", run())], "a"), "running");
  assert.equal(sessionRunMark([row("a", run({ status: "done" }))], "a"), null);
  assert.equal(sessionRunMark([row("a", run()), row("a", flag("agent_looking", "agentic"))], "a"), "flagged");
  assert.equal(sessionRunMark([row("a", flag("waiting", "copilot"))], "a"), "waiting");
  assert.equal(sessionRunMark([row("b", flag("waiting", "copilot"))], "a"), null);
});

test("a summary newer than the monitor speaks alone, never beside the older revision's numbers", () => {
  const ahead = summary({
    rev: running.rev + 1, whole: 0.3, sentence: { template: "Loss **{loss}**, still falling.", t: now }, bindings: { loss: 1 },
    headline: { name: "loss", label: "Loss", value: 1, vs: null }, etaMs: { lo: 60_000, mid: 120_000, hi: 180_000, at: now },
  });
  const line = model(run({ monitor: ahead }), { monitor: running });
  assert.equal(line.text, "Loss **1.00**, still falling.");
  assert.equal(bare(line.headline.value), "1.00");
  assert.equal(line.eta, m.run_eta_left({ duration: fmtDuration(120_000) }));
  assert.equal(line.progress, "", "progress waits for the monitor of this revision");
  assert.deepEqual([line.bar.whole, line.ticks.length, line.cost], [0.3, 0, ""]);

  const rebuilt = model(run({ monitor: summary({ rev: 1, generation: "g2", asOf: now + 1000, bindings: { loss: 1.5 }, headline: { name: "loss", label: "Loss", value: 1.5, vs: null } }) }), { monitor: running });
  assert.equal(rebuilt.text, "Loss **1.50**, falling steadily.", "another generation is never mixed in either");
  assert.equal(bare(rebuilt.headline.value), "1.50");
  assert.equal(rebuilt.progress, "");
});

test("a monitor newer than the summary states every fact, the flag and the tone included", () => {
  const kept = { ...flagged, flag: { ...flagged.flag, state: "kept" } };
  const older = summary({ rev: kept.rev - 1, tone: "flag", sentence: flagged.sentence, bindings: { loss: "NaN" }, flag: { id: "f1", state: "waiting", mode: "copilot" } });
  const line = model(run({ monitor: older }), { monitor: kept });
  assert.equal(line.needsChoice, false, "the kept flag needs no choice");
  assert.equal(line.flag, null);
  assert.equal(line.tone, "running");

  const waiting = model(run({ monitor: older }), { monitor: { ...flagged, rev: kept.rev } });
  assert.ok(waiting.needsChoice && waiting.tone === "flag");
  assert.equal(waiting.flag.record.evidence.byteFrom, flagged.flag.evidence.byteFrom);

  const otherFlag = model(run({ monitor: summary({ rev: kept.rev + 1, tone: "flag", flag: { id: "f2", state: "waiting", mode: "copilot" } }) }), { monitor: flagged });
  assert.equal(otherFlag.flag.id, "f2");
  assert.equal(otherFlag.flag.record, null, "evidence shows only for the newest source's flag");
});

test("a view that is stale, rebuilding or unavailable reads like lost contact, each in its own words", () => {
  const reason = "The cluster hasn't answered.";
  const cost = { soFar: 1.06, projected: 4, atLimit: null, spend: null, budget: null, gpuHours: null, at: now, projectedRange: null, uncertain: false };
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

test("a queued run says what orx quoted: about how long, the range and what it goes by, and the cost", () => {
  const quoted = {
    ...running, progress: null, estimate: null, before: { kind: "queued", startsAt: null, text: null },
    quote: { loMs: 3 * 3_600_000, hiMs: 4 * 3_600_000, usd: [12, 16], spendCap: null, from: { kind: "earlier_run", runNo: 2, hardware: "A100", runId: "r0" } },
  };
  const line = model(run({ status: "submitting", host: "lab-a40" }), { monitor: quoted });
  assert.equal(line.eta, m.run_eta_about({ duration: fmtDuration(3.5 * 3_600_000) }));
  const source = bare(m.run_quote_from_earlier({ n: "2", hw: "A100" }));
  assert.equal(bare(line.etaDetail), m.run_quote_range({ lo: fmtDuration(3 * 3_600_000), hi: fmtDuration(4 * 3_600_000), source }));
  assert.equal(line.cost, m.run_cost_range({ lo: "$12.00", hi: "$16.00" }));
  assert.equal(line.progress, m.run_not_started());

  const unknown = model(run({ status: "submitting" }), { monitor: { ...quoted, quote: { loMs: null, hiMs: null, usd: null, spendCap: 5, from: { kind: "none" } } } });
  assert.equal(unknown.eta, m.run_eta_none());
  assert.equal(unknown.etaDetail, m.run_quote_none());
  assert.equal(unknown.cost, m.run_quote_spend_cap({ cost: "$5.00" }));
});

test("a run with no total and no limit shows elapsed time, says why there is no estimate and fills nothing", () => {
  const open = { ...running, phases: [{ ...running.phases[0], of: null }], estimate: null, limit: null, progress: { ...running.progress, whole: null, segments: [{ name: "train", weight: 1, fill: null }] } };
  const line = model(run(), { monitor: open, at: now });
  assert.equal(line.eta, bare(m.run_elapsed({ duration: fmtDuration(1_800_000) })));
  assert.equal(line.etaDetail, m.run_eta_no_total());
  assert.deepEqual(line.bar.segments, [{ weight: 1, fill: null }]);
});

test("a line without its monitor counts down to the time limit the summary carries", () => {
  const line = model(run({ monitor: summary({ etaMs: null, limitEndsAt: now + 3 * 3_600_000 }) }));
  assert.equal(line.eta, m.run_eta_to_limit({ duration: fmtDuration(3 * 3_600_000) }));
});

test("a run whose stop couldn't be confirmed can't be dismissed until it is cleaned up", () => {
  const unconfirmed = model(run({ status: "cancelled", endedAt: now, cancelUnconfirmed: [] }));
  assert.equal(unconfirmed.canDismiss, false);
  assert.equal(model(run({ status: "cancelled", endedAt: now })).canDismiss, true);
});

test("a run that hasn't started says why it waits, in its backend's words, until contact is lost or a stop is sent", () => {
  const queued = { ...running, progress: null, estimate: null, before: { kind: "queued", startsAt: null, text: null } };
  for (const [status, backend, reason, monitor] of [
    ["starting", { kind: "slurm_job" }, "Resources", queued],
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
  assert.equal(model(run({ status: "starting", startedAt: null })).text, bare(m.run_starting({ where: "⁨Modal⁩ · ⁨A10G⁩" })));
});
