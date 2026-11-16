import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { plainSentence } from "../src/runMonitor/format.ts";
import { runModel } from "../src/runMonitor/model.ts";
import { foldedStages } from "../src/runMonitor/stages.ts";

const base = JSON.parse(readFileSync(new URL("./fixtures/runMonitor/running.json", import.meta.url), "utf8"));
const flagged = JSON.parse(readFileSync(new URL("./fixtures/runMonitor/flagged.json", import.meta.url), "utf8"));
const plain = (text) => plainSentence(text).replace(/[⁦-⁩]/g, "");
const now = Date.now();
const start = now - 180_000;
const phase = (name, from, to, priorMs, of = null, unit = null) => ({ name, startedAt: from === null ? null : start + from, endedAt: to === null ? null : start + to, priorMs, of, unit, planned: true });
const monitor = () => ({ ...base, asOf: now, estimate: { ...base.estimate, at: now }, sentence: { template: "Run is going smoothly.", t: now }, setup: null,
  phases: [phase("Install", 0, 10_000, 10_000), phase("Download", 10_000, 30_000, 20_000, 9, "shards"), phase("Tokenizer", 30_000, 60_000, 30_000), phase("Train", 60_000, null, 7_200_000, 5000, "steps"), phase("Eval", null, null, 240_000), phase("SFT", null, null, 720_000), phase("Probe", null, null, 24_000)],
  stageOrder: { index: 3, count: 7, first: 3 }, progress: { ...base.progress, phase: 3, done: 120, lastAt: now },
});
const model = (data, at = now) => runModel({ item: { sessionId: "s", experimentName: "A named run", run: { id: "r", experimentId: "e", projectId: "p", status: "running", createdAt: start, startedAt: start, updatedAt: now, contact: null, cancelling: false, host: null, backend: { kind: "local_job" } } }, monitor: data, shown: true, agent: "Claude", now: at, nextRetryMs: null });

test("C shows the folded stage position, its count, then the fresh watcher sentence", () => {
  const data = monitor();
  const row = model(data);
  assert.equal(plain(row.sentence.text), "Train (2/5) · Step 120 of 5,000. Run is going smoothly.");
  assert.equal(row.sentence.watcher, "Run is going smoothly.");
  const stale = model(data, now + 180_001);
  assert.equal(plain(stale.sentence.text), "Train (2/5) · Step 120 of 5,000");
  assert.equal(stale.sentence.watcher, undefined);
});

test("Setup folds its prep phases and retains the current download count", () => {
  const data = monitor();
  data.stageOrder.index = 1;
  data.phases = data.phases.map((phase, index) => index === 1 ? { ...phase, endedAt: null } : index > 1 ? { ...phase, startedAt: null, endedAt: null } : phase);
  data.progress = { ...data.progress, phase: 1, done: 4 };
  data.setup = { stage: "Download", line: null, startsAt: now + 120_000 };
  data.sentence = null;
  const row = model(data);
  assert.equal(plain(row.sentence.text), "Setup (1/5) · Download 4 of 9 shards");
  const stages = foldedStages(data, row);
  assert.deepEqual(stages.phases.map((phase) => phase.name), ["Setup", "Train", "Eval", "SFT", "Probe"]);
  assert.deepEqual(stages.names, ["Install", "Download", "Tokenizer"]);
  assert.equal(stages.current, 0);
  assert.equal(stages.phases[0].priorMs, 60_000);
});

test("an unknown first-data stage or an undeclared plan claims no stage total", () => {
  for (const order of [{ index: 3, count: 7, first: null }, { index: 3, count: null, first: 3 }]) {
    const data = { ...monitor(), stageOrder: order, sentence: null };
    if (order.first === null) data.setup = { stage: "Train", line: null, startsAt: null };
    assert.doesNotMatch(plain(model(data).sentence.text), /\(\d+\/\d+\)/);
  }
});

test("a flag leads with its warning while normal watcher text follows the stage", () => {
  const data = { ...monitor(), latest: { loss: "NaN" }, flag: { ...flagged.flag, at: now } };
  const row = model(data);
  assert.ok(plain(row.sentence.text).startsWith("Loss"));
  assert.ok(!row.sentence.text.includes("Train (2/5)"));
  assert.equal(row.sentence.text, row.sentence.watcher);
});

test("an empty or skipped prep group invents no duration or infinite timestamp", () => {
  const data = monitor();
  const first = { ...data, stageOrder: { index: 3, count: 7, first: 0 } };
  const empty = foldedStages(first, model(first));
  assert.equal(empty.phases[0].priorMs, null);
  const skipped = { ...data, phases: data.phases.map((phase, index) => index < 3 ? { ...phase, skipped: true, startedAt: null, endedAt: null } : phase) };
  const stages = foldedStages(skipped, model(skipped));
  assert.equal(stages.phases[0].endedAt, null);
  assert.equal(stages.phases[0].skipped, true);
});

test("an unnamed reported counter has a readable label in the stage card", () => {
  const data = monitor();
  data.phases[3].name = "";
  const stages = foldedStages(data, model(data));
  assert.equal(stages.phases[1].name, "Run");
  assert.equal(stages.current, 1);
});
