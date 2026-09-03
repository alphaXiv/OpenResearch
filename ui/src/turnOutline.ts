import type { ChatPart } from "./api";
import { isTurnStatusPart, partIsVisible } from "./chatRendering.ts";
import { isTaskListTool } from "./taskProgress.ts";

/** One phase of a running turn: the agent's narration that opened it and the
 * tool calls that followed. */
export interface OutlineStep {
  id: string;
  label: string;
  toolParts: ChatPart[];
  done: boolean;
}

const LABEL_LIMIT = 110;
// A sentence this short ("Clean tree.") says little on its own — take the next one too.
const TERSE_SENTENCE = 30;

/** Leading sentence(s) of a narration paragraph, stripped of markdown markers
 * and clipped for a one-line strip. */
export function stepLabel(text: string): string {
  const flat = text
    .replace(/```[\s\S]*?(```|$)/g, " ")
    .replace(/<(file|run)\b[^>]*\bpath="([^"]*)"[^>]*\/?>/g, "$2")
    .replace(/<(file|run)\b[^>]*\/?>/g, "")
    .replace(/^\s*(?:#+|[-*]|\d+[.)])\s+/, "")
    .replace(/\*\*|__|[`#]+/g, "")
    .replace(/\s+/g, " ")
    .trim();
  // Sentence breaks need a few letters before the stop, so "e.g." and "1." don't end one.
  const sentences = flat.split(/(?<=[a-z]{3}[.!?])\s+(?=\S)/i);
  let label = sentences[0] ?? "";
  if (label.length < TERSE_SENTENCE && sentences[1]) label = `${label} ${sentences[1]}`;
  return label.length > LABEL_LIMIT ? `${label.slice(0, LABEL_LIMIT - 1).trimEnd()}…` : label;
}

/** Phases of a running assistant message, in order: every phase but the last
 * is finished, the last is what the agent is doing now. */
export function turnOutline(parts: ChatPart[]): OutlineStep[] {
  const steps: OutlineStep[] = [];
  for (const part of parts) {
    if (isTurnStatusPart(part) || !partIsVisible(part)) continue;
    if (part.type === "text") {
      const label = stepLabel(part.text ?? "");
      if (label) steps.push({ id: part.id, label, toolParts: [], done: true });
      continue;
    }
    if (part.type !== "tool" || isTaskListTool(part.tool)) continue;
    const current = steps.at(-1);
    if (current) current.toolParts.push(part);
    else steps.push({ id: part.id, label: "", toolParts: [part], done: true });
  }
  const last = steps.at(-1);
  if (last) last.done = false;
  return steps;
}
