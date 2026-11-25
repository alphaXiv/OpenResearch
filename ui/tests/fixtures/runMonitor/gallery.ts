import type { Run, RunBannerLevel, RunStatus } from "../../../src/api";
import { runActive } from "../../../src/runDisplay";
import { bindingsOf, runTone, statedFlag } from "../../../src/runMonitor/model";
import type { ChatShownRun, Flag, RunMonitor, RunMonitorSummary } from "../../../src/runMonitor/types";
import flaggedJson from "./flagged.json" with { type: "json" };
import pipelineJson from "./pipeline.json" with { type: "json" };
import runningJson from "./running.json" with { type: "json" };
import sweepJson from "./sweep.json" with { type: "json" };

export const now = Date.now();
export const sessionId = "fixture-chat";

function fixture(raw: typeof runningJson | typeof flaggedJson | typeof pipelineJson | typeof sweepJson): RunMonitor {
  // Older JSON snapshots predate these required wire fields and JSON imports widen enum values to string.
  const data = raw as unknown as RunMonitor;
  const delta = now - data.asOf;
  const shift = (at: number | null) => at === null ? null : at + delta;
  return {
    ...data,
    brief: data.brief ?? [],
    rules: data.rules ?? [],
    logError: data.logError ?? null,
    asOf: now,
    phases: data.phases.map((phase) => ({ ...phase, startedAt: shift(phase.startedAt), endedAt: shift(phase.endedAt) })),
    progress: data.progress && { ...data.progress, lastAt: shift(data.progress.lastAt) },
    estimate: data.estimate && { ...data.estimate, at: data.estimate.at + delta, floor: false },
    sentence: data.sentence && { ...data.sentence, t: now },
    flags: data.flags.map((flag) => ({ ...flag, at: flag.at + delta })),
  };
}

const running = fixture(runningJson);
const flagged = fixture(flaggedJson);
const pipeline = fixture(pipelineJson);
const sweep = fixture(sweepJson);
const plain: RunMonitor = {
  ...running,
  sentence: null,
  series: { loss: [[0, 3.19], [160, 2.9], [340, 2.96], [650, 2.56], [870, 2.43], [1050, 2.45], [1240, 2.31]] },
};
const silent = { ...plain, estimate: null, progress: null, phases: [], stageOrder: { index: null, count: null, first: null },
  headline: null, view: { ...plain.view, headline: [], blocks: [] }, latest: {}, series: {} };
const barePhase = { ...plain.phases[0], name: "", unit: null };
const bare = { ...plain, phases: [barePhase], stageOrder: { index: 0, count: null, first: 0 } };
const estimate = { ...running.estimate!, at: now };
const automaticFormat: RunMonitor["view"] = {
  ...plain.view,
  signals: { loss: { label: "Loss", better: "lower" } },
};
export const readError = "The monitor could not read the latest report.";
const comparison = { d: -0.19, mag: 0.19, parent: 2.5, good: true, flat: false, step: 1240 };
const askRule = { name: "check-variance", text: "Ask when validation loss rises three times in a row.", does: "ask" } as const;

type FixtureRun = { item: ChatShownRun; monitor: RunMonitor };
type RunPatch = Partial<Omit<Run, "id" | "projectId" | "experimentId" | "monitor">>;

function sample(name: string, monitor: RunMonitor = running, patch: RunPatch = {}, id = "r1"): FixtureRun {
  const projectId = "fixture-project";
  const experimentId = `experiment-${id}`;
  const run: Run = {
    id, projectId, experimentId,
    chatSessionId: sessionId,
    status: "running",
    createdAt: now - 35 * 60_000,
    startedAt: now - 32 * 60_000,
    updatedAt: now,
    backend: { kind: "ssh_job", host: "lab-gpu", jobDir: "/runs/example" },
    host: "lab-gpu",
    contact: null,
    cancelling: false,
    lostReason: null,
    ...patch,
  };
  const data = { ...monitor, runId: id, viewFrom: { ...monitor.viewFrom, experimentId } };
  const live = runActive(run.status);
  const nameOfHeadline = data.headline ?? data.view.headline[0];
  const phase = data.progress ? data.phases[data.progress.phase] : null;
  const flag = statedFlag(run, data);
  const record = data.flags.find((entry) => entry.id === flag?.id);
  const tone = runTone(run, data);
  const { state, reason } = data.availability;
  const summary: RunMonitorSummary = {
    tone: tone === "lost" ? "running" : tone,
    whole: data.progress?.whole ?? null,
    stage: phase && { name: phase.name, of: phase.of, unit: phase.unit, done: data.progress?.done ?? null },
    stageOrder: data.stageOrder,
    headline: nameOfHeadline ? { name: nameOfHeadline, label: data.view.signals[nameOfHeadline]?.label ?? nameOfHeadline, value: data.latest[nameOfHeadline] ?? null } : null,
    sentence: data.sentence,
    bindings: bindingsOf(data, live),
    signals: data.view.signals,
    estimate: data.estimate,
    segments: data.progress?.segments,
    limitEndsAt: data.limit?.endsAt ?? null,
    flag: flag && { ...flag, text: record?.text, template: record?.template, bindings: record?.bindings },
    setup: data.setup,
    logError: data.logError,
    rev: data.rev,
    generation: data.generation,
    asOf: data.asOf,
    ...(state === "unavailable"
      ? { unavailable: reason ?? readError }
      : state !== "live" ? { availability: { state, reason } } : {}),
  };

  return {
    item: { sessionId, run: { ...run, monitor: summary }, experimentName: name, experimentSlug: experimentId, runNo: 1, runCount: 1 },
    monitor: data,
  };
}

type SpecimenOptions = {
  pin?: "stage" | "eta" | "stop";
  level?: RunBannerLevel;
  read?: "loading" | "error" | "refresh-error";
  failedList?: boolean;
  oldServer?: boolean;
  lineage?: "diff" | "error";
  retryHost?: boolean;
  choice?: { sending: "keep" | "stop" | "ask" } | { error: string };
};

export type Specimen = SpecimenOptions & { label: string; runs: FixtureRun[] };

function one(label: string, monitor: RunMonitor = plain, patch: RunPatch = {}, options: SpecimenOptions = {}, experimentName = "CIFAR-10 · ResNet baseline"): Specimen {
  return { label, runs: [sample(experimentName, monitor, options.lineage ? { ...patch, commitSha: "1234abcd" } : patch)], ...options };
}

const end = (status: RunStatus): RunPatch => ({ status, endedAt: now - 90_000 });
const stopped = end("cancelled");
const setup: RunMonitor = {
  ...plain, latest: {}, series: {},
  setup: { line: "Caching tokenized training data", stage: null, startsAt: null },
  progress: { ...plain.progress!, done: null, whole: null },
  stageOrder: { index: 0, count: 2, first: 1 },
  phases: [{ ...plain.phases[0], name: "Setup", of: null }, plain.phases[0]],
  estimate: null,
};
const unplanned: RunMonitor = {
  ...pipeline,
  phases: pipeline.phases.map((phase, index) => index === 2
    ? { ...phase, name: "warm restart", planned: false }
    : index === 3 ? { ...phase, skipped: true } : phase),
  stageOrder: { ...pipeline.stageOrder, count: null },
};
const phases = [
  "download weights", "validate cache", "tokenize", "build batches", "allocate devices", "warm up",
  "train", "evaluate", "calibrate", "benchmark", "export", "upload artifacts",
];
const folded: RunMonitor = {
  ...pipeline,
  phases: phases.map((name, index) => ({
    ...plain.phases[0],
    name,
    planned: true,
    startedAt: index <= 7 ? now - (8 - index) * 60_000 : null,
    endedAt: index < 7 ? now - (7 - index) * 60_000 : null,
    priorMs: 60_000,
    skipped: index === 5,
  })),
  stageOrder: { index: 7, count: phases.length, first: 6 },
  progress: { ...pipeline.progress!, phase: 7, done: 210 },
};

function warning(source: Flag["source"], state: Flag["state"], changes: Partial<Flag> = {}): RunMonitor {
  const base = flagged.flags[0];
  const words = source === "rule" ? { template: "Validation loss rose three times.", detail: askRule.text }
    : source === "tripwire" ? { template: "Loss is **{loss}**.", detail: "The loss is non-finite." }
    : {};
  const flag: Flag = { ...base, ...words, source, state, lookId: source === "look" ? base.lookId : undefined, ...changes };
  return { ...flagged, flagId: state === "kept" || state === "stopped" ? null : flag.id,
    flags: [flag], rules: source === "rule" ? [askRule] : [] };
}

const sshUnreachable = { type: "unreachable", since: now - 4 * 60_000, cause: "The SSH connection timed out." } as const;
const sshAuth = { type: "needs_auth", since: now - 4 * 60_000, cause: "SSH needs your passphrase." } as const;
const unknown = { type: "unreconciled", since: now - 4 * 60_000, detail: "The host lost its job record before reporting an exit code." } as const;

function batch(count: number, name: (index: number) => string, monitor = plain, varied = true, status: (index: number) => RunStatus = () => "running") {
  return Array.from({ length: count }, (_, index) => {
    const phase = status(index);
    const data = varied ? { ...monitor, latest: { ...monitor.latest, loss: 2.3 - index / 25 } } : monitor;
    return sample(name(index), data, { ...(runActive(phase) ? { status: phase } : end(phase)), createdAt: now - 30 * 60_000 + index * 400 }, `r${index + 1}`);
  });
}

const sweepNames = (index: number) => `CIFAR-10 · learning rate ${(0.0001 * (index + 1)).toPrecision(1)} · seed 42`;
const watchedBatch = batch(2, sweepNames);
watchedBatch[0].item.watch = {
  lookId: "sweep-look", at: now, runIds: ["r1", "r2"], keys: { r1: "r1", r2: "r2" },
  summary: { template: "{r2} has the lower loss (**{r2:loss}**). {r1} is still improving." },
};

export const specimenGroups: { name: string; specimens: Specimen[] }[] = [
  { name: "Lifecycle", specimens: [
    one("Submitting", silent, { status: "submitting", startedAt: null }),
    one("Starting · queued on Slurm", silent, { status: "starting", startedAt: null, waitingReason: "Waiting for a GPU allocation.", backend: { kind: "slurm_job", partition: "gpu" } }),
    one("Setup · before the first count", setup),
    one("Setup · first report expected", { ...setup, setup: { ...setup.setup!, startsAt: now + 40_000 } }),
    one("Setup · named preparation stage", { ...setup, phases: [{ ...setup.phases[0], name: "tokenize" }, plain.phases[0]], setup: { ...setup.setup!, stage: "tokenize", startsAt: now - 1000 } }),
    one("Running · named stage with count", pipeline, {}, { level: "line" }),
    one("Running · a bare step and total", bare),
    one("Running · a bare step, no total", { ...bare, phases: [{ ...barePhase, of: null }], estimate: null, progress: { ...bare.progress!, whole: null, segments: [{ weight: 1, fill: null }] } }),
    one("Running · no counter or estimate", silent),
    one("Running · count in batches", { ...plain, phases: [{ ...plain.phases[0], unit: "batches" }] }),
    one("Running · count in rounds", { ...plain, phases: [{ ...plain.phases[0], name: "rollout", unit: "rounds" }] }),
    one("Running · finishing the last stage", { ...plain, estimate: { ...estimate, loMs: 0, midMs: 0, hiMs: 0 }, progress: { ...plain.progress!, done: 5000, whole: 1, segments: [{ weight: 1, fill: 1 }] } }),
    one("Estimating time left", { ...plain, estimate: { ...estimate, pending: true, midMs: null } }),
    one("Known time left · range", plain, {}, { level: "line", pin: "eta" }),
    one("Stop control · tooltip", plain, {}, { level: "line", pin: "stop" }),
    one("Time left · stage only", { ...pipeline, estimate: { ...estimate, floor: true, untimed: ["eval", "export"] } }),
    one("Time left · lower bound for the run", { ...pipeline, estimate: { ...estimate, floor: true, untimed: ["export"] } }),
    one("Time left · no timed stage yet", { ...pipeline, estimate: { ...estimate, floor: true, midMs: 0, untimed: ["export"] } }),
    one("Unknown time left · time limit", { ...plain, estimate: null, limit: { endsAt: now + 12 * 60_000 } }),
    one("Time limit from a run rule", { ...plain, estimate: null, limit: { endsAt: now + 12 * 60_000 }, rules: [{ ...askRule, does: "stop" }] }),
    one("Estimate exceeds the time limit", { ...plain, estimate: { ...estimate, pastLimitMs: 15 * 60_000 }, limit: { endsAt: now + 75 * 60_000 } }),
    one("Stalled before the estimated finish", { ...plain, progress: { ...plain.progress!, stallMs: 140_000 } }),
    one("Past the estimate midpoint", { ...plain, estimate: { ...estimate, at: now - estimate.midMs! - 30_000 } }),
    one("Past the latest estimated finish", { ...plain, estimate: { ...estimate, at: now - estimate.hiMs! - 90_000 } }),
    one("Done", plain, end("done")),
    one("Failed · exit code", plain, { ...end("failed"), exitCode: 137 }),
    one("Failed · error from the log", { ...plain, logError: "RuntimeError: CUDA out of memory while allocating a training batch." }, end("failed")),
    one("Failed · no recorded reason", silent, end("failed")),
    one("Cancelled by you", plain, { ...stopped, outcome: { by: "user", at: now - 90_000 } }),
    one("Cancelled by the agent", plain, { ...stopped, outcome: { by: "agent", at: now - 90_000, reason: "Two consecutive validation passes regressed." } }),
    one("Cancelled by the agent · reason in chat", plain, { ...stopped, outcome: { by: "agent", at: now - 90_000 } }),
    one("Cancelled by a rule", plain, { ...stopped, outcome: { by: "rule", at: now - 90_000, rule: { ...askRule, does: "stop" } } }),
    one("Revising after a stop", { ...plain, revising: true }, stopped),
    one("Lost · no contact detail recorded", silent, end("lost")),
    one("Lost · outcome unavailable", plain, { ...end("lost"), contact: unknown }),
  ] },
  { name: "Stage cards", specimens: [
    one("Folded Setup · completed and upcoming stages", folded, {}, { level: "line", pin: "stage", lineage: "diff" }),
    one("Unplanned stage · a skipped planned stage", unplanned, {}, { level: "line", pin: "stage" }),
    one("Local hardware and a failed diff read", plain, { backend: { kind: "local_job" }, host: null }, { level: "line", pin: "stage", lineage: "error" }),
    one("Launch failed before any stage", silent, { ...end("failed"), startedAt: null, backend: { kind: "openresearch_job", flavor: "l40s" }, resultMarkdown: "The provider had no capacity. Launch again." }, { pin: "stage" }),
    one("Missing SSH machine name", plain, { host: null }, { level: "line", pin: "stage" }),
    one("Stage details for a run without named stages", bare, {}, { level: "line", pin: "stage" }),
    one("Stage details loading", plain, {}, { level: "line", pin: "stage", read: "loading" }),
    one("Stage details failed to load", plain, {}, { level: "line", pin: "stage", read: "error" }),
    one("Stages temporarily unavailable", { ...plain, availability: { state: "unavailable", reason: readError } }, {}, { level: "line", pin: "stage" }),
  ] },
  { name: "Watcher and flags", specimens: [
    one("Watched · healthy sentence", running, {}, { level: "line" }),
    one("Watched · comparison in the headline", { ...running, versus: { step: { loss: comparison }, final: {} } }, {}, { level: "line" }),
    one("Watched · comparison already in the sentence", { ...running, versus: { step: { loss: comparison }, final: {} }, sentence: { template: "Loss **{loss}**, {loss.vs}.", t: now } }, {}, { level: "line" }),
    one("Watching off · automatic sentence", { ...plain, watch: { ...plain.watch, state: "off", model: null, looks: [] } }),
    one("No watching model · automatic sentence", { ...plain, watch: { ...plain.watch, state: "no_model", model: null, looks: [], why: "No model is signed in." } }),
    one("Watcher sentence · a count from the log", {
      ...plain, counts: { "loss=": 18 }, sentence: { t: now, template: '**{count:"loss="}** lines like "loss=" so far.' },
    }, {}, { level: "line" }),
    one("Old watcher sentence · automatic fallback", { ...running, sentence: { ...running.sentence!, t: now - 4 * 60_000 } }),
    ...(["look", "tripwire", "rule"] as const).flatMap((source) => [
      one(`Flag · ${source === "rule" ? "Ask rule" : source} · agent reading`, warning(source, "agent_looking")),
      one(`Flag · ${source === "rule" ? "Ask rule" : source} · waiting for you`, warning(source, "waiting"), {}, { level: "line" }),
    ]),
    one("Flag · queued · collapsed", warning("look", "raised"), {}, { level: "line" }),
    one("Flag · waking the agent", warning("look", "waking")),
    one("Flag · reading without log evidence", warning("rule", "agent_looking", { evidence: null })),
    one("Flag · log evidence across several lines", warning("look", "waiting", {
      evidence: { ...flagged.flags[0].evidence!, to: 93, lines: ["epoch=3 loss=2.32", "epoch=4 loss=NaN", "skip: non-finite gradient"] },
    })),
    one("Flag · log evidence without line numbers", warning("look", "waiting", {
      evidence: { ...flagged.flags[0].evidence!, from: null, to: null },
    })),
    one("Flag · Ask sent to the agent", warning("look", "asking", { choice: { c: "ask", by: "user", at: now } })),
    one("Flag · returned for your choice and noted again", warning("look", "waiting", { handback: 12,
      choice: { c: "ask", by: "user", at: now - 60_000 }, noted: [{ at: now, lookId: "look-2", text: "The next loss report was also non-finite." }] })),
    one("Flag · run ended, Keep result or Ask", warning("look", "waiting"), end("done")),
    one("Flag · finished run waking the agent", warning("look", "waking"), end("done")),
    ...(["keep", "stop", "ask"] as const).map((sending) => one(`Flag · sending ${sending}`, warning("look", "waiting"), {}, { choice: { sending } })),
    one("Flag · choice refused", warning("look", "waiting"), {}, { choice: { error: "The run owner changed. Try again." } }),
    one("Flag kept · returns to the run strip", { ...warning("look", "kept", { choice: { c: "keep", by: "user", at: now } }), sentence: null }),
    one("Flag stopped · returns to the ended line", { ...warning("look", "stopped", { choice: { c: "stop", by: "user", at: now } }), sentence: null }, { ...stopped, outcome: { by: "user", at: now } }),
  ] },
  { name: "Freshness and contact", specimens: [
    one("Stale · no update since", { ...plain, asOf: now - 6 * 60_000, availability: { state: "stale", reason: null } }, {}, { level: "line", pin: "eta" }),
    one("Rebuilding", { ...plain, availability: { state: "rebuilding", reason: "The view changed while the run was reporting." } }),
    one("Projection unavailable", { ...plain, availability: { state: "unavailable", reason: readError } }),
    one("Strip opened while the monitor loads", plain, {}, { read: "loading" }),
    one("Monitor load failed · Retry", plain, {}, { read: "error" }),
    one("Monitor refresh failed · last chart retained", plain, {}, { read: "refresh-error" }),
    one("Final log pending · before the notice", { ...plain, availability: { state: "stale", reason: null } }, end("done")),
    one("Final log still being read", { ...plain, availability: { state: "stale", reason: "The final log is still being read." } }, end("done")),
    one("Reported rows disagree with the plan", { ...pipeline, disagrees: { stage: "eval", of: 200, unit: "examples", table: "eval", rows: 137 } }),
    one("Reporting slower than the parent", { ...plain, pace: { times: 2.5, than: "parent" } }),
    one("Reporting slower than earlier", { ...plain, pace: { times: 3, than: "earlier" } }),
    one("Reporting error from the backend", plain, { backend: { kind: "ssh_job", monitoringError: "The run's log directory cannot be read." } }),
    one("Unreachable · SSH retry scheduled", plain, { contact: sshUnreachable }, { retryHost: true }),
    one("Needs authentication · SSH Connect", plain, { contact: sshAuth }),
    one("Needs authentication · cluster credentials", plain, { contact: sshAuth, backend: { kind: "k8s_job" }, host: null }),
    one("Needs authentication · rented box may still cost money", plain, { contact: sshAuth, backend: { kind: "openresearch_job", flavor: "l40s" }, host: null }),
    one("Unconfirmed launch", silent, { status: "submitting", startedAt: null, contact: unknown }),
    one("Cancelling", plain, { cancelling: true }),
    one("Cancellation pending · launch still submitting", silent, { status: "submitting", startedAt: null, cancelling: true }),
    one("Cancellation pending · launch unconfirmed", silent, { status: "submitting", startedAt: null, cancelling: true, contact: unknown }),
    one("Cancellation pending · unreachable", plain, { cancelling: true, contact: sshUnreachable }, { retryHost: true }),
    one("Cancellation pending · needs authentication", plain, { cancelling: true, contact: sshAuth }),
    one("Cancellation pending · stop unconfirmed", plain, { cancelling: true, contact: unknown }),
    one("Cancellation pending · stop refused", plain, { cancelling: true, cancelError: "The scheduler refused the stop request." }),
    one("Cancelled · processes still running", plain, { ...stopped, contact: { type: "stopping", since: now - 60_000, count: 3 } }, { retryHost: true }),
    one("Cancelled · unreadable processes", plain, { ...stopped, cancelUnconfirmed: [1042, 1043] }),
    one("Cancelled · processes could not be checked", plain, { ...stopped, cancelUnconfirmed: [] }),
    one("Abandoned · host still named", plain, { ...end("lost"), lostReason: "abandoned", contact: sshUnreachable }),
    one("Abandoned · no host", plain, { ...end("lost"), lostReason: "abandoned", contact: unknown, host: null }),
  ] },
  { name: "Numbers and charts", specimens: [
    one("Known headline · one series"),
    one("Known headline · no chart match", { ...plain, view: { ...plain.view, blocks: [{ chart: ["loss"], phase: "eval" }] } }),
    one("Unknown headline · chart still visible", { ...plain, latest: {} }),
    one("NaN headline · gap in the chart", { ...plain, latest: { loss: "NaN" },
      series: { loss: [[0, 3.1], [200, 2.8], [480, null], [730, 2.45], [890, 2.43], [1240, null]] },
    }),
    one("Zero headline · parent had a value", { ...plain, latest: { loss: 0 },
      series: { loss: [...plain.series.loss.slice(0, -1), [1240, 0]] },
      versus: { step: { loss: { ...comparison, d: -2.5, mag: 2.5 } }, final: {} },
    }),
    one("Headline tied with the parent", { ...plain, versus: { step: { loss: { ...comparison, d: 0, mag: 0, parent: 2.31, flat: true } }, final: {} } }, {}, { level: "line" }),
    one("Headline worse than the parent", { ...plain, versus: { step: { loss: { ...comparison, d: 0.19, good: false, parent: 2.12 } }, final: {} } }),
    one("Several series · table grouped by batch size", sweep),
    one("Ghost parent line", pipeline),
    one("Log axes · positive points over several orders", { ...plain,
      view: { ...plain.view, blocks: [{ chart: ["loss"], xScale: "log", yScale: "log" }] },
      series: { loss: [[1, 100], [10, 8], [100, 0.7], [1000, 0.01]] } }),
    one("Point chart", { ...sweep, view: { ...sweep.view, blocks: [{ chart: ["val_loss"], from: "configs", x: "lr", mark: "points" }] } }),
    one("Bar chart · one row per category", { ...sweep,
      rows: { configs: sweep.rows.configs.filter((row, index, rows) => index === rows.findIndex((other) => other.batch === row.batch)) },
      view: { ...sweep.view, blocks: [{ chart: ["val_loss"], from: "configs", x: "batch", mark: "bars" }] },
    }),
    one("Bar chart · repeated categories", { ...sweep,
      view: { ...sweep.view, blocks: [{ chart: ["val_loss"], from: "configs", x: "batch", mark: "bars" }] },
    }),
  ] },
  { name: "Several runs", specimens: [
    { label: "2 runs · one leader in a shared label group", runs: batch(2, sweepNames), level: "line" },
    { label: "2 runs · watcher summary", runs: watchedBatch, level: "line" },
    { label: "2 runs · counted Setup takes priority", runs: [
      sample(sweepNames(0), { ...setup, progress: { ...setup.progress!, done: 35 } }),
      sample(sweepNames(1), plain, {}, "r2"),
    ], level: "line" },
    { label: "2 runs · time until the next finishes", runs: batch(2, sweepNames, plain, true, (index) => index === 0 ? "done" : "running"), level: "line" },
    { label: "2 runs · lower bound until the first finishes", runs: batch(2, sweepNames, { ...plain, estimate: { ...estimate, floor: true } }, false), level: "line" },
    { label: "2 runs · a lower bound and an untimed run", runs: [
      sample(sweepNames(0), { ...plain, estimate: { ...estimate, floor: true } }),
      sample(sweepNames(1), { ...plain, estimate: null }, {}, "r2"),
    ], level: "line" },
    { label: "6 runs · folded, shared labels and a tie", runs: batch(6, sweepNames, plain, false), level: "line" },
    { label: "6 runs · expanded, mixed outcomes", runs: batch(6, sweepNames, plain, true, (index) => (["submitting", "running", "done", "failed", "cancelled", "lost"] as const)[index]) },
    { label: "6 runs · two independent leaders", runs: batch(6, (index) => index < 3 ? `ResNet seed ${index}` : `ViT patch ${index}`), level: "line" },
    { label: "24 runs · folded, first estimate and short context", runs: batch(24, sweepNames, plain, false), level: "line" },
    { label: "24 runs · scrollable list", runs: batch(24, sweepNames) },
    { label: "Sweep finished · leader and Dismiss all", runs: batch(4, sweepNames, plain, true, () => "done"), level: "line" },
    { label: "Sweep paused by a flag · rows open automatically", runs: [sample(sweepNames(0), warning("rule", "waiting")), ...batch(3, sweepNames).slice(1)], level: "line" },
    { label: "A run launched from another chat", runs: [sample(sweepNames(0)), sample(sweepNames(1), warning("tripwire", "waiting"), { chatSessionId: "another-chat" }, "r2")] },
    { label: "Shown runs could not be read · Retry", runs: [], failedList: true },
    { label: "Older server · active runs without monitors", runs: batch(2, sweepNames), oldServer: true },
  ] },
  { name: "Text stress", specimens: [
    one("Long experiment name · stage-card title", plain, {}, { level: "line", pin: "stage" },
      "CIFAR-10 image classification with the residual branch rescaled after normalization and a cosine restart before each held-out validation pass, seeded separately across every augmentation"),
    one("Long watcher sentence · line and full stage card", { ...running, sentence: { t: now,
      template: "Validation loss is **{loss}** after the second restart. The first phase finished once the augmented input cache warmed up. Several additional evaluation passes are still running and will report separately for each held-out dataset." } }, {}, { level: "line", pin: "stage" }),
    one("Huge value and count", {
      ...plain, view: automaticFormat, latest: { loss: 8.7654321e21 },
      phases: [{ ...plain.phases[0], of: 1e19 }],
      progress: { ...plain.progress!, done: 1234567890123456 },
      series: { loss: [[0, 3e22], [1234567890123456, 8.7654321e21]] },
      x: { name: "step", max: 1e19 },
    }),
    one("Tiny value", {
      ...plain, view: automaticFormat, latest: { loss: 2.1e-13 },
      series: { loss: [[0, 3.7e-12], [350, 1.2e-12], [800, 4.3e-13], [1240, 2.1e-13]] },
    }),
    one("RTL experiment, stage, host and mixed-script watcher text", { ...running, phases: [{ ...running.phases[0], name: "التدريب" }],
      sentence: { t: now, template: "انخفضت الخسارة إلى **{loss}** بعد تشغيل ResNet-50 مرة أخرى." } }, { host: "مختبر-الحوسبة" }, { level: "line", pin: "stage" }, "تجربة تدريب ResNet-50 على مجموعة بيانات الصور"),
  ] },
];
