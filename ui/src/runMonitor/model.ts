import { backendDetail, backendKind, type Run, type RunStatus } from "../api";
import { backendName } from "../components/BackendLogos";
import { runNoticeCopy } from "../components/RunContactNotice";
import { statusLabel } from "../components/StatusBadge";
import { autoDir, ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { runActive, runApiProvider, runConnectHost, runDisplay, runNotice, runStoppable, type RunNotice } from "../runDisplay";
import { fillSentence, fmtMagnitude, fmtSignal, plainSentence, tokensOf } from "./format";
import type { Flag, FlagMode, FlagState, RunMonitor, RunMonitorSummary, ShownRun, SignalSpec, SignalValue, Signals, Versus, VersusBy } from "./types";
import { capitalized, captionProgress, clockText, costWords, durationText, estimateWords, progressWords, quoteWords, stageWords } from "./words";

/** How the headline compares with the parent: the arrow follows the number, the colour whether that is better. */
export interface Delta {
  down: boolean;
  good: boolean;
  flat: boolean;
  magnitude: string;
}

export type Tone = "starting" | "running" | "flag" | "done" | "failed" | "stopped" | "revising" | "lost";

export interface RunModel {
  run: Run;
  sessionId: string;
  /** The experiment's slug, else its title from an older server. */
  name: string;
  /** Backend and machine: "Modal · A10G". */
  where: string;
  tone: Tone;
  live: boolean;
  /** Submitted and not yet running. */
  pre: boolean;
  /** The state a run ended in, or is in, said before its sentence: "Auto-stopped". */
  lead: string | null;
  /** The status word the mark and announcements read. */
  status: string;
  sentence: {
    /** The one figure that matters is wrapped in `**`; tokens are filled. */
    text: string;
    /** Changes when the words do, never when a number refills. */
    key: string;
    /** When the watcher wrote it. */
    at: number | null;
  };
  /** The run's one number, drawn only where the sentence doesn't already say it. */
  headline: {
    name: string;
    label: string;
    value: string;
    /** The value to rank by: a finite number, else null. */
    number: number | null;
    better: SignalSpec["better"];
    known: boolean;
    delta: Delta | null;
    inSentence: boolean;
    vsInSentence: boolean;
  } | null;
  eta: string;
  /** How long until a live run should finish, as `eta` says it; `floor` when that is only its least. */
  left: { ms: number; floor: boolean } | null;
  etaDetail: string;
  /** Whether `eta` is a live countdown, drawn heavier in the strip. */
  etaStrong: boolean;
  progress: string;
  cost: string;
  /** A null fill is an empty track: the run has no total and no limit to fill against. */
  bar: { segments: { weight: number; fill: number | null }[]; whole: number | null; frozen: boolean; label: string };
  ticks: { id: string; at: number; flag: boolean; text: string }[];
  /** The unsettled flag as the newest source states it; `record` is its evidence and words, once a monitor has them. */
  flag: { id: string; state: FlagState; mode: FlagMode; record: Flag | null } | null;
  /** What the watcher found, in full, while the flag is unsettled; the strip says it in place of the sentence. */
  flagDetail: string | null;
  /** A Copilot flag waits on the user; the strip opens by itself. */
  needsChoice: boolean;
  /** The chat's agent is reading a flag. */
  looking: boolean;
  /** What only the strip says under the sentence: a rule's detail, or that the agent is revising. */
  note: string | null;
  /** Lost contact, a pending cancel and the like, worded by `runNoticeCopy`. */
  notice: RunNotice | null;
  /** The run's view is older than it looks: when it was last seen, and in its state's own words why. */
  stale: { state: "stale" | "rebuilding" | "unavailable" | "failed"; age: string; why: string } | null;
  connectHost: string | null;
  canStop: boolean;
  canDismiss: boolean;
}

/** Notices about a run's log or its message, which only the experiment page shows. */
const PAGE_ONLY: ReadonlySet<RunNotice["kind"]> = new Set(["log_pending", "log_failed", "notice_failed"]);

/** Processes that may still be running keep the run on screen until they are cleaned up. */
const UNDISMISSABLE: ReadonlySet<RunNotice["kind"]> = new Set(["not_fully_stopped", "cancelled_unconfirmed"]);

const STATUS_TONE: Record<RunStatus, Tone> = {
  submitting: "starting",
  starting: "starting",
  running: "running",
  done: "done",
  failed: "failed",
  cancelled: "stopped",
  lost: "lost",
};

/** A run's tone as the mark and the bar draw it. */
export const TONE_BG: Record<Tone, string> = {
  starting: "bg-mark-amber",
  running: "bg-mark-teal",
  flag: "bg-mark-amber",
  done: "bg-mark-green",
  failed: "bg-accent-red",
  stopped: "bg-accent-orange",
  revising: "bg-accent-purple",
  lost: "bg-mark-amber",
};

/** A tone as text: the accent mixed with the text colour, to 4.5:1 in both themes. */
export const TONE_INK: Record<Tone, string> = {
  starting: "text-ink-amber",
  running: "text-ink-teal",
  flag: "text-ink-orange",
  done: "text-ink-green",
  failed: "text-ink-red",
  stopped: "text-ink-orange",
  revising: "text-ink-purple",
  lost: "text-ink-orange",
};

/** The longest log line a sentence quotes. */
const SETUP_LINE = 120;

const bold = (value: string) => `**${value}**`;

/** A log line as one line of a sentence: its whitespace collapsed, shortened, its `*` kept from drawing bold. */
function quoted(line: string): string {
  const flat = line.replace(/\s+/g, " ").replaceAll("*", "∗").trim();
  const chars = [...flat];
  return ltr(chars.length > SETUP_LINE ? `${chars.slice(0, SETUP_LINE - 1).join("")}…` : flat);
}

/** The bar's accessible text: its parts as one sentence, none keeping a stop of its own. */
function barLabel(parts: string[]): string {
  const text = parts.map((part) => plainSentence(part).trim().replace(/[.。۔]$/u, "")).filter(Boolean).join(", ");
  return text && `${text}.`;
}

function where(run: Run): string {
  const kind = backendKind(run.backend);
  const detail = backendDetail(run.backend) || run.host;
  return [kind ? backendName(kind) : null, detail].filter(Boolean).map((part) => autoDir(String(part))).join(" · ");
}

/**
 * How a signal compares with the parent: the way a block names, else the signal's own, else at the same step while
 * the run goes and with where the parent ended once it ended.
 */
export const versusOf = (monitor: RunMonitor, name: string, live: boolean, by?: VersusBy): Versus | null =>
  monitor.versus[by ?? monitor.view.signals[name]?.vs ?? (live ? "step" : "final")][name] ?? null;

/** What a sentence's tokens read from a full monitor: each signal, `signal.vs` as the raw difference, and counts. */
export function bindingsOf(monitor: RunMonitor, live: boolean): Record<string, SignalValue> {
  const bindings: Record<string, SignalValue> = { ...monitor.latest };
  for (const name of Object.keys(monitor.view.signals)) {
    const vs = versusOf(monitor, name, live);
    if (vs) bindings[`${name}.vs`] = vs.d;
  }
  for (const [pattern, count] of Object.entries(monitor.counts)) bindings[`count:${JSON.stringify(pattern)}`] = count;
  return bindings;
}

const unsettled = (state: Flag["state"]): state is FlagState => state !== "kept" && state !== "answered" && state !== "stopped";

/**
 * Whether the run's summary is newer than its full monitor, which then states none of the run's facts. Newer is by
 * `rev` within a generation; generations have no order, so across one the later `asOf` wins.
 */
export function monitorBehind(summary: RunMonitorSummary | null | undefined, monitor: RunMonitor): boolean {
  if (!summary) return false;
  return summary.generation === monitor.generation ? summary.rev > monitor.rev : summary.asOf > monitor.asOf;
}

/** Whether a run's stages are worth listing: several, each named. Bars a log printed have no names. */
export const namedStages = (monitor: RunMonitor) => monitor.phases.length > 1 && monitor.phases.every((phase) => phase.name);

/** The unsettled flag as the newer of the summary and the monitor states it. */
export function statedFlag(run: Run, fetched: RunMonitor | null): RunMonitorSummary["flag"] {
  if (!fetched || monitorBehind(run.monitor, fetched)) return run.monitor?.flag ?? null;
  const { flag } = fetched;
  return flag && unsettled(flag.state) ? { id: flag.id, state: flag.state, mode: flag.mode } : null;
}

/** A run's tone from its newest source: the full monitor when it is that, else what every `Run` carries. */
export function runTone(run: Run, monitor: RunMonitor | null = null): Tone {
  const live = runActive(run.status);
  if (run.status === "lost" || (live && run.contact !== null && run.contact.type !== "stopping")) return "lost";
  if (monitor) {
    if (!live) return monitor.revising && run.status === "cancelled" ? "revising" : STATUS_TONE[run.status];
    return monitor.flag && unsettled(monitor.flag.state) ? "flag" : STATUS_TONE[run.status];
  }
  // The server reprojects an ended run whose summary still says it is going before it lists it.
  return run.monitor?.tone ?? STATUS_TONE[run.status];
}

export interface RunModelInput {
  item: ShownRun;
  /** The run's whole live state, once a strip has fetched it; it states the run's facts only while it is the newest source. */
  monitor: RunMonitor | null;
  /** The monitor's last refresh failed, so what it holds may be out of date. */
  monitorFailed?: boolean;
  /** Whether this server keeps shown runs; an older one lists only active runs. */
  shown: boolean;
  agent: string;
  now: number;
  /** When orx next tries an unreachable machine. */
  nextRetryMs: number | null;
}

/** One run at one moment, as the line and the strip read it. */
export function runModel({ item, monitor: fetched, monitorFailed = false, shown, agent, now, nextRetryMs }: RunModelInput): RunModel {
  const { run } = item;
  // Every fact comes from the newest source alone; the other is read only for what belongs to its flag or its history.
  const monitor = fetched && !monitorBehind(run.monitor, fetched) ? fetched : null;
  const summary = monitor ? null : run.monitor ?? null;
  const sameGeneration = fetched && run.monitor?.generation === fetched.generation ? fetched : monitor;
  const availability = monitor?.availability.state ?? "live";
  // The run's projection is failing, which reaches lines that never read its whole monitor too.
  const failing = run.monitor?.unavailable ?? null;
  const shownAsOf = monitor?.asOf ?? run.monitor?.asOf ?? now;
  const degraded = failing !== null || (monitor !== null && (availability !== "live" || monitorFailed));
  const live = runActive(run.status);
  const pre = run.status === "submitting" || run.status === "starting" || monitor?.before != null;
  const display = runDisplay(run);

  const pageOnly = runNotice(run);
  const notice = pageOnly && !PAGE_ONLY.has(pageOnly.kind) ? pageOnly : null;
  const tone = runTone(run, monitor);

  const outcome = run.outcome ?? null;
  const lead = tone === "revising" ? m.status_revising()
    : tone === "failed" ? m.status_failed()
    : tone === "stopped" ? (outcome && outcome.by !== "user" ? m.status_auto_stopped() : m.run_lead_stopped())
    : null;
  const status = tone === "lost" ? statusLabel(display.status, display.stopping)
    : lead ?? (tone === "flag" ? m.status_flagged() : tone === "done" ? m.status_done() : tone === "starting" ? m.status_starting() : m.status_running());

  const signals: Signals = monitor?.view.signals ?? summary?.signals ?? {};
  const bindings = monitor ? bindingsOf(monitor, live) : summary?.bindings ?? {};
  const phases = monitor?.phases ?? [];
  const progress = monitor?.progress ?? null;
  const startedAt = run.startedAt ?? run.createdAt;
  const elapsed = (live ? now : run.endedAt ?? run.updatedAt) - startedAt;
  const limit = monitor?.limit ? { endsAt: monitor.limit.endsAt, ms: monitor.limit.endsAt - startedAt } : null;
  // The summary says where the run stands too, so a line needs no full monitor to say it.
  const stage = summary?.stage ?? null;
  const stagePhase = progress ? phases[progress.phase] : stage ? { ...stage, priorMs: null, startedAt: null, endedAt: null } : undefined;
  // The server leaves a live run's estimate out exactly when how much of it remains is unknown.
  const open = live && !pre && (monitor ? monitor.estimate === null : summary !== null && summary.etaMs === null && !summary.pending);
  const limitEndsAt = monitor ? limit?.endsAt ?? null : summary?.limitEndsAt ?? null;
  const noEstimate = !stagePhase ? m.run_eta_no_progress() : stagePhase.of == null ? m.run_eta_no_total() : m.run_eta_unknown_left();
  const setup = (monitor ? monitor.setup : summary?.setup) ?? null;
  const settingUp = live && !pre && !degraded && !notice && !display.waiting && setup !== null && (setup.until === null || now < setup.until);
  const stageDone = progress?.done ?? stage?.done ?? null;
  const progressText = stageDone === null ? null : progressWords(stagePhase, stageDone);

  const stated = statedFlag(run, fetched);
  // A flag's words and evidence never change, so a lagging monitor still supplies them for the same flag.
  const flag = stated && { id: stated.id, state: stated.state, mode: stated.mode, record: fetched?.flag?.id === stated.id ? fetched.flag : null };
  const needsChoice = live && flag?.state === "waiting" && flag.mode === "copilot";
  const looking = live && (flag?.state === "waking" || flag?.state === "agent_looking" || flag?.state === "asking");

  const watched = monitor ? monitor.sentence : summary?.sentence ?? null;
  let sentence: RunModel["sentence"];
  if (notice) {
    const { lead } = runNoticeCopy(notice, null, runApiProvider(run), bold);
    const seen = progressText && live ? ` ${m.run_progress_last_seen({ progress: autoDir(progressText) })}.` : "";
    sentence = { text: lead + seen, key: `notice:${notice.kind}`, at: null };
  } else if (display.waiting) {
    const text = `${statusLabel(display.status)} · ${autoDir(display.waiting)}`;
    sentence = { text, key: `pre:waiting:${display.waiting}`, at: null };
  } else if (watched && !(tone === "stopped" && outcome?.by === "user")) {
    sentence = { text: fillSentence(watched.template, bindings, signals), key: `look:${watched.template}`, at: watched.t };
  } else if (!live) {
    // A finished run says how long it took instead, where the time goes.
    const text = !progressText || tone === "done" ? ""
      : outcome?.by === "user" ? m.run_auto_by_you({ progress: autoDir(progressText) })
      : m.run_auto_at({ progress: autoDir(progressText) });
    sentence = { text, key: `end:${tone}`, at: null };
  } else if (pre) {
    const before = monitor?.before ?? null;
    const text = before?.text ? fillSentence(before.text.template, bindings, signals)
      : before?.kind === "queued" && run.host ? (before.startsAt === null ? m.run_queued_on({ host: ltr(run.host) }) : m.run_queued_start_at({ host: ltr(run.host), clock: clockText(before.startsAt) }))
      : m.run_starting({ where: where(run) || m.compute_target_local() });
    sentence = { text, key: `pre:${before?.kind ?? "starting"}`, at: null };
  } else if (settingUp) {
    const begun = setup?.stage ? capitalized(m.run_stage_started({ stage: autoDir(setup.stage) })) : null;
    const text = begun ? (setup?.line ? `${begun} · ${quoted(setup.line)}` : begun)
      : setup?.line ? m.run_setting_up({ line: quoted(setup.line) }) : m.run_setting_up_on({ where: where(run) || m.compute_target_local() });
    sentence = { text, key: "setup", at: null };
  } else {
    const auto = stageDone === null ? "" : stageWords(stagePhase, stageDone);
    const text = auto ? capitalized(auto) : m.run_running_on({ where: where(run) || m.compute_target_local() });
    sentence = { text, key: "auto", at: null };
  }

  let eta = "";
  let etaDetail = "";
  let etaStrong = false;
  let left: RunModel["left"] = null;
  // Time since the run started, where it has no time left to show, said as that.
  let etaElapsed = false;
  const lastSeen = tone === "lost" || degraded;
  // What a run has cost only grows, so it keeps showing while the facts wait for the monitor to catch up.
  let cost = sameGeneration?.cost ? costWords(sameGeneration.cost, live, lastSeen) : "";
  const ended = tone === "done" ? m.run_done_in({ duration: durationText(elapsed) })
    : tone === "failed" ? m.run_failed_after({ duration: durationText(elapsed) })
    : tone === "stopped" || tone === "revising" ? m.run_eta_stopped_at({ duration: durationText(elapsed) })
    : "";
  if (!live) eta = ended;
  else if (notice?.kind === "unreachable") eta = nextRetryMs === null ? "" : m.run_notice_retry_at({ time: clockText(nextRetryMs) });
  else if (notice || degraded) eta = "";
  else if (pre) {
    const quote = monitor?.quote ? quoteWords(monitor.quote) : null;
    etaElapsed = !quote && !monitor;
    eta = quote?.eta ?? (monitor ? m.run_eta_none() : m.run_elapsed({ duration: durationText(now - run.createdAt) }));
    etaDetail = quote?.detail ?? "";
    cost = quote?.cost ?? "";
  } else if (open && limitEndsAt !== null) {
    eta = m.run_eta_to_limit({ duration: durationText(limitEndsAt - now) });
    etaDetail = monitor?.rules.length ? m.run_eta_ends_by_rules({ clock: clockText(limitEndsAt) }) : m.run_eta_ends_by({ clock: clockText(limitEndsAt) });
  } else if (open) {
    eta = m.run_elapsed({ duration: durationText(elapsed) });
    etaElapsed = true;
    etaDetail = settingUp ? "" : noEstimate;
  } else if (monitor?.estimate) {
    ({ eta, detail: etaDetail, strong: etaStrong, left } = estimateWords(monitor.estimate, now, limit, progress?.stallMs ?? 0));
  } else if (summary?.etaMs) {
    const { lo, mid, hi, at } = summary.etaMs;
    ({ eta, detail: etaDetail, strong: etaStrong, left } = estimateWords({ loMs: lo, midMs: mid, hiMs: hi, at, pending: false, pastLimitMs: null, floor: summary.etaFloor ?? false, untimed: summary.etaUntimed }, now, null, 0));
  } else if (summary?.pending) {
    eta = m.run_eta_estimating();
    etaDetail = m.run_eta_needs_steps();
  } else {
    // A server that sends no monitor at all.
    eta = m.run_elapsed({ duration: durationText(elapsed) });
    etaElapsed = true;
  }
  if (!sentence.text && !lead) {
    sentence = { ...sentence, text: eta };
    eta = "";
  }

  const headline = headlineOf(monitor, summary?.headline ?? null, signals, sentence.key, live);

  const segments = progress?.segments.map(({ weight, fill }) => ({ weight, fill: pre ? 0 : fill }))
    ?? summary?.segments?.map(({ weight, fill }) => ({ weight, fill: pre ? 0 : fill }))
    ?? [{ weight: 1, fill: pre ? 0 : summary?.whole ?? (tone === "done" ? 1 : null) }];
  const whole = progress?.whole ?? summary?.whole ?? null;
  // With no watcher the sentence is the stage and how far, so the caption says only what is new.
  const said = sentence.key === "auto" || sentence.key.startsWith("end:");
  const caption = pre ? m.run_not_started()
    : progress && (notice || degraded) ? m.run_progress_last_seen({ progress: autoDir(progressText ?? "") })
    : progress ? captionProgress(progress, phases, open && limit ? limit.ms : null, elapsed, said)
    : "";

  const ticks = (monitor?.watch.looks ?? []).flatMap((look) => look.whole === null ? [] : [{
    id: look.id,
    at: look.whole,
    flag: look.said === "flag",
    text: `${clockText(look.at)} · ${plainSentence(look.text)}`,
  }]);

  const logError = (monitor ?? summary)?.logError ?? null;
  const note = tone === "revising" ? m.run_flag_revising({ agent: autoDir(agent) })
    : tone === "stopped" && outcome?.by === "rule" && outcome.rule ? m.run_rule_detail({ text: autoDir(outcome.rule.text), name: autoDir(outcome.rule.name) })
    : tone === "stopped" && outcome?.by === "agent" ? m.run_rule_reason_in_chat()
    : tone === "failed" && logError ? m.run_failed_log_error({ line: quoted(logError) })
    : null;

  // A failed refresh is newer than the availability the last read reported.
  const staleState: NonNullable<RunModel["stale"]>["state"] = failing !== null ? "unavailable"
    : monitorFailed || availability === "live" ? "failed"
    : availability;
  const words = { stale: m.run_view_stale, rebuilding: m.run_view_rebuilding, unavailable: m.run_view_unavailable, failed: m.run_view_refresh_failed }[staleState]();
  const why = failing ?? (!monitorFailed ? monitor?.availability.reason : null) ?? null;
  const reason = why ? autoDir(why) : null;
  const stale = degraded ? { state: staleState, age: m.status_last_seen({ time: clockText(shownAsOf) }), why: [words, reason].filter(Boolean).join(" · ") } : null;
  const timeLabel = stale?.age ?? eta;

  return {
    run,
    sessionId: item.sessionId,
    name: item.experimentSlug ?? item.experimentName ?? run.experimentId,
    where: where(run),
    tone,
    live,
    pre,
    lead,
    status,
    sentence,
    headline,
    eta,
    left,
    etaDetail,
    etaStrong: etaStrong && !lastSeen,
    progress: caption,
    cost,
    bar: { segments, whole, frozen: tone === "lost" || degraded, label: barLabel([sentence.text, caption, timeLabel, etaElapsed ? etaDetail : ""]) },
    ticks,
    flag,
    flagDetail: live && flag?.record?.detail ? fillSentence(flag.record.detail, bindings, signals) : null,
    needsChoice,
    looking,
    note,
    notice,
    stale,
    connectHost: live ? runConnectHost(run) : null,
    canStop: runStoppable(run) && tone !== "revising",
    canDismiss: shown && !live && tone !== "revising" && !(notice && UNDISMISSABLE.has(notice.kind)),
  };
}

function headlineOf(monitor: RunMonitor | null, summary: NonNullable<Run["monitor"]>["headline"], signals: Signals, sentenceKey: string, live: boolean): RunModel["headline"] {
  let name: string;
  let label: string;
  let value: SignalValue;
  let vs: Versus | null;
  if (monitor) {
    if (monitor.headline === null) return null;
    name = monitor.headline;
    label = signals[name]?.label ?? name;
    value = monitor.latest[name] ?? null;
    vs = versusOf(monitor, name, live);
  } else if (summary) {
    ({ name, label, value, vs } = summary);
  } else return null;
  const said = sentenceKey.startsWith("look:") ? tokensOf(sentenceKey.slice(5)) : new Set<string>();
  return {
    name,
    label,
    value: ltr(fmtSignal(signals[name], value)),
    number: typeof value === "number" && Number.isFinite(value) ? value : null,
    better: signals[name]?.better,
    // A NaN loss is the number most worth seeing; only a missing one is left out.
    known: value !== null,
    delta: vs ? { down: vs.d < 0, good: vs.good, flat: vs.flat, magnitude: ltr(fmtMagnitude(signals[name], vs.mag)) } : null,
    inSentence: said.has(name),
    vsInSentence: said.has(`${name}.vs`),
  };
}

export type SessionRunMark = "waiting" | "flagged" | "running" | null;

/** What a chat's sidebar row shows of its runs: the warning while a flag is unsettled, else the flask while one runs. */
export function sessionRunMark(runs: ShownRun[], sessionId: string): SessionRunMark {
  const own = runs.filter((item) => item.sessionId === sessionId);
  const flags = own.flatMap((item) => item.run.monitor?.flag ? [item.run.monitor.flag] : []);
  if (flags.some((flag) => flag.mode === "copilot" && flag.state === "waiting")) return "waiting";
  if (flags.length > 0) return "flagged";
  return own.some((item) => runActive(item.run.status)) ? "running" : null;
}

/** What a row of run history says beside its status: what the run finished with, or where it stood, in its own words. */
export function historySentence(run: Run, now: number): string {
  const model = runModel({ item: { sessionId: "", run, experimentName: null }, monitor: null, shown: false, agent: m.chat_the_agent(), now, nextRetryMs: null });
  const { headline } = model;
  if (model.sentence.key === "end:done") return headline?.known ? `${headline.label} ${headline.value}` : "";
  return plainSentence(model.sentence.text);
}
