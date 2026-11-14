import { fmtDuration } from "../api";
import { autoDir, fmtClock, fmtNumber, ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import { fmtPercent, fmtUsd } from "./format";
import type { Cost, Estimate, Phase, Progress, Quote } from "./types";

/** Durations, times and prices formatted for the locale and isolated, so words in the other direction keep them whole. */
export const durationText = (ms: number) => autoDir(fmtDuration(ms));
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
  const where = said ? null : stageWords(phases[progress.phase], progress.done);
  const share = openLimitMs !== null
    ? m.run_progress_limit({ elapsed: durationText(elapsedMs), limit: durationText(openLimitMs) })
    : progress.whole === null ? null : fmtPercent(progress.whole);
  return [where, share].filter(Boolean).join(" · ");
}

/** What orx quoted before launch: about how long, the range and what it goes by, and what it may cost. */
export function quoteWords(quote: Quote): { eta: string; detail: string; cost: string } {
  const { from } = quote;
  const source = from.kind === "earlier_run" ? m.run_quote_from_earlier({ n: ltr(fmtNumber(from.runNo)), hw: ltr(from.hardware) })
    : from.kind === "parent_same_hw" ? m.run_quote_parent({ hw: ltr(from.hardware) })
    : from.kind === "parent_other_hw" ? m.run_quote_parent_other({ hw: ltr(from.hardware) })
    : from.kind === "agent" ? m.run_quote_agent()
    : m.run_quote_none();
  const { loMs: lo, hiMs: hi } = quote;
  const cost = quote.usd ? m.run_cost_range({ lo: usdText(quote.usd[0]), hi: usdText(quote.usd[1]) })
    : quote.spendCap !== null ? m.run_quote_spend_cap({ cost: usdText(quote.spendCap) })
    : "";
  if (lo === null || hi === null) return { eta: m.run_eta_none(), detail: source, cost };
  return {
    eta: m.run_eta_about({ duration: durationText((lo + hi) / 2) }),
    detail: m.run_quote_range({ lo: durationText(lo), hi: durationText(hi), source }),
    cost,
  };
}

/** Time left and its detail, counted down from when the server estimated it. */
export function estimateWords(estimate: Estimate, now: number, limit: { endsAt: number; ms: number } | null, stallMs: number): { eta: string; detail: string; strong: boolean; left: { ms: number; floor: boolean } | null } {
  const remaining = (ms: number) => Math.max(0, ms - (now - estimate.at));
  if (estimate.pending || estimate.midMs === null) return { eta: m.run_eta_estimating(), detail: m.run_eta_needs_steps(), strong: false, left: null };
  const mid = remaining(estimate.midMs);
  // A floor is true only at the estimate's low end: a bar may finish fast with nothing after it.
  const left = estimate.floor ? { ms: remaining(estimate.loMs ?? estimate.midMs), floor: true } : { ms: mid, floor: false };
  const eta = left.floor ? m.run_eta_at_least_left({ duration: durationText(left.ms) }) : m.run_eta_left({ duration: durationText(mid) });
  if (stallMs > 0) return { eta, detail: m.run_eta_stalled({ duration: durationText(stallMs) }), strong: true, left };
  if (estimate.pastLimitMs !== null && limit) {
    return { eta, detail: m.run_eta_past_limit({ duration: durationText(estimate.pastLimitMs), limit: durationText(limit.ms) }), strong: true, left };
  }
  if (estimate.floor) return { eta, detail: m.run_eta_floor(), strong: true, left };
  const done = m.run_eta_done_about({ clock: clockText(now + mid) });
  const [lo, hi] = [estimate.loMs, estimate.hiMs].map((ms) => ms === null ? null : durationText(remaining(ms)));
  // A range whose ends read alike says nothing the time left does not.
  const range = lo !== null && hi !== null && lo !== hi ? `${m.run_eta_range({ lo, hi })} · ${done}` : done;
  return { eta, detail: range, strong: true, left };
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
    if (live && cost.atLimit !== null) return m.run_cost_at_limit({ cost: amount, limitCost: usdText(cost.atLimit) });
    return m.run_cost_so_far({ cost: amount });
  }
  if (cost.gpuHours === null) return "";
  return m.run_cost_gpu_hours({ hours: ltr(new Intl.NumberFormat(getLocale(), { maximumFractionDigits: 1 }).format(cost.gpuHours)) });
}
