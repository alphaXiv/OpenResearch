import type { Run, RunStatus } from "../api";
import { runNoticeCopy } from "../components/RunContactNotice";
import { statusLabel } from "../components/StatusBadge";
import { autoDir, fmtNumber, ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { runActive, runApiProvider, runDisplay, runNeverStarted, runNotice, runResult, runStoppable, type RunNotice } from "../runDisplay";
import { fillSentence, fmtMagnitude, fmtSignal, plainSentence, tokensOf } from "./format";
import type { Bindings, Flag, FlagState, RunMonitor, RunMonitorSummary, ShownRun, SignalSpec, SignalValue, Signals, Versus, VersusBy } from "./types";
import { tickText } from "./journal";
import { monitorBehind } from "./revision";
import { stagePosition } from "./stages";
import { capitalized, captionProgress, clockText, costWords, durationText, estimateWords, progressWords, remainingText } from "./words";

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
  /** The experiment's title, else its slug. */
  name: string;
  tone: Tone;
  live: boolean;
  /** Submitted and not yet running. */
  pre: boolean;
  /** Ended before it ran its command, so it has no log, stages or duration. */
  neverStarted: boolean;
  settingUp: boolean;
  /** The state a run ended in, or is in, said before its sentence: "Auto-stopped". */
  lead: string | null;
  /** The status word the mark and announcements read. */
  status: string;
  sentence: {
    /** Bold spans wrap the displayed figures; tokens are filled. */
    text: string;
    /** The full, refilled watcher sentence when one is shown. */
    watcher?: string;
    /** Separate slots let the row preserve the count before stage words and watcher prose. */
    stage?: { name: string; count: string };
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
  /** Time left for the whole run, when known. */
  leftMs: number | null;
  etaFloor: boolean;
  etaDetail: string;
  /** `eta` counts only the stage the run is in, never the whole run. */
  etaStage: boolean;
  /** Whether `eta` is a live countdown, drawn heavier in the strip. */
  etaStrong: boolean;
  progress: string;
  cost: string;
  /** A null fill is an empty track: the run has no total and no limit to fill against. */
  bar: { segments: { weight: number; fill: number | null }[]; whole: number | null; frozen: boolean; label: string };
  ticks: { id: string; at: number; flag: boolean; text: string }[];
  /** The unsettled flag as the newest source states it; `record` is its evidence and words, once a monitor has them. */
  flag: { id: string; state: FlagState; handback?: number | null; record: Flag | null } | null;
  /** What the watcher found, in full, while the flag is unsettled. */
  flagDetail: string | null;
  /** A flag waiting on the user opens the strip by itself. */
  needsChoice: boolean;
  /** What only the strip says under the sentence: a rule's detail, or that the agent is revising. */
  note: string | null;
  /** Lost contact, a pending cancel and the like, worded by `runNoticeCopy`. */
  notice: RunNotice | null;
  /** The run's view is older than it looks: when it was last seen, and in its state's own words why. */
  stale: { state: "stale" | "rebuilding" | "unavailable" | "failed"; age: string | null; why: string } | null;
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

/** Text as one line of a sentence: its whitespace collapsed, its `*` kept from drawing bold. */
const flat = (text: string) => text.replace(/\s+/g, " ").replaceAll("*", "∗").trim();

/** A log line as one line of a sentence, shortened. */
function quoted(line: string): string {
  const text = flat(line);
  const chars = [...text];
  return ltr(chars.length > SETUP_LINE ? `${chars.slice(0, SETUP_LINE - 1).join("")}…` : text);
}

/** The bar's accessible text: its parts as one sentence, none keeping a stop of its own. */
function barLabel(parts: string[]): string {
  const text = parts.map((part) => plainSentence(part).trim().replace(/[.。۔]$/u, "")).filter(Boolean).join(", ");
  return text && `${text}.`;
}

/**
 * How a signal compares with the parent: the way a block names, else the signal's own, else at the same step while
 * the run goes and with where the parent ended once it ended.
 */
export const versusOf = (monitor: RunMonitor, name: string, live: boolean, by?: VersusBy): Versus | null =>
  monitor.versus[by ?? monitor.view.signals[name]?.vs ?? (live ? "step" : "final")][name] ?? null;

/** What a sentence's tokens read from a full monitor: each signal, `signal.vs` as the parent comparison, and counts. */
export function bindingsOf(monitor: RunMonitor, live: boolean): Bindings {
  const bindings: Bindings = { ...monitor.latest };
  for (const name of Object.keys(monitor.view.signals)) {
    const vs = versusOf(monitor, name, live);
    if (vs) bindings[`${name}.vs`] = vs;
  }
  for (const [pattern, count] of Object.entries(monitor.counts)) bindings[`count:${JSON.stringify(pattern)}`] = count;
  return bindings;
}

const unsettled = (state: Flag["state"]): state is FlagState => state !== "kept" && state !== "stopped";

/** Whether a run's stages are worth listing: several, each named. Bars a log printed have no names. */
export const namedStages = (monitor: RunMonitor) => monitor.phases.length > 1 && monitor.phases.every((phase) => phase.name);

/** The unsettled flag as the newer of the summary and the monitor states it. */
export function statedFlag(run: Run, fetched: RunMonitor | null): RunMonitorSummary["flag"] {
  if (!fetched || monitorBehind(run.monitor, fetched)) return run.monitor?.flag ?? null;
  const flag = fetched.flags.find((flag) => flag.id === fetched.flagId);
  return flag && unsettled(flag.state) ? { id: flag.id, state: flag.state, handback: flag.handback } : null;
}

/** A run's tone from its newest source: the full monitor when it is that, else what every `Run` carries. */
export function runTone(run: Run, monitor: RunMonitor | null = null): Tone {
  const live = runActive(run.status);
  if (run.status === "lost" || (live && run.contact !== null && run.contact.type !== "stopping")) return "lost";
  if (runNotice(run)?.kind === "monitoring") return "lost";
  if (monitor) {
    if (!live && monitor.revising && run.status === "cancelled") return "revising";
    // A flag no one settled outlasts the run, and colours one that finished.
    const flagged = monitor.flagId !== null && (live || run.status === "done");
    return flagged ? "flag" : STATUS_TONE[run.status];
  }
  // The server reprojects an ended run whose summary still says it is going before it lists it.
  return run.monitor?.tone ?? STATUS_TONE[run.status];
}

export interface RunModelInput {
  item: ShownRun;
  /** The run's whole live state, once a strip has fetched it; it states the run's facts only while it is the newest source. */
  monitor: RunMonitor | null;
  /** The last read of the full monitor failed. */
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
  const source = (monitor ?? summary)?.availability;
  const availability = source?.state ?? "live";
  // The run's projection is failing, which reaches lines that never read its whole monitor too.
  const failing = run.monitor?.unavailable ?? null;
  const shownAsOf = monitor?.asOf ?? run.monitor?.asOf ?? now;
  const degraded = failing !== null || monitorFailed || availability !== "live";
  const live = runActive(run.status);
  const finalizing = !live && failing === null && !monitorFailed && (availability === "stale" || availability === "rebuilding");
  const pre = run.status === "submitting" || run.status === "starting";
  const neverStarted = runNeverStarted(run, fetched);
  const display = runDisplay(run);

  const pageOnly = runNotice(run);
  const notice = pageOnly && !PAGE_ONLY.has(pageOnly.kind) ? pageOnly : null;
  const tone = runTone(run, monitor);
  const lostStatus = tone === "lost" ? statusLabel(display.status, display.stopping) : null;

  const outcome = run.outcome ?? null;
  const lead = tone === "revising" ? m.status_revising()
    : tone === "failed" ? m.status_failed()
    : tone === "stopped" ? (outcome && outcome.by !== "user" ? m.status_auto_stopped() : m.run_lead_stopped())
    : !live && notice?.kind !== "abandoned" ? lostStatus
    : null;

  const signals: Signals = monitor?.view.signals ?? summary?.signals ?? {};
  const bindings = monitor ? bindingsOf(monitor, live) : summary?.bindings ?? {};
  const phases = monitor?.phases ?? [];
  const progress = monitor?.progress ?? null;
  const startedAt = run.startedAt ?? run.createdAt;
  const elapsed = (live ? now : run.endedAt ?? run.updatedAt) - startedAt;
  const limit = monitor?.limit ? { endsAt: monitor.limit.endsAt, ms: monitor.limit.endsAt - startedAt } : null;
  // The summary says where the run stands too, so a line needs no full monitor to say it.
  const stage = summary?.stage ?? null;
  const order = monitor?.stageOrder ?? summary?.stageOrder;
  const stagePhase = progress ? phases[progress.phase] : stage ? { ...stage, priorMs: null, startedAt: null, endedAt: null }
    : order?.index != null ? phases[order.index] : undefined;
  // The server leaves a live run's estimate out exactly when how much of it remains is unknown.
  const estimate = monitor ? monitor.estimate : summary?.estimate ?? null;
  const open = live && !pre && (monitor !== null || summary !== null) && estimate === null;
  const limitEndsAt = monitor ? limit?.endsAt ?? null : summary?.limitEndsAt ?? null;
  const noEstimate = !stagePhase ? m.run_eta_no_progress() : stagePhase.of == null ? m.run_eta_no_total() : m.run_eta_unknown_left();
  const setup = live && !pre && !degraded && !notice && !display.waiting
    ? monitor ? monitor.setup : summary?.setup
    : null;
  const hasStages = monitor ? phases.length > 0 : stage !== null || (order?.count ?? 0) > 0;
  const settingUp = setup != null && hasStages;
  const stageDone = progress?.done ?? stage?.done ?? null;
  const progressText = stageDone === null ? null : progressWords(stagePhase, stageDone);
  const finishing = tone === "running" && !settingUp && !notice && !degraded
    && estimate?.midMs === 0 && !estimate.floor
    && stageDone !== null && stagePhase?.of != null && stageDone >= stagePhase.of
    && (order?.count == null || order.index === order.count - 1);
  const status = lostStatus ?? lead ?? (tone === "flag" ? m.status_flagged() : tone === "done" ? m.status_done() : tone === "starting" ? m.status_starting() : finishing ? m.run_stage_finishing() : m.status_running());

  const stated = statedFlag(run, fetched);
  // A flag's words and evidence never change, so a lagging monitor still supplies them for the same flag.
  const flag = stated && { id: stated.id, state: stated.state, handback: stated.handback, record: fetched?.flags.find((flag) => flag.id === stated.id) ?? null };
  const needsChoice = flag?.state === "waiting";
  const record = flag?.record;
  const flagSummary = flag && run.monitor?.flag?.id === flag.id ? run.monitor.flag : null;
  const flagTemplate = record?.template ?? flagSummary?.template;
  const flagBindings = record?.bindings ?? flagSummary?.bindings;
  const flagText = flagTemplate && flagBindings ? fillSentence(flagTemplate, flagBindings, signals)
    : (record?.source === "look" ? record.text : record?.template) ?? flagSummary?.text ?? null;

  const position = stagePosition(order, settingUp);
  const rawName = stagePhase?.name ?? "";
  const setupName = m.run_stage_setup();
  const name = settingUp ? setupName : finishing ? status : rawName || status;
  const stageLabel = autoDir(position?.index != null && position.count !== null
    ? m.run_stage_position({ stage: autoDir(name), position: ltr(`${fmtNumber(position.index + 1)}/${fmtNumber(position.count)}`) })
    : name);
  const count = progressText === null ? "" : autoDir(capitalized(progressText));
  const logLine = setup?.line;
  const setupStage = settingUp && rawName.toLowerCase() !== setupName.toLowerCase() ? rawName : "";
  const setupDetail = setupStage ? m.run_stage_started({ stage: autoDir(setupStage) })
      : logLine && !/^\s*(?:[{}\[\]]|["']?[\w.-]+["']?\s*[:=])/.test(logLine) ? quoted(logLine) : "";
  const detail = count ? setupStage : setupDetail;
  const stageSlots = { name: detail ? `${stageLabel} · ${autoDir(detail)}` : count ? `${stageLabel} ·` : stageLabel, count };
  const statusText = [stageSlots.name, count].filter(Boolean).join(" ");
  const stageKey = `${settingUp ? "setup" : finishing ? "finishing" : "stage"}:${order?.index ?? rawName}:${order?.first ?? ""}:${order?.count ?? ""}`;
  const logError = (monitor ?? summary)?.logError ?? null;
  const recorded = monitor ? monitor.sentence : summary?.sentence ?? null;
  const watched = live && recorded && now - recorded.t <= 3 * 60_000 ? recorded : null;
  const warning = (live || run.status === "done") && flagText;
  let speaking: string | null = null;
  let sentence: RunModel["sentence"];
  if (notice) {
    const { lead } = runNoticeCopy(notice, nextRetryMs, runApiProvider(run), bold);
    const seen = progressText && live ? ` ${m.run_progress_last_seen({ progress: autoDir(progressText) })}.` : "";
    sentence = { text: lead + seen, key: `notice:${notice.kind}:${lead}`, at: null };
  } else if (display.waiting) {
    const text = `${statusLabel(display.status)} · ${autoDir(display.waiting)}`;
    sentence = { text, key: `pre:waiting:${display.waiting}`, at: null };
  } else if (warning) {
    const text = autoDir(warning);
    sentence = { text, watcher: text, key: `flag:${flag?.id}:${run.status}`, at: record?.at ?? null };
  } else if (tone === "failed") {
    const result = runResult(run)?.trim();
    const why = result ? autoDir(flat(result)) : logError ? quoted(logError) : null;
    const where = [
      run.exitCode != null ? m.run_failed_exit_code({ code: ltr(String(run.exitCode)) }) : "",
      progressText ? m.run_auto_at({ progress: autoDir(progressText) }) : "",
    ].filter(Boolean).join(" ");
    sentence = { text: why ?? (where || m.run_failed_no_reason()), key: `end:${tone}`, at: null };
  } else if (!live) {
    const text = !progressText || tone === "done" ? ""
      : outcome?.by === "user" ? m.run_auto_by_you({ progress: autoDir(progressText) })
      : m.run_auto_at({ progress: autoDir(progressText) });
    sentence = { text, stage: text ? { name: "", count: autoDir(text) } : undefined, key: `end:${tone}`, at: null };
  } else if (pre) {
    sentence = { text: m.status_starting(), key: "pre:starting", at: null };
  } else if (watched) {
    speaking = watched.template;
    const watcher = fillSentence(watched.template, bindings, signals);
    const text = m.run_stage_with_look({ stage: autoDir(statusText.replace(/[.。۔।]+$/u, "")), sentence: autoDir(watcher) });
    sentence = { text, watcher, stage: stageSlots, key: `look:${stageKey}:${watched.template}`, at: watched.t };
  } else {
    sentence = { text: statusText, stage: stageSlots, key: `auto:${stageKey}`, at: null };
  }

  // A floor that every later stage is missing from counts only the stage the run is in.
  const untimed = estimate?.untimed ?? [];
  const later = order?.count != null && order.index != null ? order.count - order.index - 1 : 0;
  const floorStage = estimate?.floor && rawName && later > 0 && untimed.length === later && !untimed.includes(rawName)
    ? { name: rawName, setup: settingUp && order?.index != null && order.index + 1 === order.first }
    : null;
  let eta = "";
  let etaDetail = "";
  let etaStrong = false;
  const etaFloor = Boolean(estimate?.floor && !estimate.pending);
  let leftMs: RunModel["leftMs"] = null;
  // Time since the run started, where it has no time left to show, said as that.
  let etaElapsed = false;
  const frozen = tone === "lost" || degraded;
  // What a run has cost only grows, so it keeps showing while the facts wait for the monitor to catch up.
  const cost = !pre && sameGeneration?.cost ? costWords(sameGeneration.cost, live, frozen && !finalizing) : "";
  const ended = run.status === "done" ? m.run_done_in({ duration: durationText(elapsed) })
    : tone === "failed" ? m.run_failed_after({ duration: durationText(elapsed) })
    : tone === "stopped" || tone === "revising" ? m.run_eta_stopped_at({ duration: durationText(elapsed) })
    : "";
  if (neverStarted) eta = m.run_not_started();
  else if (!live) eta = ended;
  else if (notice || degraded) eta = "";
  else if (pre) {
    etaElapsed = !monitor;
    eta = monitor ? m.run_eta_none() : m.run_elapsed({ duration: durationText(now - run.createdAt) });
  } else if (settingUp && (setup?.startsAt != null || (!etaFloor && !estimate?.pending))) {
    const startsAt = setup?.startsAt;
    eta = startsAt == null ? m.run_eta_none() : startsAt > now ? m.run_setup_starts_in({ duration: remainingText(startsAt - now) }) : m.run_setup_starting();
  } else if (open && limitEndsAt !== null) {
    eta = m.run_eta_to_limit({ duration: durationText(limitEndsAt - now) });
    etaDetail = monitor?.rules.length ? m.run_eta_ends_by_rules({ clock: clockText(limitEndsAt) }) : m.run_eta_ends_by({ clock: clockText(limitEndsAt) });
  } else if (open || finishing) {
    eta = m.run_elapsed({ duration: durationText(elapsed) });
    etaElapsed = true;
    if (!finishing && !setup && (stagePhase?.of != null || stageDone === null)) etaDetail = noEstimate;
  } else if (estimate) {
    ({ eta, detail: etaDetail, strong: etaStrong, leftMs } = estimateWords(estimate, now, limit, progress?.stallMs ?? 0, floorStage));
  } else {
    // A server that sends no monitor at all.
    eta = m.run_elapsed({ duration: durationText(elapsed) });
    etaElapsed = true;
  }
  if (!sentence.text && !lead) {
    sentence = { ...sentence, text: eta };
    eta = "";
  }

  const headline = headlineOf(monitor, summary?.headline ?? null, bindings, signals, speaking, live);

  let segments = progress?.segments.map(({ weight, fill }) => ({ weight, fill: pre ? 0 : fill }))
    ?? summary?.segments?.map(({ weight, fill }) => ({ weight, fill: pre ? 0 : fill }))
    ?? [{ weight: 1, fill: pre ? 0 : summary?.whole ?? (tone === "done" ? 1 : null) }];
  const whole = etaFloor ? null : progress?.whole ?? summary?.whole ?? null;
  if (whole === null && segments.length > 1) {
    const fill = stageDone !== null && stagePhase?.of ? Math.min(1, stageDone / stagePhase.of) : null;
    segments = [{ weight: 1, fill }];
  }
  const said = sentence.key.startsWith("auto:") || sentence.key.startsWith("look:") || sentence.key.startsWith("end:");
  const caption = run.status === "done" ? "" : pre ? m.run_not_started()
    : !finalizing && progress && (notice || degraded) ? m.run_progress_last_seen({ progress: autoDir(progressText ?? "") })
    : progress ? captionProgress({ ...progress, whole }, phases, open && limit ? limit.ms : null, elapsed, said)
    : "";

  // Without a share of the whole run the bar is the stage's own, so only its looks stand on it.
  const onBar = (look: { stage?: string | null }) => whole !== null || (look.stage ?? null) === (stagePhase?.name ?? null);
  const ticks = (monitor?.watch.looks ?? []).flatMap((look) => look.whole === null || !onBar(look) ? [] : [{
    id: look.id,
    at: look.whole,
    flag: look.said === "flag",
    text: tickText(look, signals, clockText(look.at)),
  }]);

  const pace = monitor?.pace ?? null;
  const disagrees = monitor?.disagrees ?? null;
  const note = tone === "revising" ? m.run_flag_revising({ agent: autoDir(agent) })
    : tone === "stopped" && outcome?.by === "rule" && outcome.rule ? m.run_rule_detail({ text: autoDir(outcome.rule.text), name: autoDir(outcome.rule.name) })
    : tone === "stopped" && outcome?.by === "agent" ? (outcome.reason ? m.run_stop_reason({ agent: autoDir(agent), reason: autoDir(outcome.reason) }) : m.run_rule_reason_in_chat())
    : disagrees ? m.run_plan_disagrees({ of: ltr(fmtNumber(disagrees.of)), unit: autoDir(disagrees.unit ?? m.run_plan_steps()), stage: autoDir(disagrees.stage), rows: ltr(fmtNumber(disagrees.rows)) })
    : live && pace ? (pace.than === "earlier" ? m.run_pace_slower_earlier : m.run_pace_slower_parent)({ times: ltr(fmtNumber(pace.times)) })
    : null;

  // A failed refresh is newer than the availability the last read reported.
  const staleState: NonNullable<RunModel["stale"]>["state"] = failing !== null ? "unavailable"
    : monitorFailed || availability === "live" ? "failed"
    : availability;
  const words = {
    stale: m.run_view_stale,
    rebuilding: m.run_view_rebuilding,
    unavailable: m.run_view_unavailable,
    failed: fetched ? m.run_view_refresh_failed : m.run_monitor_load_failed,
  }[staleState]();
  const why = failing ?? (!monitorFailed ? source?.reason : null) ?? null;
  const reason = why ? autoDir(why) : null;
  const stale = degraded && (!finalizing || reason || staleState === "rebuilding") ? {
    state: staleState,
    age: finalizing ? null : m.status_last_seen({ time: clockText(shownAsOf) }),
    why: finalizing && reason ? reason : [words, reason].filter(Boolean).join(" · "),
  } : null;
  const timeLabel = stale?.age ?? stale?.why ?? eta;

  return {
    run,
    sessionId: item.sessionId,
    name: item.experimentName ?? item.experimentSlug ?? run.experimentId,
    tone,
    live,
    pre,
    neverStarted,
    settingUp,
    lead,
    status,
    sentence,
    headline,
    eta,
    leftMs: settingUp ? null : leftMs,
    etaFloor,
    etaStage: floorStage !== null && leftMs !== null,
    etaDetail,
    etaStrong: etaStrong && !frozen,
    progress: caption,
    cost,
    bar: { segments, whole, frozen, label: barLabel([sentence.text, caption, timeLabel, etaElapsed ? etaDetail : ""]) },
    ticks,
    flag,
    flagDetail: record?.detail === flagTemplate ? flagText && autoDir(flagText)
      : record?.detail ? autoDir(fillSentence(record.detail, flagBindings ?? {}, signals)) : null,
    needsChoice,
    note,
    notice,
    stale,
    canStop: runStoppable(run) && tone !== "revising",
    canDismiss: shown && !live && tone !== "revising" && !(notice && UNDISMISSABLE.has(notice.kind)),
  };
}

function headlineOf(monitor: RunMonitor | null, summary: NonNullable<Run["monitor"]>["headline"], bindings: Bindings, signals: Signals, sentenceTemplate: string | null, live: boolean): RunModel["headline"] {
  let name: string;
  let label: string;
  let value: SignalValue;
  let vs: Versus | null;
  if (monitor) {
    const declared = monitor.headline ?? monitor.view.headline[0];
    if (!declared) return null;
    name = declared;
    label = signals[name]?.label ?? name;
    value = monitor.latest[name] ?? null;
    vs = versusOf(monitor, name, live);
  } else if (summary) {
    ({ name, label, value } = summary);
    const comparison = bindings[`${name}.vs`];
    vs = comparison !== null && typeof comparison === "object" ? comparison : null;
  } else return null;
  const said = tokensOf(sentenceTemplate ?? "");
  return {
    name,
    label,
    value: ltr(fmtSignal(signals[name], value)),
    number: typeof value === "number" && Number.isFinite(value) ? value : null,
    better: signals[name]?.better,
    known: value !== null,
    delta: vs ? { down: vs.d < 0, good: vs.good, flat: vs.flat, magnitude: ltr(fmtMagnitude(signals[name], vs.mag)) } : null,
    inSentence: said.has(name),
    vsInSentence: said.has(`${name}.vs`),
  };
}

export type SessionRunMark = "waiting" | "flagged" | "running" | null;

/** What a chat's sidebar row shows of its runs: the warning while a flag is unsettled, else the flask while one runs. */
export function sessionRunMark(runs: readonly ShownRun[], sessionId: string): SessionRunMark {
  const own = runs.filter((item) => item.sessionId === sessionId);
  const flags = own.flatMap((item) => item.run.monitor?.flag ? [item.run.monitor.flag] : []);
  if (flags.some((flag) => flag.state === "waiting")) return "waiting";
  if (flags.length > 0) return "flagged";
  return own.some((item) => runActive(item.run.status)) ? "running" : null;
}

/**
 * The monitored run a chat's inbox card gives a line to: the one with an unsettled flag, else the newest still going,
 * else the newest shown, else `last`, the newest the chat launched, which stays after its result is answered.
 */
export function sessionLeadRun(runs: readonly ShownRun[], sessionId: string, last: Run | null | undefined): ShownRun | null {
  const monitored = runs.filter((item) => item.run.monitor).sort((a, b) => b.run.createdAt - a.run.createdAt);
  return monitored.find((item) => item.run.monitor!.flag) ?? monitored.find((item) => runActive(item.run.status)) ?? monitored[0]
    // A live run is always among the shown ones; one listed live here is a stale read of a run that has since ended.
    ?? (last?.monitor && !runActive(last.status) ? { sessionId, run: last, experimentName: null } : null);
}

/** What an inbox card says of its run: the watcher's words, else the stage's count without the stage, else how it ended. */
export function cardSentence(model: RunModel): string {
  const { sentence, headline } = model;
  if (sentence.watcher) return sentence.watcher;
  if (sentence.key === "end:done") return headline?.known ? `${headline.label} **${headline.value}**` : model.status;
  return sentence.stage?.count || sentence.text || model.status;
}

/** What a row of run history says beside its status: what the run finished with, or where it stood, in its own words. */
export function historySentence(run: Run, now: number): string {
  const model = runModel({ item: { sessionId: "", run, experimentName: null }, monitor: null, shown: false, agent: m.chat_the_agent(), now, nextRetryMs: null });
  const { headline } = model;
  if (model.sentence.key === "end:done") return headline?.known ? `${headline.label} ${headline.value}` : "";
  return plainSentence(model.sentence.text);
}
