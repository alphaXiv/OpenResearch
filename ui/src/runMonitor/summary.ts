import { autoDir, fmtNumber, ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { fillSentence } from "./format";
import type { RunModel, Tone } from "./model";
import type { ChatWatch } from "./types";
import { durationText } from "./words";

/** A title's words: runs of non-space, each with where it sits in the title. */
function wordsOf(title: string) {
  return [...title.matchAll(/\S+/gu)].map((match) => ({ text: match[0], start: match.index, end: match.index + match[0].length }));
}

type Words = ReturnType<typeof wordsOf>;

/** How many words every title starts with, and then ends with, without the two overlapping in the shortest. */
function shared(titles: Words[]): { prefix: number; suffix: number } {
  const shortest = Math.min(...titles.map((words) => words.length));
  const agree = (pick: (words: Words) => string) => titles.every((words) => pick(words) === pick(titles[0]));
  let prefix = 0;
  while (prefix < shortest && agree((words) => words[prefix].text)) prefix += 1;
  let suffix = 0;
  while (prefix + suffix < shortest && agree((words) => words[words.length - 1 - suffix].text)) suffix += 1;
  return { prefix, suffix };
}

/**
 * Words a shared start strips, keeping its last one: that word names what the rest varies ("Depth" in "Depth 4"),
 * where a shared end is only the setting all of them ran in.
 */
const stripped = ({ prefix, suffix }: { prefix: number; suffix: number }) => ({ start: Math.max(0, prefix - 1), end: suffix });

/** Punctuation that only joined the label to the words taken off it. */
const DANGLING = /[\s,;:.、。，；：]+$/u;

/**
 * Each run's short label from its experiment's title: the words it shares with every title it is grouped with go, from
 * the start (but for the last shared word) and from the end. Titles group when two of them share such words, so one
 * unrelated title keeps its own whole and leaves the rest their labels. `groups` numbers each title's group, null for a
 * title said whole. `context` is the shared words as written, with "…" where the labels go, when every title is in one
 * group. Whole words only, split on spaces: a title written without them (Chinese, Japanese) stays whole.
 */
export function shortLabels(titles: string[]): { labels: string[]; groups: (number | null)[]; context: string | null } {
  const distinct = [...new Set(titles.map((title) => title.normalize("NFC")))];
  const words = distinct.map(wordsOf);
  const cut = (group: number[]) => stripped(shared(group.map((index) => words[index])));

  const family = distinct.map((_, index) => index);
  const root = (index: number): number => family[index] === index ? index : root(family[index]);
  for (let a = 0; a < distinct.length; a += 1) {
    for (let b = a + 1; b < distinct.length; b += 1) {
      const { start, end } = cut([a, b]);
      if (start + end > 0) family[root(b)] = root(a);
    }
  }

  const label = new Map<string, { text: string; group: number | null }>();
  let context: string | null = null;
  for (const head of new Set(distinct.map((_, index) => root(index)))) {
    const group = distinct.flatMap((_, index) => root(index) === head ? [index] : []);
    const { start, end } = group.length > 1 ? cut(group) : { start: 0, end: 0 };
    for (const index of group) {
      const own = words[index];
      const text = start + end === 0 ? distinct[index] : distinct[index].slice(own[start].start, own[own.length - 1 - end].end).replace(DANGLING, "");
      label.set(distinct[index], start + end === 0 ? { text: distinct[index], group: null } : { text: text || distinct[index], group: head });
    }
    if (group.length === distinct.length && start + end > 0) {
      const [title, own] = [distinct[group[0]], words[group[0]]];
      const before = title.slice(0, own[start].start).replace(/[\s\p{Ps}\p{Pi}]+$/u, "");
      const after = title.slice(own[own.length - 1 - end].end).replace(/^[\s\p{Pe}\p{Pf},;:.、。，；：]+/u, "");
      context = [before, "…", after].filter(Boolean).join(" ");
    }
  }
  const of = (title: string) => label.get(title.normalize("NFC"));
  return { labels: titles.map((title) => of(title)?.text ?? title), groups: titles.map((title) => of(title)?.group ?? null), context };
}

/** A run in the summary's words: its tone, and the text it is said with, `**figures**` drawn bold in that tone. */
export interface SummaryPart {
  tone: Tone;
  /** Who and in what state, before the run's own sentence. */
  who?: string;
  text: string;
}

/** Whether a run's state needs someone, most urgent first: a choice, a flag, a repair, a failure, lost contact, a stop the user didn't make. */
function urgency(model: RunModel): number {
  if (model.needsChoice) return 0;
  if (model.tone === "flag") return 1;
  if (model.tone === "revising") return 2;
  if (model.tone === "failed") return 3;
  if (model.tone === "lost") return 4;
  if (model.tone === "stopped" && model.run.outcome && model.run.outcome.by !== "user") return 5;
  return Infinity;
}

const PROGRESS: { tone: Tone; words: (count: string) => string }[] = [
  { tone: "starting", words: (count) => m.run_summary_starting({ count }) },
  { tone: "running", words: (count) => m.run_summary_running({ count }) },
  { tone: "done", words: (count) => m.run_summary_done({ count }) },
  { tone: "stopped", words: (count) => m.run_summary_stopped({ count }) },
];

/** The leader among runs that share one headline whose better direction is declared, once two of them have a number. */
function leader(models: RunModel[]): RunModel | null {
  const headed = models.filter((model) => model.headline?.known);
  const first = headed[0]?.headline;
  if (!first?.better || headed.some((model) => model.headline?.name !== first.name || model.headline.better !== first.better)) return null;
  const ranked = headed.filter((model) => model.headline?.number !== null);
  if (ranked.length < 2) return null;
  const score = (model: RunModel) => (model.headline?.number ?? 0) * (first.better === "lower" ? 1 : -1);
  return ranked.reduce((best, model) => score(model) < score(best) ? model : best);
}

/**
 * The watcher's summary sentence, its `{r1}` and `{r1:signal}` filled from the runs its look named, when that look
 * covered exactly the runs live now; else null, and orx words the summary itself. Its bold takes the tone of the run the
 * bold's first token reads.
 */
function watcherSummary(models: RunModel[], labels: string[], watch: ChatWatch | null | undefined): SummaryPart | null {
  const live = models.filter((model) => model.live).map((model) => model.run.id).sort();
  if (!watch?.summary || live.length < 2 || [...watch.runIds].sort().join(" ") !== live.join(" ")) return null;
  const runOf = (key: string) => models.find((model) => model.run.id === watch.keys[key]);
  const { template } = watch.summary;
  const bold = /\*\*[^*]*?\{(r\d+)[:}]/u.exec(template)?.[1];
  const tone = (bold && runOf(bold)?.tone) || "running";
  const text = template.replace(/\{(r\d+)(?::([^{}]+))?\}/gu, (_token, key: string, rest?: string) => {
    const model = runOf(key);
    if (!model) return "—";
    if (rest === undefined) return autoDir(labels[models.indexOf(model)]);
    const summary = model.run.monitor;
    return summary ? fillSentence(`{${rest}}`, summary.bindings, summary.signals) : "—";
  });
  return { tone, text };
}

/**
 * The one line that stands for several runs, worded by orx from their views: who needs attention first; else, when the
 * runs of exactly one label group share a headline that says which way is better, who leads them; else how many are in
 * each state and when the first should finish. Runs only lead where they differ by what their labels say, as a sweep
 * does. The watcher's summary, when there is one, says the last two. `firstDone` is when the soonest live run should
 * finish, for the line to say beside a sentence that doesn't.
 */
export function summarize(models: RunModel[], { labels, groups }: { labels: string[]; groups: (number | null)[] }, watch?: ChatWatch | null): { parts: SummaryPart[]; firstDone: string | null } {
  const name = (model: RunModel) => autoDir(labels[models.indexOf(model)]);
  const urgent = models.reduce<RunModel | null>((most, model) => urgency(model) < (most ? urgency(most) : Infinity) ? model : most, null);
  const leaders = [...new Set(groups)].flatMap((group) => {
    const found = group === null ? null : leader(models.filter((_, index) => groups[index] === group));
    return found ? [{ found, group }] : [];
  });
  const leading = urgent || leaders.length !== 1 ? null : leaders[0];
  // A run that ended with nothing to say, not even why, says how long it took.
  const why = urgent && (urgent.sentence.text || urgent.note);
  const watched = urgent ? null : watcherSummary(models, labels, watch);
  const parts: SummaryPart[] | null = watched ? [watched] : urgent ? [why
    ? { tone: urgent.tone, who: m.run_summary_attention({ name: name(urgent), state: urgent.status }), text: why }
    : { tone: urgent.tone, who: m.run_summary_attention_ended({ name: name(urgent), ended: urgent.eta }), text: "" }]
    : leading?.found.headline ? [{
      tone: leading.found.tone,
      text: (models.some((model) => model.live && groups[models.indexOf(model)] === leading.group) ? m.run_summary_leads : m.run_summary_leads_all_done)({
        name: name(leading.found),
        label: autoDir(leading.found.headline.label),
        value: `**${leading.found.headline.value}**`,
      }),
    }]
    : null;

  const soonest = models.flatMap((model) => model.live && model.left ? [model.left] : []).sort((a, b) => a.ms - b.ms)[0];
  // Once one has ended, the soonest of the rest is the next done, not the first.
  const after = models.some((model) => !model.live);
  const firstDone = (duration: string) => !soonest ? null
    : soonest.floor ? (after ? m.run_summary_next_done_at_least : m.run_summary_first_done_at_least)({ duration })
    : (after ? m.run_summary_next_done : m.run_summary_first_done)({ duration });
  if (parts) return { parts, firstDone: soonest ? firstDone(durationText(soonest.ms)) : null };

  // The common case: the counts, then when the first finishes, all one sentence.
  const counts = PROGRESS.flatMap(({ tone, words }) => {
    const count = models.filter((model) => model.tone === tone).length;
    return count > 0 ? [{ tone, text: words(`**${ltr(fmtNumber(count))}**`) }] : [];
  });
  const first = soonest ? firstDone(`**${durationText(soonest.ms)}**`) : null;
  return { parts: first ? [...counts, { tone: "running", text: first }] : counts, firstDone: null };
}
