import { fmtDuration } from "../api";
import { autoDir, fmtClock, fmtNumber, ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import { fmtPercent, fmtUsd } from "./format";
import type { Cost, Estimate, Phase, Progress } from "./types";

/** Durations, times and prices formatted for the locale and isolated, so words in the other direction keep them whole. */
export const durationText = (ms: number) => autoDir(fmtDuration(ms));
export const remainingText = (ms: number) => durationText(Math.max(1000, ms));
export const clockText = (ms: number) => autoDir(fmtClock(ms));
const usdText = (value: number) => autoDir(fmtUsd(value));

/** Where a run stands within its stage: "step 1,140 of 5,000", "step 24", "35 of 90 batches". With no unit the word is "step". */
export function progressWords(phase: Phase | undefined, done: number): string {
  const count = ltr(fmtNumber(done));
  const unit = phase?.unit ?? "";
  if (phase?.of == null) return unit ? m.run_progress_count({ done: count, unit: autoDir(unit) }) : m.run_progress_step({ done: count });
  const of = ltr(fmtNumber(phase.of));
  if (/^rounds?$/.test(unit)) return m.run_progress_rounds({ done: count, of });
  if (!unit || /^steps?$/.test(unit)) return m.run_progress_steps({ done: count, of });
  return m.run_progress_units({ done: count, of, unit: autoDir(unit) });
}

/** The word a stage counts in: "step" with no unit, else its unit, singular. */
const countedIn = (unit: string) => (unit || "step").toLocaleLowerCase("en").replace(/s$/, "");

/** How far, after the stage's name when it has one and it isn't what the stage counts: "train · step 1,140 of 5,000", "round 14 of 24". */
export function stageWords(phase: Phase | undefined, done: number): string {
  const where = progressWords(phase, done);
  const named = phase?.name && countedIn(phase.name) !== countedIn(phase.unit ?? "");
  return named ? `${autoDir(phase.name)} · ${where}` : where;
}

/** `text` with its first letter capitalised, past the direction marks that isolate a name. */
export const capitalized = (text: string) =>
  text.replace(/^([\u2066-\u2069]*)(\p{L})/u, (_, marks: string, letter: string) => marks + letter.toLocaleUpperCase(getLocale()));

/**
 * The caption under a strip: the stage and how far, unless the sentence already `said` it, then the share of the whole
 * bar, or, given the run's time limit when how much of it remains is unknown, how much of the limit it has used.
 */
export function captionProgress(progress: Progress, phases: Phase[], openLimitMs: number | null, elapsedMs: number, said: boolean): string {
  const phase = phases[progress.phase];
  const where = said ? null : progress.done === null ? (phase?.name ? autoDir(phase.name) : null) : stageWords(phase, progress.done);
  const share = openLimitMs !== null
    ? m.run_progress_limit({ elapsed: durationText(elapsedMs), limit: durationText(openLimitMs) })
    : progress.whole === null ? null : fmtPercent(progress.whole);
  return [where, share].filter(Boolean).join(" · ");
}

/** Time left and its detail, counted down from when the server estimated it. */
export function estimateWords(estimate: Estimate, now: number, limit: { endsAt: number; ms: number } | null, stallMs: number): { eta: string; detail: string; strong: boolean; leftMs: number | null } {
  const remaining = (ms: number) => Math.max(0, ms - (now - estimate.at));
  if (estimate.pending || estimate.midMs === null) return { eta: m.run_eta_estimating(), detail: m.run_eta_needs_steps(), strong: false, leftMs: null };
  if (estimate.floor) {
    const stages = estimate.untimed ?? [];
    const left = remaining(estimate.loMs ?? 0);
    const timed = left >= 1000;
    return {
      eta: timed ? m.run_eta_floor({ duration: durationText(left) }) : m.run_eta_no_estimate(),
      detail: stages.length ? m.run_eta_untimed({ stages: autoDir(stages.join(", ")) }) : m.run_eta_log_floor_detail(),
      strong: timed,
      leftMs: timed ? left : null,
    };
  }
  const mid = remaining(estimate.midMs);
  const stalled = stallMs > 0 ? m.run_eta_stalled({ duration: durationText(stallMs) }) : null;
  // Past its estimate with no progress since, a run is late, never "0s left".
  if (estimate.midMs > 0 && mid === 0) {
    return { eta: m.run_eta_silent_for({ duration: durationText(now - estimate.at) }), detail: m.run_eta_was_due({ clock: clockText(estimate.at + estimate.midMs) }), strong: true, leftMs: null };
  }
  const eta = m.run_eta_left({ duration: remainingText(mid) });
  if (stalled) return { eta, detail: stalled, strong: true, leftMs: mid };
  if (estimate.pastLimitMs != null && estimate.pastLimitMs > 0 && limit) {
    return { eta, detail: m.run_eta_past_limit({ duration: durationText(estimate.pastLimitMs), limit: durationText(limit.ms) }), strong: true, leftMs: mid };
  }
  const done = m.run_eta_done_about({ clock: clockText(now + mid) });
  const [lo, hi] = [estimate.loMs, estimate.hiMs].map((ms) => ms === null ? null : durationText(remaining(ms)));
  // A range whose ends read alike says nothing the time left does not.
  const range = lo !== null && hi !== null && lo !== hi ? `${m.run_eta_range({ lo, hi })} · ${done}` : done;
  return { eta, detail: range, strong: true, leftMs: mid };
}

/** What a run has cost and will, under the strip. */
export function costWords(cost: Cost, live: boolean, lastSeen: boolean): string {
  const spent = cost.spend ?? cost.soFar;
  if (spent !== null) {
    const amount = usdText(spent);
    if (lastSeen) return m.run_cost_last_seen({ cost: amount });
    if (cost.spend !== null) {
      const spend = live && cost.projected !== null ? m.run_cost_spend_in_all({ cost: amount, total: usdText(cost.projected) }) : m.run_cost_spend({ cost: amount });
      return cost.budget === null ? spend : `${spend} · ${m.run_cost_budget({ budget: usdText(cost.budget) })}`;
    }
    if (live && cost.projected !== null) return m.run_cost_in_all({ cost: amount, total: usdText(cost.projected) });
    return m.run_cost_so_far({ cost: amount });
  }
  return "";
}
