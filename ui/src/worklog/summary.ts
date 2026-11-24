import { pluralMessage } from "../components/ResearchCards";
import { fmtNumber, ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import type { ToolActivity } from "./activity";

/** What a group of steps leads with, for its icon and, without a summary of its own, its line:
 * changes first, then tests, research, other chats and the web, and what was only read or looked up last. */
export function leadActivity(activities: ToolActivity[]): ToolActivity | undefined {
  const order: Array<(activity: ToolActivity) => boolean> = [
    (activity) => activity.kind === "edit",
    (activity) => activity.icon === "tests",
    (activity) => activity.kind === "run" || activity.kind === "log" || activity.kind === "project",
    (activity) => activity.kind === "agent",
    (activity) => activity.kind === "web",
    (activity) => activity.kind === "read" || activity.kind === "skill",
    (activity) => activity.kind === "search",
  ];
  const visible = activities.filter((activity) => !activity.aside);
  for (const pool of [visible, activities]) {
    for (const matches of order) {
      const lead = pool.find(matches);
      if (lead) return lead;
    }
  }
  return visible[0] ?? activities[0];
}

/** A mixed group's line: the step it leads with, and the others it did. */
export function mixedSummary(activities: ToolActivity[]): { summary: string; summaryMore: string[] } | undefined {
  const lead = leadActivity(activities);
  if (!lead) return undefined;
  const labels = [...new Set(activities.map((activity) => activity.label))];
  return { summary: lead.label, summaryMore: labels.filter((label) => label !== lead.label) };
}

/** One line for a stretch of code work, most telling first: "Edited score.py, ran tests, and read 4 files". */
export function codeSummary(activities: ToolActivity[]): string {
  const edited = new Map<string, string>();
  const read = new Map<string, string>();
  let tests = 0;
  let searches = 0;
  let commands = 0;
  for (const activity of activities) {
    const files = activity.files ?? [];
    if (activity.kind === "edit" && files.length > 0) for (const file of files) edited.set(file.path, file.name);
    else if (activity.kind === "read" && files.length > 0) for (const file of files) read.set(file.path, file.name);
    else if (activity.icon === "tests") tests++;
    else if (activity.kind === "search") searches++;
    else commands++;
  }

  const onlyFile = edited.size === 1 && read.size <= 1 && [...read.keys()].every((path) => edited.has(path)) ? [...edited.values()][0] : null;
  if (onlyFile && read.size === 1 && tests + searches + commands === 0) return m.activity_read_and_edited({ target: ltr(onlyFile) });
  for (const path of edited.keys()) read.delete(path);

  const count = (value: number) => ({ count: fmtNumber(value) });
  const named = (files: Map<string, string>, one: (params: { target: string }) => string, many: [(params: { count: string }) => string, (params: { count: string }) => string]) =>
    files.size === 1 ? one({ target: ltr([...files.values()][0]) }) : pluralMessage(files.size, ...many)(count(files.size));
  const clauses = [
    edited.size > 0 && named(edited, m.activity_clause_edited_target, [m.activity_clause_edited_files_one, m.activity_clause_edited_files_other]),
    tests > 0 && m.activity_clause_ran_tests(),
    read.size > 0 && named(read, m.activity_clause_read_target, [m.activity_clause_read_files_one, m.activity_clause_read_files_other]),
    searches > 0 && pluralMessage(searches, m.activity_clause_searched_one, m.activity_clause_searched_other)(count(searches)),
    commands > 0 && pluralMessage(commands, m.activity_clause_ran_commands_one, m.activity_clause_ran_commands_other)(count(commands)),
  ].filter((clause): clause is string => Boolean(clause)).slice(0, 3);
  const sentence = new Intl.ListFormat(getLocale(), { type: "conjunction" }).format(clauses);
  return sentence.charAt(0).toLocaleUpperCase(getLocale()) + sentence.slice(1);
}
