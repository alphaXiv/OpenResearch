import { autoDir } from "../i18n";
import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import { fillSentence } from "./format";
import type { Flag, Look, Signals } from "./types";

/** What a look said, with the numbers of its moment, in the app's language; a look that failed says so. */
export function lookText(look: Look, signals: Signals): string {
  if (look.said === "interrupted" || look.said === "error") return m.run_watch_look_failed();
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

/** A look that says something beyond its words: where it left a credential out, or the flag it was noted under. */
const ownRow = (look: Look) => Boolean(look.withheld.length || look.notedUnder);

/**
 * The journal lists changes, not looks, newest first: looks that kept the words before them fold into that row,
 * which says how many looks have said them since when. Flags and failures stand on their own.
 */
export function journalRows(looks: Look[]): JournalRow[] {
  const rows: JournalRow[] = [];
  for (const look of looks) {
    // A look that kept silent changed nothing a reader sees.
    if ((look.said === "new" || look.said === "same") && look.template === null && !look.text && !ownRow(look)) continue;
    const last = rows.at(-1);
    const keeps = last && look.said !== "flag" && last.look.said !== "error" && last.look.said !== "interrupted"
      && !ownRow(look) && !ownRow(last.look) && look.template !== null && look.template === last.look.template;
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
  return { id: `flag:${flag.id}`, at: flag.at, kind: "change", whole: flag.whole, said: "flag", text: flag.detail, template: null, bindings: null, with: [], why: "", stage: null, withheld: [] };
}

/** The row a flag stands under: its look's, or its own. */
export const flagRow = (flag: Flag) => flag.lookId ?? `flag:${flag.id}`;

/** Who settled a flag and how, or who it was sent to, in the app's language; null while no one has chosen. */
export function flagOutcome(flag: Flag, agent: string | null): string | null {
  const { choice } = flag;
  if (!choice) return null;
  const name = autoDir(agent ?? m.chat_the_agent());
  const byYou = choice.by === "user";
  if (flag.state === "kept") return byYou ? m.run_watch_kept() : choice.unsettled ? m.run_watch_kept_unsettled({ agent: name }) : m.run_watch_kept_by({ agent: name });
  if (flag.state === "stopped") {
    if (byYou) return m.run_watch_stopped_you();
    if (choice.by === "agent") return m.run_watch_stopped_by({ agent: name });
    if (choice.by === "rule") return m.run_watch_stopped_by_rule({ name: autoDir(choice.rule ?? "?") });
    return m.run_watch_stopped();
  }
  if (choice.c === "ask") return byYou ? (agent === null ? m.run_watch_asked_unnamed() : m.run_watch_asked({ agent: name })) : m.run_watch_left_to_you({ agent: name });
  return null;
}
