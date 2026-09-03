import type { ChatMessage, ChatPart } from "./api";
// Extension spelled out: Node's test runner resolves this without a bundler.
import { isTaskListTool } from "./taskProgress.ts";

/** Whether a part paints anything in the transcript. */
export function partIsVisible(part: ChatPart, activePermissionId?: string | null): boolean {
  // The persisted marker still delimits stopped turns for history consumers.
  if (part.type === "tool" && part.tool?.toLowerCase() === "interrupted") return false;
  if (part.type === "prompt") {
    if (!part.prompt) return false;
    if (part.prompt.kind === "permission") {
      if (part.prompt.resolved) return false;
      // Without a selected prompt, keep unresolved permissions visible as tail boundaries.
      if (activePermissionId !== undefined) return part.id === activePermissionId;
    }
    return true;
  }
  // Hidden reasoning must not displace a visible tool tail during brief thinking bursts.
  if (part.type === "reasoning") return false;
  if (part.type === "text") return Boolean(part.text);
  return true;
}

export function isTurnStatusPart(part: ChatPart): boolean {
  return part.id === "turn-retry" || part.id === "turn-recovery";
}

/** The last visible part, when it is a non-errored tool. A task-list write
 * renders as a checklist, not an activity row, so it never carries the
 * in-progress shimmer — the Thinking status covers that gap instead. */
export function partsTailToolId(parts: ChatPart[]): string | null {
  for (let index = parts.length - 1; index >= 0; index--) {
    const part = parts[index];
    if (part.type === "steer" || isTurnStatusPart(part) || !partIsVisible(part)) continue;
    if (part.type !== "tool" || part.state?.status === "error" || isTaskListTool(part.tool)) return null;
    return part.id;
  }
  return null;
}

export function streamTailTool(messages: ChatMessage[]): { messageId: string; toolId: string } | null {
  const message = messages.at(-1);
  if (message?.role !== "assistant") return null;
  const toolId = partsTailToolId(message.parts);
  return toolId ? { messageId: message.id, toolId } : null;
}

export function streamTailIsText(messages: ChatMessage[]): boolean {
  const message = messages.at(-1);
  if (message?.role !== "assistant") return false;
  for (let index = message.parts.length - 1; index >= 0; index--) {
    const part = message.parts[index];
    if (part.type === "steer" || isTurnStatusPart(part)) continue;
    // Hidden reasoning ends a text tail so Thinking can show while generation pauses.
    return part.type === "text" && Boolean(part.text);
  }
  return false;
}
