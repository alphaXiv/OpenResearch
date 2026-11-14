import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { m } from "../src/paraglide/messages.js";
import { runModel } from "../src/runMonitor/model.ts";
import { shortLabels, summarize } from "../src/runMonitor/summary.ts";

const sweep = ["Depth 4 at 2e15 FLOPs", "Depth 6 at 2e15 FLOPs", "Depth 8 at 2e15 FLOPs"];

for (const [titles, labels, context] of [
  [sweep, ["Depth 4", "Depth 6", "Depth 8"], "… at 2e15 FLOPs"],
  // One unrelated title keeps its whole; the rest keep their labels, and no context claims to cover it.
  [[...sweep, "Tokenizer check"], ["Depth 4", "Depth 6", "Depth 8", "Tokenizer check"], null],
  [["LR 3e-4, warmup 200 steps", "LR 1e-3, warmup 200 steps"], ["LR 3e-4", "LR 1e-3"], "… warmup 200 steps"],
  [["nanochat d20 lr 3e-4", "nanochat d20 lr 1e-3"], ["lr 3e-4", "lr 1e-3"], "nanochat d20 …"],
  [["Ablation: no dropout", "Ablation: no weight decay"], ["no dropout", "no weight decay"], "Ablation: …"],
  [["Baseline (depth 4)", "Baseline (depth 12)"], ["(depth 4)", "(depth 12)"], "Baseline …"],
  [["Dropout 10% at 2e15 FLOPs", "Dropout 20% at 2e15 FLOPs"], ["Dropout 10%", "Dropout 20%"], "… at 2e15 FLOPs"],
  [["Глубина 4 при 2e15 FLOPs", "Глубина 6 при 2e15 FLOPs"], ["Глубина 4", "Глубина 6"], "… при 2e15 FLOPs"],
  [["العمق 4 عند 2e15", "العمق 6 عند 2e15"], ["العمق 4", "العمق 6"], "… عند 2e15"],
  // Written without spaces there are no whole words to take off.
  [["深さ4で学習", "深さ6で学習"], ["深さ4で学習", "深さ6で学習"], null],
  [["深さ 4 で学習", "深さ 6 で学習"], ["深さ 4", "深さ 6"], "… で学習"],
  // Nothing shared but the word that names what varies.
  [["Depth 4", "Depth 6"], ["Depth 4", "Depth 6"], null],
  [["Baseline", "Baseline with dropout"], ["Baseline", "Baseline with dropout"], null],
  // Two runs of one experiment share its title, and say it whole.
  [["Depth 4 at 2e15 FLOPs", "Depth 4 at 2e15 FLOPs"], ["Depth 4 at 2e15 FLOPs", "Depth 4 at 2e15 FLOPs"], null],
  [[...sweep, sweep[0]], ["Depth 4", "Depth 6", "Depth 8", "Depth 4"], "… at 2e15 FLOPs"],
  // Two families, each its own.
  [["Depth 4 at 2e15 FLOPs", "Depth 6 at 2e15 FLOPs", "LR 3e-4 on SQuAD", "LR 1e-3 on SQuAD"], ["Depth 4", "Depth 6", "LR 3e-4", "LR 1e-3"], null],
  [["depth-4", "depth-6"], ["depth-4", "depth-6"], null],
]) {
  test(`labels for ${JSON.stringify(titles)}`, () => {
    const named = shortLabels(titles);
    assert.deepEqual({ labels: named.labels, context: named.context }, { labels, context });
  });
}

const fixture = (name) => JSON.parse(readFileSync(new URL(`./fixtures/runMonitor/${name}.json`, import.meta.url), "utf8"));
const running = fixture("running");
const now = running.asOf;
const bare = (text) => text.replace(/[⁦-⁩]/g, "");
const signals = { val_bpb: { label: "val bpb", digits: 2, better: "lower" } };
const summary = (extra = {}) => ({
  tone: "running", whole: 0.4, headline: null, sentence: null, bindings: {}, signals: {}, etaMs: null, pending: false, flag: null,
  rev: 1, generation: "g1", asOf: now, limitEndsAt: null, ...extra,
});
const scored = (value, extra = {}) => summary({ headline: { name: "val_bpb", label: "val bpb", value, vs: null }, signals, ...extra });
const model = (id, monitor, extra = {}) => runModel({
  item: { sessionId: "s", experimentName: id, run: { id, experimentId: id, projectId: "p", status: "running", createdAt: now - 600_000, startedAt: now - 600_000, updatedAt: now, contact: null, cancelling: false, host: null, backend: { kind: "local_job" }, monitor, ...extra } },
  monitor: null, shown: true, agent: "Claude", now, nextRetryMs: null,
});
/** Titles that differ by one setting: a label group. */
const depths = (...depths) => depths.map((depth) => `Depth ${depth} at 2e15 FLOPs`);
const said = (models, titles = models.map((item) => item.run.id)) => {
  const { parts, firstDone } = summarize(models, shortLabels(titles));
  return { text: bare(parts.map((part) => [part.who, part.text].filter(Boolean).join(" ")).join(" · ")), firstDone: firstDone && bare(firstDone) };
};

test("runs that differ by one setting and share a headline that says which way is better name the leader, its number bound to its value", () => {
  const models = [model("a", scored(1.31)), model("b", scored(1.21)), model("c", scored(1.27))];
  assert.equal(said(models, depths(4, 6, 8)).text, "Depth 6 leads at val bpb **1.21**");
  const higher = { acc: { label: "accuracy", digits: 2, better: "higher" } };
  const accurate = (value) => summary({ headline: { name: "acc", label: "accuracy", value, vs: null }, signals: higher });
  assert.equal(said([model("a", accurate(0.5)), model("b", accurate(0.75))], ["LR 3e-4 on SQuAD", "LR 1e-3 on SQuAD"]).text, "LR 1e-3 leads at accuracy **0.75**");
});

test("runs lead only within one label group; unrelated runs are counted", () => {
  const count = (n) => bare(m.run_summary_running({ count: `**${n}**` }));
  // Unrelated titles share no setting to rank.
  assert.equal(said([model("a", scored(1.3)), model("b", scored(1.2))]).text, count(2));
  // An unrelated run with the same headline neither leads nor stops the sweep from leading.
  assert.equal(said([model("a", scored(1.3)), model("b", scored(1.2)), model("c", scored(0.9))], [...depths(4, 6), "Tokenizer check"]).text, "Depth 6 leads at val bpb **1.20**");
  // Two sweeps that could each lead: the line can't say both, so it counts.
  const two = [model("a", scored(1.3)), model("b", scored(1.2)), model("c", scored(1.1)), model("d", scored(1.0))];
  assert.equal(said(two, [...depths(4, 6), "LR 3e-4 on SQuAD", "LR 1e-3 on SQuAD"]).text, count(4));
  // Two runs of one experiment are one title, not a group.
  assert.equal(said([model("a", scored(1.3)), model("b", scored(1.2))], depths(4, 4)).text, count(2));
});

test("once every run has finished the leader is said with that", () => {
  const done = { status: "done", endedAt: now };
  const models = [model("a", scored(1.3, { tone: "done" }), done), model("b", scored(1.2, { tone: "done" }), done)];
  assert.equal(said(models, depths(4, 6)).text, "Depth 6 leads at val bpb **1.20**; all finished");
});

test("no leader without a declared direction, a shared headline, or two numbers", () => {
  const undeclared = { val_bpb: { label: "val bpb", digits: 2 } };
  const progress = bare(m.run_summary_running({ count: "**2**" }));
  assert.equal(said([model("a", scored(1.3, { signals: undeclared })), model("b", scored(1.2, { signals: undeclared }))], depths(4, 6)).text, progress);
  const other = summary({ headline: { name: "loss", label: "loss", value: 2, vs: null }, signals: { loss: { label: "loss", better: "lower" } } });
  assert.equal(said([model("a", scored(1.3)), model("b", other)], depths(4, 6)).text, progress);
  assert.equal(said([model("a", scored(1.3)), model("b", scored("NaN"))], depths(4, 6)).text, progress);
  // A run with no headline yet neither leads nor stops the others from leading.
  assert.equal(said([model("a", scored(1.3)), model("b", scored(1.2)), model("c", summary())], depths(4, 6, 8)).text, "Depth 6 leads at val bpb **1.20**");
});

test("who needs attention comes first, before any leader", () => {
  const failed = model("c", scored(1.0, { tone: "failed" }), { status: "failed", endedAt: now });
  const models = [model("a", scored(1.3)), model("b", scored(1.2)), failed];
  // A run that ended with nothing more to say says how long it took.
  assert.equal(said(models).text, bare(m.run_summary_attention_ended({ name: "c", ended: m.run_failed_after({ duration: "10m" }) })));
  const choice = model("d", summary({ tone: "flag", flag: { id: "f", state: "waiting", mode: "copilot" }, sentence: { template: "Loss went to **NaN**.", t: now } }));
  assert.match(said([...models, choice]).text, /^d · Flagged: Loss went to \*\*NaN\*\*\.$/);
  const lost = model("e", summary(), { contact: { type: "unreachable", since: now - 60_000, cause: "timed out" }, host: "lab-a40", backend: { kind: "ssh_job" } });
  assert.match(said([model("a", scored(1.3)), lost]).text, /^e · /);
  const byUser = model("f", summary({ tone: "stopped" }), { status: "cancelled", endedAt: now, outcome: { by: "user", at: now } });
  const byRule = model("g", summary({ tone: "stopped" }), { status: "cancelled", endedAt: now, outcome: { by: "rule", at: now, rule: { id: "r", name: "nan", text: "Loss went to NaN.", does: "stop" } } });
  assert.doesNotMatch(said([model("a", summary()), byUser]).text, /^f · /);
  assert.match(said([model("a", summary()), byRule]).text, /^g · Auto-stopped:/);
});

test("otherwise the summary counts the runs in each state and, in the same sentence, when the next should finish", () => {
  const eta = (mid, floor = false) => summary({ etaMs: { lo: mid - 60_000, mid, hi: mid + 60_000, at: now }, etaFloor: floor });
  const models = [model("a", eta(29 * 60_000)), model("b", eta(40 * 60_000)), model("c", summary({ tone: "starting" }), { status: "starting" }), model("d", summary({ tone: "done" }), { status: "done", endedAt: now })];
  const { text, firstDone } = said(models);
  assert.equal(text, [
    m.run_summary_starting({ count: "**1**" }), m.run_summary_running({ count: "**2**" }), m.run_summary_done({ count: "**1**" }), m.run_summary_next_done({ duration: "**29m**" }),
  ].map(bare).join(" · "), "once one is done, the soonest of the rest is the next");
  assert.equal(firstDone, null, "said once, in the sentence");
  // A time read from a bar in the log is only the least it will take.
  assert.match(said([model("a", eta(29 * 60_000, true)), model("b", eta(40 * 60_000))]).text, new RegExp(`${bare(m.run_summary_first_done_at_least({ duration: "**28m**" })).replace(/\*/g, "\\*")}$`));
  assert.equal(said([model("a", summary()), model("b", summary())]).text, bare(m.run_summary_running({ count: "**2**" })));
  // Beside a sentence about one run, the time stands apart.
  const failed = model("f", summary({ tone: "failed" }), { status: "failed", endedAt: now });
  assert.equal(said([model("a", eta(29 * 60_000)), failed]).firstDone, bare(m.run_summary_next_done({ duration: "29m" })));
  assert.equal(said([model("a", eta(29 * 60_000)), model("b", eta(40 * 60_000))]).text, [m.run_summary_running({ count: "**2**" }), m.run_summary_first_done({ duration: "**29m**" })].map(bare).join(" · "));
});

test("the watcher's summary stands for the runs its look covered, with names and numbers bound to each run", () => {
  const models = [model("a", scored(1.31, { bindings: { val_bpb: 1.31 } })), model("b", scored(1.207, { bindings: { val_bpb: 1.207 } }))];
  const watch = { lookId: "7", at: now, runIds: ["b", "a"], keys: { r1: "a", r2: "b" }, summary: { template: "{r2} leads at val bpb **{r2:val_bpb}**; {r1} is close behind." } };
  const { parts } = summarize(models, shortLabels(depths(4, 6)), watch);
  assert.equal(bare(parts.map((part) => part.text).join("")), "Depth 6 leads at val bpb **1.21**; Depth 4 is close behind.");
  // A look that missed a run now live, or a run that needs attention, gives the words back to orx.
  const third = [...models, model("c", summary())];
  assert.doesNotMatch(bare(summarize(third, shortLabels(depths(4, 6, 8)), watch).parts.map((part) => part.text).join("")), /close behind/);
  const failed = [models[0], model("b", scored(1.2, { tone: "failed" }), { status: "failed", endedAt: now })];
  assert.match(bare(summarize(failed, shortLabels(depths(4, 6)), watch).parts[0].who ?? ""), /Depth 6/);
});
