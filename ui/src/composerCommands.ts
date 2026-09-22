import type { SkillInfo } from "./api";
import { m } from "./paraglide/messages.js";

export function commandDisplayName(name: string): string {
  return name.charAt(0).toUpperCase() + name.slice(1).replaceAll("-", " ");
}

export function commandLabel(skill: SkillInfo): string {
  return `${skill.plugin ? `${commandDisplayName(skill.plugin)}: ` : ""}${commandDisplayName(skill.name)}`;
}

/** Built-in commands the dashboard runs instead of sending, same on every harness. */
export const COMPOSER_COMMANDS = ["plan", "goal", "new", "resume", "model", "compact", "copy", "export"] as const;

export type ComposerCommandName = (typeof COMPOSER_COMMANDS)[number];

const COMMAND_DESCRIPTIONS: Record<ComposerCommandName, () => string> = {
  plan: () => m.plan_command_description(),
  goal: () => m.goal_command_description(),
  new: () => m.new_command_description(),
  resume: () => m.resume_command_description(),
  model: () => m.model_command_description(),
  compact: () => m.compact_command_description(),
  copy: () => m.copy_command_description(),
  export: () => m.export_command_description(),
};

function composerCommand(name: ComposerCommandName): SkillInfo {
  return {
    name,
    get description() {
      return COMMAND_DESCRIPTIONS[name]();
    },
    source: "command",
  };
}

export function isComposerCommand(name: string): name is ComposerCommandName {
  return COMPOSER_COMMANDS.some((command) => command === name);
}

/** Alternate spellings the agents' own CLIs use, accepted but not listed. */
const COMMAND_ALIASES: Record<string, ComposerCommandName> = { clear: "new", summarize: "compact" };

export function resolveComposerCommand(name: string): ComposerCommandName | null {
  const lower = name.toLowerCase();
  if (isComposerCommand(lower)) return lower;
  return Object.hasOwn(COMMAND_ALIASES, lower) ? COMMAND_ALIASES[lower] : null;
}

function aliasesOf(name: ComposerCommandName): string[] {
  return Object.keys(COMMAND_ALIASES).filter((alias) => COMMAND_ALIASES[alias] === name);
}

/** Menu filtering: a command is also reachable by the aliases it answers to. */
export function commandMatchesQuery(skill: SkillInfo, query: string): boolean {
  if (skill.name.toLowerCase().startsWith(query)) return true;
  if (skill.plugin && `${skill.plugin}:${skill.name}`.toLowerCase().startsWith(query)) return true;
  return skill.source === "command" && isComposerCommand(skill.name)
    && aliasesOf(skill.name).some((alias) => alias.startsWith(query));
}

export interface SlashCommandContext {
  query: string;
  start: number;
  end: number;
}

export function slashCommandContext(
  text: string,
  cursor: number,
): SlashCommandContext | null {
  if (cursor < 0 || cursor > text.length) return null;
  let start = cursor;
  while (start > 0 && !/\s/.test(text[start - 1])) start -= 1;
  if (text[start] !== "/") return null;
  let end = cursor;
  while (end < text.length && !/\s/.test(text[end])) end += 1;
  const query = text.slice(start + 1, end);
  if (query.includes("/")) return null;
  return { query: query.toLowerCase(), start, end };
}

interface CommandSegment {
  text: string;
  command: boolean;
}

/** Split a message into plain runs and whole `/name` tokens naming a known
 * command, so both the composer and the transcript can chip them in place. */
export function splitCommandTokens(
  text: string,
  isCommand: (name: string) => boolean,
): CommandSegment[] {
  const segments: CommandSegment[] = [];
  let plain = "";
  for (const run of text.split(/(\s+)/)) {
    const match = /^\/([^\s/]+)$/.exec(run);
    if (match && isCommand(match[1].toLowerCase())) {
      if (plain) segments.push({ text: plain, command: false });
      plain = "";
      segments.push({ text: run, command: true });
    } else {
      plain += run;
    }
  }
  if (plain) segments.push({ text: plain, command: false });
  return segments;
}

/** Replace the `/query` token under the caret with the chosen command, leaving
 * the rest of the message where it was. */
export function insertSlashCommand(
  text: string,
  context: SlashCommandContext,
  name: string,
  marginSpaces = 1,
): { text: string; cursor: number } {
  const margin = " ".repeat(marginSpaces);
  const before = text.slice(0, context.start);
  let after = text.slice(context.end);
  if (!after) {
    after = margin;
  } else if (!after.startsWith("\n")) {
    const leading = /^[ \t]+/.exec(after)?.[0];
    after = leading
      ? `${leading.length >= marginSpaces ? leading : margin}${after.slice(leading.length)}`
      : margin + after;
  }
  // The caret lands past the reserved inline margin, where surrounding prose continues.
  const gap = /^[ \t]+/.exec(after)?.[0].length ?? 0;
  return {
    text: `${before}/${name}${after}`,
    cursor: before.length + name.length + 1 + gap,
  };
}

export function removeSlashCommand(
  text: string,
  context: SlashCommandContext,
): { text: string; cursor: number } {
  let before = text.slice(0, context.start);
  let after = text.slice(context.end);
  if (!before) {
    after = after.replace(/^\s/, "");
  } else if (!after) {
    before = before.replace(/\s$/, "");
  } else if (/\s$/.test(before) && /^\s/.test(after)) {
    after = after.slice(1);
  }
  return { text: before + after, cursor: before.length };
}

/** Plan is the one command a harness can lack; the rest are the dashboard's own. */
function availableCommands(
  planActivation: "permission" | "command" | null | undefined,
): ComposerCommandName[] {
  return COMPOSER_COMMANDS.filter((name) => name !== "plan" || planActivation);
}

export function commandsForHarness(
  skills: SkillInfo[],
  planActivation: "permission" | "command" | null | undefined,
): SkillInfo[] {
  // Every skill is inserted and resolved by its bare name, a plugin's included,
  // so one sharing a command's name (or alias) would shadow it.
  const availableSkills = skills.filter((skill) => !resolveComposerCommand(skill.name));
  for (const name of availableCommands(planActivation)) availableSkills.push(composerCommand(name));
  return availableSkills.sort((a, b) => {
    const group = Number(b.source === "command") - Number(a.source === "command");
    return group || commandLabel(a).localeCompare(commandLabel(b), undefined, { sensitivity: "base" });
  });
}

/** Commands that read the rest of the message: Plan as the prompt to plan,
 * Goal as the goal to keep. The rest run only as a whole message, so prose
 * mentioning `/export` or asking what `/clear` does still reaches the agent. */
export function takesArgument(name: ComposerCommandName): boolean {
  return name === "plan" || name === "goal";
}

/** The command a typed message runs instead of sending, minus its token. */
export function parseComposerCommand(
  text: string,
  planActivation: "permission" | "command" | null | undefined,
): { name: ComposerCommandName; prompt: string } | null {
  for (const name of availableCommands(planActivation).sort((a, b) => Number(a === "plan") - Number(b === "plan"))) {
    const spellings = [name, ...aliasesOf(name)].join("|");
    if (name === "plan") {
      const token = new RegExp(`(^|\\s)\\/(?:${spellings})(?=\\s|$)`, "gi");
      if (token.test(text)) return { name, prompt: text.replace(token, "").trim() };
    } else if (name === "goal") {
      // Anchored: a goal is what follows the token, so `/goal` must lead.
      const token = new RegExp(`^\\/(?:${spellings})(?=\\s|$)`, "i");
      if (token.test(text.trim())) {
        return { name, prompt: text.trim().replace(token, "").trim() };
      }
    } else if (new RegExp(`^\\/(?:${spellings})$`, "i").test(text.trim())) {
      return { name, prompt: "" };
    }
  }
  return null;
}

export function effectiveCommandPlanMode(
  planActivation: "permission" | "command" | null | undefined,
  toggledMode: boolean | undefined,
  pendingMode: boolean | null,
): boolean | undefined {
  if (planActivation !== "command") return undefined;
  if (toggledMode !== undefined) return toggledMode;
  return pendingMode ?? undefined;
}
