import type { ChatActiveRun } from "../api";

/** A measurement as the server keeps it; JSON can't carry NaN, so it arrives as "NaN", "Infinity" or "-Infinity". */
export type SignalValue = number | string | null;
export type Point = [number, number | null];
export type Bindings = Record<string, SignalValue | Versus>;

export type FlagState = "raised" | "waking" | "agent_looking" | "waiting" | "asking";

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

/** A rule the run was launched with, which orx checks on each report with no model. */
export interface RunRule {
  name: string;
  /** The agent's words for it. */
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
export type ChartEdit =
  | { kind: "x" | "y"; log: boolean }
  | { kind: "smooth"; signal: string; weight: number | null };

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
  ghost?: "parent" | "false";
}

export type ViewBlock = { phase?: string; title?: string } & (
  | ChartBlock
  | { stats: string[]; vs?: VersusBy }
  | { table: string; columns: string[]; last?: number; vs?: "parent" | VersusBy }
  | { bars: string; by: string }
  | { cells: string }
  | { media: string; last: number }
  | { text: string }
);

export interface ViewTree {
  signals: Signals;
  headline: string[];
  blocks: ViewBlock[];
}


export type BriefLabel = "question" | "healthy" | "noise" | "worth_a_flag" | "from_parent" | "decided";

export interface Look {
  id: string;
  at: number;
  /** What brought it: a change, the longest silence, a person, the run's end. */
  kind: "change" | "ceiling" | "manual" | "terminal";
  /** The bar's fraction at the look; null when the run has no total and no limit, or the look said nothing. */
  whole: number | null;
  /** `error` and `interrupted` looks said nothing; `text` is why. */
  said: "same" | "new" | "flag" | "error" | "interrupted";
  /** Filled with the numbers of its moment, in English. */
  text: string;
  template: string | null;
  /** What the template's tokens read at the look, so a row says the numbers of its moment. */
  bindings: Bindings | null;
  /** The other runs this look covered, by name. */
  with: string[];
  /** What made it due, in English, for the record. */
  why: string;
  /** The stage the run was in, whose bar `whole` is a share of when the run has no plan. */
  stage: string | null;
  /** Where this look first left out what looks like a credential, in English. */
  withheld: string[];
  /** What this look left out to keep within its token budget, in English; absent when it left nothing out. */
  omitted?: string | null;
  /** When the flag was raised that was open as this look flagged, which noted it instead; absent when none was. */
  notedUnder?: number | null;
}

interface Evidence {
  lines: string[];
  /** 1-based, inclusive; null when the log is too long to count its lines. */
  from: number | null;
  to: number | null;
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
  text?: string;
  /** Values from the look that raised the flag, when still available. */
  bindings?: Bindings | null;
  evidence: Evidence | null;
  state: FlagState | "kept" | "stopped";
  /** The choice that settled it, or the last that sent it to someone. */
  choice?: {
    /** Null when a stop came from an orx that kept no record of who asked. */
    by: "user" | "agent" | "rule" | null;
    c: "stop" | "keep" | "ask";
    at: number | null;
    reason?: string | null;
    /** The stopping rule's name. */
    rule?: string | null;
    /** The run was stopped some other way, so this flag had nothing left to choose. */
    moot?: boolean;
    /** Kept because the agent ended two turns without settling it. */
    unsettled?: boolean;
  };
  /** What raised it: a look of the watcher, a check orx makes itself, or a rule that asks. */
  source: "look" | "tripwire" | "rule";
  /** The look that raised it, which the journal shows it under. */
  lookId?: string;
  /** What later looks flagged while this one was open, oldest first; absent when none did. */
  noted?: { at: number; text: string; lookId: string }[];
}

/** A live run before its first displayed point or detected work. */
export interface Setup {
  /** Trimmed; null when it printed nothing or only a credential. */
  line: string | null;
  /** The current named stage, when the log has declared one. */
  stage: string | null;
  /** Expected first headline or chart point, from an earlier run's stage timings. */
  startsAt: number | null;
}

/** Raw phase positions; `first` bounds the stages folded into Setup. */
export interface StageOrder {
  index: number | null;
  /** Null without a declared plan. */
  count: number | null;
  /** The first displayed point or detected work stage, or an earlier run's matching stage. */
  first: number | null;
}

/** What every `Run` carries, so a line, a badge or the sidebar needs no extra request. */
export interface RunMonitorSummary {
  tone: "starting" | "running" | "flag" | "done" | "failed" | "stopped" | "revising";
  whole: number | null;
  /** Where the run stands in its current stage, so a line can say it without the full monitor. */
  stage: { name: string; done: number | null; of: number | null; unit: string | null } | null;
  stageOrder: StageOrder;
  headline: { name: string; label: string; value: SignalValue } | null;
  sentence: Sentence | null;
  /** Each referenced signal, `signal.vs` as the parent comparison, and `count:"pattern"` keys. */
  bindings: Bindings;
  signals: Signals;
  estimate: Omit<Estimate, "pastLimitMs"> | null;
  /** Timed stages of the whole run, or one current-stage fill when its share is unknown. */
  segments?: Progress["segments"];
  /** Why this summary stopped updating, while the run's projection fails. */
  unavailable?: string;
  /** Why this summary's signal data is no longer live. */
  availability?: { state: "stale" | "rebuilding"; reason: string | null };
  /** When the run's time limit ends; lets a line count down to it without the full monitor. */
  limitEndsAt: number | null;
  /** The unsettled flag only. */
  flag: { id: string; state: FlagState; template?: string | null; text?: string | null; bindings?: Bindings | null } | null;
  /** Present until a headline or chart signal has its first point. */
  setup: Setup | null;
  /** For a failed run that recorded no message, the line in its log that says why. */
  logError: string | null;
  rev: number;
  generation: string;
  asOf: number;
}

export interface RunOutcome {
  by: "user" | "agent" | "rule";
  rule?: RunRule;
  at: number;
  /** The agent's `--reason`, or what a rule saw, in English. */
  reason?: string | null;
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
  segments: { weight: number; fill: number | null }[];
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
  pastLimitMs?: number | null;
  /** Times only known stages, so more of the run may follow it. */
  floor: boolean;
  untimed?: string[];
}

export interface Cost {
  soFar: number | null;
  projected: number | null;
  spend: number | null;
  budget: number | null;
}

export interface Watch {
  /** `none` when no chat follows the run; `unknown` when the server could not read what the watcher said. */
  state: "on" | "off" | "no_model" | "needs_pick" | "none" | "unknown";
  model: string | null;
  /** Why the run is not watched, in the server's words, when it is not. */
  why?: string | null;
  looks: Look[];
  /** What the looks cost, each split over the runs it covered; null when no look reported a price. */
  spentUsd: number | null;
  tokens?: number;
  /** When the next look is due at the latest, while the run keeps printing. */
  /** Look now starts no look before this. */
  floorUntil?: number | null;
  error: { code: string; message: string } | null;
}

export interface MediaItem {
  path: string;
  version: string;
}

export interface MediaGallery {
  /** Run-relative previews, newest first. */
  items: MediaItem[];
  skipped?: number;
  truncated?: boolean;
  error?: { kind: "unsupported"; backend: string } | { kind: "unreadable"; detail: string };
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
  phases: Phase[];
  stageOrder: StageOrder;
  progress: Progress | null;
  x: { name: string; max: number | null };
  latest: Record<string, SignalValue>;
  /** The best value so far of each `pick: "best"` signal, by its `better`; absent from a server that doesn't say. */
  best?: Record<string, SignalValue>;
  series: Record<string, Point[]>;
  seriesRev: number;
  /** Each of the view's tables, row by row. */
  rows: Record<string, Record<string, SignalValue>[]>;
  /** A bars block's retained outcome counts, keyed by table and grouping column. */
  groups: Record<string, Record<string, { key: string; ok: number; n: number }[]>>;
  media: Record<string, MediaGallery>;
  /** `rows` holds the parent's tables a table with `vs` compares with, matched by their first column. */
  parent: { runId: string; phase: string | null; series: Record<string, Point[]>; rows?: Record<string, Record<string, SignalValue>[]> } | null;
  /** For a failed run that recorded no message, the line in its log that says why. */
  logError: string | null;
  /** A stage whose steps take at least twice as long as those of the run that timed it: how many times, against which run. */
  pace?: { times: number; than: "parent" | "earlier" } | null;
  /** A plan that counts work the run reports only as rows, with no step for its bar to move by. */
  disagrees?: { stage: string; of: number; unit: string | null; table: string; rows: number } | null;
  /** The signal the run leads with: one that went wrong (non-finite, or zero where the parent's isn't), else the view's first headline with a number. */
  headline: string | null;
  /** Each signal compared both ways; `versusOf` picks the way a block or the signal asks for. */
  versus: Record<VersusBy, Record<string, Versus>>;
  counts: Record<string, number>;
  estimate: Estimate | null;
  limit: { endsAt: number } | null;
  cost: Cost | null;
  watch: Watch;
  sentence: Sentence | null;
  flagId: string | null;
  /** Every flag raised on the run, oldest first, with what became of it. */
  flags: Flag[];
  setup: Setup | null;
  /** The chat's agent is repairing the experiment after this stopped run. */
  revising: boolean;
  /** What the watcher is told of the experiment, label by label. */
  brief: { label: BriefLabel; text: string }[];
  rules: RunRule[];
  /** Exactly what `orx exp status` prints for the run. */
  statusText: string;
}

/** A reset rereads the monitor. Otherwise only the series and table histories need merging. */
export type RunMonitorEvent =
  | (Pick<RunMonitor, "runId" | "rev" | "generation"> & { reset: true; watch?: Watch })
  | (Omit<RunMonitor, "view" | "parent" | "series" | "rows"> & {
    reset?: never;
    appendFrom: number;
    append: RunMonitor["series"];
    appendRows: RunMonitor["rows"];
    dropRows: Record<string, number>;
  });

/** `GET /api/chat/shown-runs`: each chat's runs above its composer, ended ones until the agent has said what came of them. */
export interface ChatShownRun extends ChatActiveRun {
  experimentSlug: string;
  runNo: number;
  runCount: number;
  /** The chat's newest look: the runs it covered under their keys, and its summary sentence. */
  watch?: ChatWatch | null;
}

export interface ChatWatch {
  lookId: string;
  at: number;
  runIds: string[];
  /** `r1` → run id: what `{r1}` and `{r1:signal}` in the summary name. */
  keys: Record<string, string>;
  summary: { template: string } | null;
}

/** An older server has no shown runs, so the page falls back to its active runs, which don't say these. */
export type ShownRun = ChatActiveRun & Partial<Pick<ChatShownRun, "experimentSlug" | "runNo" | "runCount" | "watch">>;
