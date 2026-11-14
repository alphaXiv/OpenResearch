import { autoDir } from "../i18n";
import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import { fillSentence } from "./format";
import type { Flag, Look, Signals } from "./types";

/** What a look said, with the numbers of its moment, in the app's language; a look that said nothing, why. */
export function lookText(look: Look, signals: Signals): string {
  if (look.said === "interrupted") return m.run_watch_interrupted();
  if (look.said === "error") return look.text ? m.run_watch_failed({ why: autoDir(look.text) }) : m.run_watch_failed_bare();
  return look.template && look.bindings ? fillSentence(look.template, look.bindings, signals) : look.text;
}

/** "Depth 6 and Depth 8", in the app's language. */
export const listed = (names: string[]) =>
  new Intl.ListFormat(getLocale(), { type: "conjunction" }).format(names.map(autoDir));

/** A tick's words: what was said, and what else the look covered. */
export function tickText(look: Look, signals: Signals, clock: string): string {
  const said = `${clock} · ${lookText(look, signals).replaceAll("**", "")}`;
  return look.with.length ? `${said}\n${m.run_watch_with({ names: listed(look.with) })}` : said;
}

/** One row of the journal: a change, with how long its words then held. */
export interface JournalRow {
  look: Look;
  /** The first look of a run of the same words, when there were several. */
  since: Look | null;
  count: number;
}

/**
 * The journal lists changes, not looks, newest first: looks that kept the words before them fold into that row,
 * which says how many looks have said them since when. Flags and failures stand on their own.
 */
export function journalRows(looks: Look[]): JournalRow[] {
  const rows: JournalRow[] = [];
  for (const look of looks) {
    const last = rows.at(-1);
    const keeps = last && look.said !== "flag" && last.look.said !== "error" && last.look.said !== "interrupted"
      && !look.withheld?.length && !last.look.withheld?.length && look.template !== null && look.template === last.look.template;
    if (keeps) {
      last.since ??= last.look;
      last.count += 1;
      // The row shows the words as they read at its newest look.
      last.look = { ...look, said: last.look.said };
      continue;
    }
    rows.push({ look, since: null, count: 1 });
  }
  return rows.reverse();
}

/** A flag no look raised, a tripwire's or a rule's, as a row of the journal. */
export function flagLook(flag: Flag): Look {
  return { id: `flag:${flag.id}`, at: flag.at, kind: "change", whole: flag.whole, said: "flag", text: flag.detail, template: null, bindings: null, with: [], why: "" };
}

/** The row a flag stands under: its look's, or its own. */
export const flagRow = (flag: Flag) => flag.lookId ?? `flag:${flag.id}`;

/** Who settled a flag and how, or who it was sent to, in the app's language; null while no one has chosen. */
export function flagOutcome(flag: Flag, agent: string): string | null {
  const { choice } = flag;
  if (!choice) return null;
  const name = autoDir(agent);
  const byYou = choice.by === "user";
  if (flag.state === "kept") return byYou ? m.run_watch_kept() : m.run_watch_kept_by({ agent: name });
  if (flag.state === "stopped") return byYou ? m.run_watch_stopped_you() : m.run_watch_stopped_by({ agent: name });
  if (choice.c === "ask") return byYou ? m.run_watch_asked({ agent: name }) : m.run_watch_left_to_you({ agent: name });
  return null;
}
