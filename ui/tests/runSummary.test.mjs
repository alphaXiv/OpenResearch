import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { m } from "../src/paraglide/messages.js";
import { runModel } from "../src/runMonitor/model.ts";
import { shortLabels, summarize } from "../src/runMonitor/summary.ts";

const sweep = ["Depth 4 at 2e15 FLOPs", "Depth 6 at 2e15 FLOPs", "Depth 8 at 2e15 FLOPs"];

for (const [titles, labels, context] of [
  [sweep, ["4", "6", "8"], "Depth … at 2e15 FLOPs"],
  [[...sweep, "Tokenizer check"], ["4", "6", "8", "Tokenizer check"], null],
  [["LR 3e-4, warmup 200 steps", "LR 1e-3, warmup 200 steps"], ["3e-4", "1e-3"], "LR … warmup 200 steps"],
  [["nanochat d20 lr 3e-4", "nanochat d20 lr 1e-3"], ["3e-4", "1e-3"], "nanochat d20 lr …"],
  [["Ablation: no dropout", "Ablation: no weight decay"], ["dropout", "weight decay"], "Ablation: no …"],
  [["Baseline (depth 4)", "Baseline (depth 12)"], ["4)", "12)"], "Baseline (depth …"],
  [["Dropout 10% at 2e15 FLOPs", "Dropout 20% at 2e15 FLOPs"], ["10%", "20%"], "Dropout … at 2e15 FLOPs"],
  [["Глубина 4 при 2e15 FLOPs", "Глубина 6 при 2e15 FLOPs"], ["4", "6"], "Глубина … при 2e15 FLOPs"],
  [["العمق 4 عند 2e15", "العمق 6 عند 2e15"], ["4", "6"], "العمق … عند 2e15"],
  [["深さ4で学習", "深さ6で学習"], ["深さ4で学習", "深さ6で学習"], null],
  [["深さ 4 で学習", "深さ 6 で学習"], ["4", "6"], "深さ … で学習"],
  [["Depth 4", "Depth 6"], ["4", "6"], "Depth …"],
  [["Baseline", "Baseline with dropout"], ["Baseline", "with dropout"], "Baseline …"],
  [["Depth 4 at 2e15 FLOPs", "Depth 4 at 2e15 FLOPs"], ["Depth 4 at 2e15 FLOPs", "Depth 4 at 2e15 FLOPs"], null],
  [[...sweep, sweep[0]], ["4", "6", "8", "4"], "Depth … at 2e15 FLOPs"],
  [["Depth 4 at 2e15 FLOPs", "Depth 6 at 2e15 FLOPs", "LR 3e-4 on SQuAD", "LR 1e-3 on SQuAD"], ["4", "6", "3e-4", "1e-3"], null],
  [["depth-4", "depth-6"], ["4", "6"], "depth …"],
  [["short-pipeline-sft-lr-x1", "short-pipeline-sft-lr-x2"], ["x1", "x2"], "short-pipeline-sft-lr …"],
  [["lr-3e-4", "lr-1e-3"], ["3e-4", "1e-3"], "lr …"],
  [["lr_3e-4", "lr_1e-3"], ["3e-4", "1e-3"], "lr …"],
  [["scale_-0.5", "scale_+0.5"], ["-0.5", "+0.5"], "scale …"],
  [["scale--0.5", "scale--1.5"], ["-0.5", "-1.5"], "scale …"],
  [["Scale -0.5", "Scale -1.5"], ["-0.5", "-1.5"], "Scale …"],
  [["scale-x0.5", "scale-x1.5"], ["x0.5", "x1.5"], "scale …"],
  [["compute-2e15", "compute-3e15"], ["2e15", "3e15"], "compute …"],
  [["run-small_lr-3e-4-eval", "run-large_lr-1e-3-eval"], ["small_lr-3e-4", "large_lr-1e-3"], "run … eval"],
  [["lr-3e-4-eval", "lr-1e-4-eval"], ["3e-4", "1e-4"], "lr … eval"],
  [["run--0.5-eval", "run--1.5-eval"], ["-0.5", "-1.5"], "run … eval"],
]) {
  test(`labels for ${JSON.stringify(titles)}`, () => {
    const named = shortLabels(titles);
    assert.deepEqual({ labels: named.labels, context: named.context?.short ?? null }, { labels, context });
  });
}

for (const [titles, whole, short] of [
  [["Voted LoRA, no augmentation", "Voted LoRA, 30% augmented", "Voted LoRA, 60% augmented"], "Voted LoRA, no augmentation, 30% augmented, 60% augmented", "Voted LoRA"],
  [sweep, "Depth 4, 6, 8 at 2e15 FLOPs", "Depth … at 2e15 FLOPs"],
  [["LR 3e-4", "LR 1e-3", "LR 3e-3"], "LR 3e-4, 1e-3, 3e-3", "LR …"],
  [["nanochat d12 baseline", "nanochat d20 baseline", "nanochat d26 baseline"], "nanochat d12, d20, d26 baseline", "nanochat … baseline"],
  [["LoRA on Wispr text", "LoRA on voted labels"], "LoRA on Wispr text, voted labels", "LoRA on …"],
  [[...sweep, sweep[0]], "Depth 4, 6, 8 at 2e15 FLOPs", "Depth … at 2e15 FLOPs"],
]) {
  test(`what ${JSON.stringify(titles)} share, whole and short`, () => {
    assert.deepEqual(shortLabels(titles).context, { whole, short });
  });
}

const fixture = (name) => JSON.parse(readFileSync(new URL(`./fixtures/runMonitor/${name}.json`, import.meta.url), "utf8"));
const running = fixture("running");
const now = running.asOf;
const bare = (text) => text.replace(/[⁦-⁩]/g, "");
const signals = { val_bpb: { label: "val bpb", digits: 2, better: "lower" } };
const summary = (extra = {}) => ({
  tone: "running", whole: 0.4, headline: null, sentence: null, bindings: {}, signals: {}, estimate: null, flag: null,
  rev: 1, generation: "g1", asOf: now, limitEndsAt: null, ...extra,
});
const scored = (value, extra = {}) => summary({ headline: { name: "val_bpb", label: "val bpb", value }, signals, ...extra });
const model = (id, monitor, extra = {}) => runModel({
  item: { sessionId: "s", experimentName: id, run: { id, experimentId: id, projectId: "p", status: "running", createdAt: now - 600_000, startedAt: now - 600_000, updatedAt: now, contact: null, cancelling: false, host: null, backend: { kind: "local_job" }, monitor, ...extra } },
  monitor: null, shown: true, agent: "Claude", now, nextRetryMs: null,
});
/** Titles that differ by one setting: a label group. */
const depths = (...depths) => depths.map((depth) => `Depth ${depth} at 2e15 FLOPs`);
const said = (models, titles = models.map((item) => item.run.id)) => {
  const { parts, firstDone } = summarize(models, shortLabels(titles), null, now);
  return { text: bare(parts.map((part) => [part.who, part.text].filter(Boolean).join(" ")).join(" · ")), firstDone: firstDone && bare(firstDone) };
};

test("runs that differ by one setting and share a headline that says which way is better name the leader, its number bound to its value", () => {
  const models = [model("a", scored(1.31)), model("b", scored(1.21)), model("c", scored(1.27))];
  assert.equal(said(models, depths(4, 6, 8)).text, "6 leads at val bpb **1.21**");
  const higher = { acc: { label: "accuracy", digits: 2, better: "higher" } };
  const accurate = (value) => summary({ headline: { name: "acc", label: "accuracy", value }, signals: higher });
  assert.equal(said([model("a", accurate(0.5)), model("b", accurate(0.75))], ["LR 3e-4 on SQuAD", "LR 1e-3 on SQuAD"]).text, "1e-3 leads at accuracy **0.75**");
});

test("runs lead only within one label group; unrelated runs are counted", () => {
  const count = (n) => bare(m.run_summary_running({ count: `**${n}**` }));
  // Unrelated titles share no setting to rank.
  assert.equal(said([model("a", scored(1.3)), model("b", scored(1.2))]).text, count(2));
  // An unrelated run with the same headline neither leads nor stops the sweep from leading.
  assert.equal(said([model("a", scored(1.3)), model("b", scored(1.2)), model("c", scored(0.9))], [...depths(4, 6), "Tokenizer check"]).text, "6 leads at val bpb **1.20**");
  // Two sweeps that could each lead: the line can't say both, so it counts.
  const two = [model("a", scored(1.3)), model("b", scored(1.2)), model("c", scored(1.1)), model("d", scored(1.0))];
  assert.equal(said(two, [...depths(4, 6), "LR 3e-4 on SQuAD", "LR 1e-3 on SQuAD"]).text, count(4));
  // Two runs of one experiment are one title, not a group.
  assert.equal(said([model("a", scored(1.3)), model("b", scored(1.2))], depths(4, 4)).text, count(2));
});

test("a tie for best reports the sweep's progress in either order", () => {
  const models = [model("a", scored(1.2)), model("b", scored(1.2)), model("c", scored(1.3))];
  const count = bare(m.run_summary_running({ count: "**3**" }));
  assert.equal(said(models, depths(4, 6, 8)).text, count);
  assert.equal(said([...models].reverse(), depths(8, 6, 4)).text, count);
  const unique = [model("a", scored(1.2)), model("b", scored(1.3)), model("c", scored(1.3))];
  assert.equal(said(unique, depths(4, 6, 8)).text, "4 leads at val bpb **1.20**");
});

test("once every run has finished the leader is said with that", () => {
  const done = { status: "done", endedAt: now };
  const models = [model("a", scored(1.3, { tone: "done" }), done), model("b", scored(1.2, { tone: "done" }), done)];
  assert.equal(said(models, depths(4, 6)).text, "6 leads at val bpb **1.20**; all finished");
});

test("no leader without a declared direction, a shared headline, or two numbers", () => {
  const undeclared = { val_bpb: { label: "val bpb", digits: 2 } };
  const progress = bare(m.run_summary_running({ count: "**2**" }));
  assert.equal(said([model("a", scored(1.3, { signals: undeclared })), model("b", scored(1.2, { signals: undeclared }))], depths(4, 6)).text, progress);
  const other = summary({ headline: { name: "loss", label: "loss", value: 2 }, signals: { loss: { label: "loss", better: "lower" } } });
  assert.equal(said([model("a", scored(1.3)), model("b", other)], depths(4, 6)).text, progress);
  assert.equal(said([model("a", scored(1.3)), model("b", scored("NaN"))], depths(4, 6)).text, progress);
  // A run with no headline yet neither leads nor stops the others from leading.
  assert.equal(said([model("a", scored(1.3)), model("b", scored(1.2)), model("c", summary())], depths(4, 6, 8)).text, "6 leads at val bpb **1.20**");
});

test("who needs attention comes first, before any leader", () => {
  const failed = model("c", scored(1.0, { tone: "failed", logError: "ImportError: cannot import name 'Parakeet'" }), { status: "failed", endedAt: now, exitCode: 1 });
  const models = [model("a", scored(1.3)), model("b", scored(1.2)), failed];
  assert.equal(said(models).text, `${bare(m.run_summary_attention({ name: "c", state: m.status_failed() }))} ImportError: cannot import name 'Parakeet'`);
  // A run that ended with nothing more to say says how long it took.
  const stopped = model("e", scored(1.0, { tone: "stopped" }), { status: "cancelled", endedAt: now, exitCode: 143, outcome: { by: "rule", at: now } });
  assert.equal(said([models[0], stopped]).text, bare(m.run_summary_attention_ended({ name: "e", ended: m.run_eta_stopped_at({ duration: "10m" }) })));
  const choice = model("d", summary({ tone: "flag", flag: { id: "f", state: "waiting", text: "Loss went to **NaN**." } }));
  assert.match(said([...models, choice]).text, /^d · Flagged: Loss went to \*\*NaN\*\*\.$/);
  const lost = model("e", summary(), { contact: { type: "unreachable", since: now - 60_000, cause: "timed out" }, host: "lab-a40", backend: { kind: "ssh_job" } });
  const withLost = [model("a", scored(1.3)), lost];
  assert.match(said(withLost).text, /Can't reach lab-a40/);
  const opened = summarize(withLost, shortLabels(["a", "e"]), null, now, true);
  assert.equal(opened.parts[0].text, "", "the run row already has the contact message");
  const byUser = model("f", summary({ tone: "stopped" }), { status: "cancelled", endedAt: now, outcome: { by: "user", at: now } });
  const byRule = model("g", summary({ tone: "stopped" }), { status: "cancelled", endedAt: now, outcome: { by: "rule", at: now, rule: { id: "r", name: "nan", text: "Loss went to NaN.", does: "stop" } } });
  assert.doesNotMatch(said([model("a", summary()), byUser]).text, /^f · /);
  assert.match(said([model("a", summary()), byRule]).text, /^g · Auto-stopped:/);
});

test("otherwise the summary counts each state and provides a separate estimate for the next finish", () => {
  const eta = (mid, floor = false) => summary({ estimate: { loMs: mid - 60_000, midMs: mid, hiMs: mid + 60_000, at: now, pending: false, floor } });
  const models = [model("a", eta(29 * 60_000)), model("b", eta(40 * 60_000)), model("c", summary({ tone: "starting" }), { status: "starting" }), model("d", summary({ tone: "done" }), { status: "done", endedAt: now })];
  const { text, firstDone } = said(models);
  assert.equal(text, [
    m.run_summary_starting({ count: "**1**" }), m.run_summary_running({ count: "**2**" }), m.run_summary_done({ count: "**1**" }),
  ].map(bare).join(" · "));
  assert.equal(firstDone, bare(m.run_summary_next_done({ duration: "**29m**" })), "once one is done, the soonest of the rest is the next");
  assert.equal(said([model("a", eta(29 * 60_000, true)), model("b", eta(40 * 60_000))]).firstDone, bare(m.run_summary_first_floor({ duration: "**29m**" })));
});

test("the watcher's summary stands for the runs its look covered, with names and numbers bound to each run", () => {
  const models = [model("a", scored(1.31, { bindings: { val_bpb: 1.31 } })), model("b", scored(1.207, { bindings: { val_bpb: 1.207 } }))];
  const watch = { lookId: "7", at: now, runIds: ["b", "a"], keys: { r1: "a", r2: "b" }, summary: { template: "{r2} leads at val bpb **{r2:val_bpb}**; {r1} is close behind." } };
  const { parts } = summarize(models, shortLabels(depths(4, 6)), watch, now);
  assert.equal(bare(parts.map((part) => part.text).join("")), "6 leads at val bpb **1.21**; 4 is close behind.");
  // A look that missed a run now live, or a run that needs attention, gives the words back to orx.
  const third = [...models, model("c", summary())];
  assert.doesNotMatch(bare(summarize(third, shortLabels(depths(4, 6, 8)), watch, now).parts.map((part) => part.text).join("")), /close behind/);
  const failed = [models[0], model("b", scored(1.2, { tone: "failed" }), { status: "failed", endedAt: now })];
  assert.match(bare(summarize(failed, shortLabels(depths(4, 6)), watch, now).parts[0].who ?? ""), /6/);
});

test("a summary's subsecond time left stays positive", () => {
  const line = model("one", summary({ estimate: { loMs: 500, midMs: 500, hiMs: 500, at: now, pending: false, floor: false } }));
  const { firstDone } = said([line]);
  const expected = m.run_summary_first_done({ duration: `**${m.duration_seconds({ value: "1" })}**` });
  assert.equal(firstDone, bare(expected));
});

test("a stage counting work during setup takes precedence over the watcher's starting summary", () => {
  const models = [
    model("baseline", scored(1.3)),
    model("new labels", summary({
      setup: { line: null, startsAt: null },
      stage: { name: "label", done: 1664, of: 5086, unit: "clips" },
    })),
  ];
  const watch = {
    at: now, runIds: ["baseline", "new labels"], keys: { r1: "baseline", r2: "new labels" },
    summary: { template: "{r1} is training. {r2} is still starting." },
  };
  const { parts } = summarize(models, shortLabels(models.map((item) => item.name)), watch, now);
  assert.equal(bare(parts[0].who), "new labels");
  assert.match(bare(parts[0].text), /label \*\*1,664\*\* of \*\*5,086\*\* clips/);
});
