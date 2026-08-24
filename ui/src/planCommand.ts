import type { SkillInfo } from "./api";

export const PLAN_COMMAND: SkillInfo = {
  name: "plan",
  description: "Toggle Plan mode for this chat",
  source: "command",
};

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
  let before = text.slice(0, context.start);
  if (marginSpaces > 1 && /[ \t]$/.test(before)) {
    before = before.replace(/[ \t]+$/, (spaces) => {
      if (spaces.includes("\t")) return spaces;
      return spaces.length >= marginSpaces ? spaces : margin;
    });
  }
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

export function commandsForHarness(
  skills: SkillInfo[],
  planActivation: "permission" | "command" | null | undefined,
): SkillInfo[] {
  const availableSkills = skills.filter(
    (skill) => skill.name.toLowerCase() !== PLAN_COMMAND.name,
  );
  return planActivation
    ? [PLAN_COMMAND, ...availableSkills]
    : availableSkills;
}

export function parsePlanCommand(
  text: string,
  planActivation: "permission" | "command" | null | undefined,
): { prompt: string } | null {
  if (!planActivation) return null;
  const token = /(^|\s)\/plan(?=\s|$)/gi;
  if (!token.test(text)) return null;
  return { prompt: text.replace(token, "").trim() };
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
