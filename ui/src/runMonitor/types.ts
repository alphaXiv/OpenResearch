import type { ChatActiveRun } from "../api";

/** A measurement as the server keeps it; JSON can't carry NaN, so it arrives as "NaN", "Infinity" or "-Infinity". */
export type SignalValue = number | string | null;
export type Point = [number, number | null];

export type FlagState = "raised" | "waking" | "agent_looking" | "waiting" | "asking";
export type FlagMode = "agentic" | "copilot";

/** How a run's signal compares with its parent's; `mag` is already rounded to the signal's digits. */
export interface Versus {
  d: number;
  good: boolean;
  flat: boolean;
  mag: number;
  parent: number;
  /** The step both were taken at; absent when compared with the value the parent ended with. */
  step?: number;
}

/** At the same stage and step, or with the value the parent ended with. */
export type VersusBy = "step" | "final";

/** The watcher's words: tokens `{signal}`, `{signal.vs}` and `{count:"pattern"}`, with one `**bold**` figure. */
export interface Sentence {
  template: string;
  t: number;
}

interface RunRule {
  id: string;
  name: string;
  text: string;
  does: "stop" | "ask";
}

export interface SignalSpec {
  label: string;
  unit?: string;
  digits?: number;
  better?: "lower" | "higher";
  smooth?: number;
  as?: "duration" | "compact";
  scale?: number;
  sign?: boolean;
  /** Stats show the best value so far, as `RunMonitor.best` gives it, instead of the latest. */
  pick?: "latest" | "best";
  /** A cells signal's reported values and the outcome each one means. */
  values?: Record<string, "correct" | "wrong" | "error">;
  /** The word shown for a raw value, in stats, tables and the headline: `{"1": "valid"}`. */
  labels?: Record<string, string>;
  /** How it compares with the parent; at the same step while the run goes and with where the parent ended once it ended when absent. */
  vs?: VersusBy;
}

/** One line of a chart drawn from a table, with its own x column. */
export interface ChartSeries {
  x: string;
  y: string;
  label?: string;
  /** False keeps the parent's line from being drawn under this one, as for a reference the run measures itself. */
  ghost?: boolean;
}

export type Signals = Record<string, SignalSpec>;

/** Signals over steps, or columns of a table. */
export interface ChartBlock {
  /** Signals over `x`, or columns of `from`; empty when `series` names the lines. */
  chart: string[];
  /** "step" when absent; with `from`, a column of that table. */
  x?: string;
  /** Draws from this table's rows instead of signals over steps. */
  from?: string;
  series?: ChartSeries[];
  /** One line per value of this column of `from`. */
  by?: string;
  xScale?: "log";
  yScale?: "log";
  /** Over steps, the axis reaches the parent's end too; it spans the run's own total, the parent's line cut at the edge, when absent. */
  xRange?: "parent";
  /** A line over steps, and a line with points from a table, when absent. */
  mark?: "line" | "points" | "bars";
  ghost?: "parent";
}

export type ViewBlock = { phase?: string; title?: string } & (
  | ChartBlock
  | { stats: string[]; vs?: VersusBy }
  | { table: string; columns: string[]; last?: number; vs?: "parent" | VersusBy }
  | { bars: string; by: string }
  | { cells: string }
  | { media: string; last?: number; columns?: number; size?: "s" | "m" | "l"; caption?: string }
  | { text: string }
);

export interface ViewTree {
  signals: Signals;
  headline: string[];
  blocks: ViewBlock[];
}


interface Look {
  id: string;
  at: number;
  /** The bar's fraction at the look; null when the run has no total and no limit. */
  whole: number | null;
  said: "same" | "new" | "flag";
  /** Filled with the numbers of its moment. */
  text: string;
  template: string;
  locale: string;
}

interface Evidence {
  lines: string[];
  /** 1-based, inclusive. */
  from: number;
  to: number;
  ident: string;
  byteFrom: number;
  byteTo: number;
}

export interface Flag {
  id: string;
  at: number;
  whole: number | null;
  template: string;
  detail: string;
  evidence: Evidence | null;
  state: FlagState | "kept" | "answered" | "stopped";
  mode: FlagMode;
  choice?: { by: "user" | "agent"; c: "stop" | "keep" | "ask"; at: number };
}

/** What a run is doing before it reports a step: the newest thing its log said since its stage began. */
export interface Setup {
  /** Trimmed; null when it printed nothing or only a credential. */
  line: string | null;
  /** The stage the run declared and has not reported a step of; null while its log says nothing orx recognises. */
  stage: string | null;
  /** When an unrecognised log stops reading as setting up; null for a declared stage. */
  until: number | null;
}

/** What every `Run` carries, so a line, a badge or the sidebar needs no extra request. */
export interface RunMonitorSummary {
  tone: "starting" | "running" | "flag" | "done" | "failed" | "stopped" | "revising";
  whole: number | null;
  /** Where the run stands in its current stage, so a line can say it without the full monitor; absent from an older server. */
  stage?: { name: string; done: number; of: number | null; unit: string | null } | null;
  headline: { name: string; label: string; value: SignalValue; vs: Versus | null } | null;
  sentence: Sentence | null;
  /** Each referenced signal, `signal.vs` as the raw difference, and `count:"pattern"` keys. */
  bindings: Record<string, SignalValue>;
  signals: Signals;
  etaMs: { lo: number; mid: number; hi: number; at: number } | null;
  /** `etaMs` times a bar or counter read from the log, so more of the run may follow it; absent from an older server. */
  etaFloor?: boolean;
  /** The plan's stages `etaMs` leaves out, being untimed, which make it a floor; absent from an older server. */
  etaUntimed?: string[];
  /** One per stage of a declared plan, so a line's underline is segmented; absent without a plan or from an older server. */
  segments?: { weight: number; fill: number | null }[];
  /** The estimate waits on more progress; with no `etaMs` and not pending, how much is left is unknown. */
  pending: boolean;
  /** Why this summary stopped updating, while the run's projection fails. */
  unavailable?: string;
  /** When the run's time limit ends; lets a line count down to it without the full monitor. */
  limitEndsAt: number | null;
  /** The unsettled flag only. */
  flag: { id: string; state: FlagState; mode: FlagMode } | null;
  /** Present while a live run's log has said nothing orx recognises, or its stage has no step yet; absent from an older server. */
  setup?: Setup | null;
  rev: number;
  generation: string;
  asOf: number;
}

export interface RunOutcome {
  by: "user" | "agent" | "rule";
  rule?: RunRule;
  at: number;
}

export interface Phase {
  name: string;
  of: number | null;
  unit: string | null;
  priorMs: number | null;
  startedAt: number | null;
  endedAt: number | null;
  /** Whether the stage is in the run's declared plan; absent when the run has no plan, false for a stage the plan didn't name. */
  planned?: boolean;
  /** A planned stage the run went past, or finished without. */
  skipped?: boolean;
}

export interface Progress {
  phase: number;
  /** Null while a planned stage has reported no step, or before the run's first stage began. */
  done: number | null;
  /** `fill` is null when the phase has no total and no limit: an empty track, never a zero. */
  segments: { name: string; weight: number; fill: number | null }[];
  whole: number | null;
  /** When the last report was placed; null when only its duration is known. */
  lastAt: number | null;
  /** Null until two reports give a cadence to measure a stall against. */
  stallMs: number | null;
}

export interface Estimate {
  loMs: number | null;
  midMs: number | null;
  hiMs: number | null;
  at: number;
  pending: boolean;
  pastLimitMs: number | null;
  /** Times a bar or counter read from the log, or leaves out `untimed` stages, so more of the run may follow it. */
  floor: boolean;
  /** Stages of the plan, this one or to come, that no earlier run timed: time left is at least the rest. */
  untimed?: string[];
  /** Why parts of the estimate could not be computed. */
  reasons?: string[];
}

export interface Cost {
  soFar: number | null;
  projected: number | null;
  atLimit: number | null;
  spend: number | null;
  budget: number | null;
  gpuHours: number | null;
  at: number;
  projectedRange: [number, number] | null;
  uncertain: boolean;
}

type QuoteSource =
  | { kind: "earlier_run"; runNo: number; hardware: string; runId: string }
  | { kind: "parent_same_hw" | "parent_other_hw"; hardware: string; runId: string }
  | { kind: "agent"; explanation: string }
  | { kind: "none" };

export interface Quote {
  loMs: number | null;
  hiMs: number | null;
  usd: [number, number] | null;
  spendCap: number | null;
  from: QuoteSource;
}

interface Watch {
  state: "on" | "off" | "no_model" | "needs_pick";
  model: string | null;
  looks: Look[];
  spentUsd: number | null;
  nextAt: number | null;
  error: { code: string; message: string } | null;
}

/** One outcome per item, oldest first: 1 correct, 0 wrong, 2 an error, null not known. `from` items came before the first. */
export interface Cells {
  of: number | null;
  outcomes: (0 | 1 | 2 | null)[];
  from: number;
}

/** A file under the project's artifacts that a media block shows, newest last. */
export interface MediaItem {
  path: string;
  kind: "image" | "video" | "audio";
  at: number;
  caption?: string;
}

/** `GET /api/runs/:id/monitor`: the whole live state of one run. */
export interface RunMonitor {
  runId: string;
  rev: number;
  generation: string;
  asOf: number;
  /** Anything but live is old data, said as such. */
  availability: { state: "live" | "stale" | "rebuilding" | "unavailable"; reason: string | null };
  view: ViewTree;
  /** Where the view comes from: the experiment that stores it, or drawn from the run's first signal. */
  viewFrom: { experimentId: string; markup: string; isDefault: boolean };
  /** How the run's signals are read; null before any arrived. */
  source: "declared" | "pattern" | "detected" | "trackio" | null;
  /** The experiment's pattern, when `source` is a pattern. */
  pattern?: string;
  /** The newest log line the run's signals came from. */
  newestLine?: string;
  phases: Phase[];
  progress: Progress | null;
  x: { name: string; max: number | null };
  latest: Record<string, SignalValue>;
  /** The best value so far of each `pick: "best"` signal, by its `better`; absent from a server that doesn't say. */
  best?: Record<string, SignalValue>;
  series: Record<string, Point[]>;
  seriesRev: number;
  /** Each of the view's tables, row by row. */
  rows: Record<string, Record<string, SignalValue>[]>;
  /** A bars block's signal by its key; `value` is `ok` of `n`. */
  groups: Record<string, { key: string; value: number | null; ok: number; n: number }[]>;
  cells: Record<string, Cells>;
  media: Record<string, MediaItem[]>;
  /** `rows` holds the parent's tables a table with `vs` compares with, matched by their first column. */
  parent: { runId: string; phase: string | null; series: Record<string, Point[]>; rows?: Record<string, Record<string, SignalValue>[]> } | null;
  /** Each signal compared its own way. */
  versus: Record<string, Versus>;
  /** Each signal compared both ways, for a block that names its own; absent from an older server. */
  versusBy?: Record<VersusBy, Record<string, Versus>>;
  counts: Record<string, number>;
  estimate: Estimate | null;
  limit: { endsAt: number } | null;
  cost: Cost | null;
  quote: Quote | null;
  before: { kind: "queued" | "starting"; startsAt: number | null; text: Sentence | null } | null;
  watch: Watch;
  sentence: Sentence | null;
  flag: Flag | null;
  setup?: Setup | null;
  /** The chat's agent is editing the experiment after this stopped run. */
  revising: boolean;
  rules: RunRule[];
  /** Exactly what `orx exp status` prints for the run. */
  statusText: string;
}

/** The `run.monitor` event: scalars replace, `append` extends series that share the cached basis. */
export interface RunMonitorEvent {
  runId: string;
  rev: number;
  generation: string;
  asOf: number;
  seriesRev: number;
  appendFrom: number;
  append: Record<string, Point[]>;
  /** Rows each of the view's tables gained, on the same basis as `append`. */
  appendRows?: Record<string, Record<string, SignalValue>[]>;
  /** How many rows left the front of each table, before `appendRows` are added. */
  dropRows?: Record<string, number>;
  latest: Record<string, SignalValue>;
  counts: Record<string, number>;
  progress: Progress | null;
  estimate: Estimate | null;
  cost: Cost | null;
  sentence: Sentence | null;
  watch?: Watch;
  flag?: Flag | null;
  reset?: true;
  patch?: Partial<RunMonitor>;
}

/** `GET /api/chat/shown-runs`: each chat's runs above its composer, ended ones until the agent has said what came of them. */
export interface ChatShownRun extends ChatActiveRun {
  experimentSlug: string;
  runNo: number;
  runCount: number;
}

/** An older server has no shown runs, so the page falls back to its active runs, which don't say these. */
export type ShownRun = ChatActiveRun & Partial<Pick<ChatShownRun, "experimentSlug" | "runNo" | "runCount">>;
