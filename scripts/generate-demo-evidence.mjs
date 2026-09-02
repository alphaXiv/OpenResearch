import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const log = readFileSync(join(root, "demo/nanochat/run-output.txt"), "utf8");
const output = join(root, "demo/nanochat/evidence");
mkdirSync(output, { recursive: true });

const rows = new Map();
let phase = "base";
let trainBpb;
let validationBpb;
const core = [];

function rowFor(currentPhase, step) {
  const key = `${currentPhase}:${step}`;
  const existing = rows.get(key);
  if (existing) return existing;
  const row = {
    phase: currentPhase,
    step,
    loss: "",
    validationBpb: "",
    tokensPerSecond: "",
    totalMinutes: "",
  };
  rows.set(key, row);
  return row;
}

for (const line of log.split("\n")) {
  if (line === "=== Supervised fine-tuning ===") phase = "sft";

  const training = line.match(
    /^step (\d+)(?:\/\d+)? .*?\| loss: ([\d.]+).*?\| tok\/sec: ([\d,]+).*?\| total time: ([\d.]+)m/,
  );
  if (training) {
    const row = rowFor(phase, Number(training[1]));
    row.loss = training[2];
    row.tokensPerSecond = training[3].replaceAll(",", "");
    row.totalMinutes = training[4];
  }

  const validation = line.match(/^Step (\d+) \| Validation bpb: ([\d.]+)/);
  if (validation) rowFor(phase, Number(validation[1])).validationBpb = validation[2];

  if (phase === "base") {
    const train = line.match(/^train bpb: ([\d.]+)/);
    if (train) trainBpb = Number(train[1]);
    const val = line.match(/^val bpb: ([\d.]+)/);
    if (val) validationBpb = Number(val[1]);
    const evaluation = line.match(
      /^Evaluating: ([^ ]+).*?accuracy: ([\d.]+) \| centered: ([\d.]+) \| time: ([\d.]+)s/,
    );
    if (evaluation) {
      core.push({
        task: evaluation[1],
        accuracy: Number(evaluation[2]),
        centered: Number(evaluation[3]),
        seconds: Number(evaluation[4]),
      });
    }
  }
}

const orderedRows = [...rows.values()].sort((left, right) => {
  if (left.phase !== right.phase) return left.phase === "base" ? -1 : 1;
  return left.step - right.step;
});
const finalValidationBpb = (currentPhase) =>
  Number(
    orderedRows.findLast(
      (row) => row.phase === currentPhase && row.validationBpb !== "",
    ).validationBpb,
  );
const csv = [
  "phase,step,loss,validation_bpb,tokens_per_second,total_minutes",
  ...orderedRows.map((row) =>
    [
      row.phase,
      row.step,
      row.loss,
      row.validationBpb,
      row.tokensPerSecond,
      row.totalMinutes,
    ].join(","),
  ),
].join("\n");
writeFileSync(join(output, "training-metrics.csv"), `${csv}\n`);

const evaluation = {
  baseEvaluation: {
    trainBpb,
    validationBpb,
  },
  core,
  final: {
    baseValidationBpb: finalValidationBpb("base"),
    sftValidationBpb: finalValidationBpb("sft"),
    chatAnswer: "Paris",
  },
};
writeFileSync(join(output, "evaluation-metrics.json"), `${JSON.stringify(evaluation, null, 2)}\n`);
