import type { ChatMessage, ChatPart } from "./api";

/** One step of the agent's own task list. `activeText` is the present-tense
 * form Claude Code sends alongside each item ("Running the tests"). */
export interface TaskItem {
  text: string;
  status: TaskStatus;
  activeText?: string;
}

export type TaskStatus = "pending" | "in_progress" | "completed" | "cancelled";

export interface TaskList {
  items: TaskItem[];
  done: number;
  /** Items still in play — cancelled ones are listed but not counted. */
  total: number;
  /** The first in-progress item, when the agent is mid-step. */
  current: TaskItem | null;
}

/** A tool name split on its MCP server / namespace separators, lowercased. */
export function toolSegments(tool: string): string[] {
  return tool.toLowerCase().split(/(?::|\.|__)+/);
}

export function toolBaseName(tool: string): string {
  return toolSegments(tool).at(-1) ?? tool.toLowerCase();
}

// Claude Code `TodoWrite`, OpenCode `todowrite`, Codex `update_plan`.
export function isTaskListTool(tool: string | undefined): boolean {
  if (!tool) return false;
  const base = toolBaseName(tool);
  return base === "todowrite" || base === "update_plan";
}

function taskStatus(raw: unknown): TaskStatus {
  const status = typeof raw === "string" ? raw.toLowerCase() : "";
  if (status === "in_progress" || status === "inprogress") return "in_progress";
  if (status === "completed") return "completed";
  if (status === "cancelled") return "cancelled";
  return "pending";
}

function taskItem(raw: unknown): TaskItem | null {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const record = Object.fromEntries(Object.entries(raw));
  const text = [record.content, record.step].find(
    (value): value is string => typeof value === "string" && value.trim() !== "",
  );
  if (!text) return null;
  const activeText = typeof record.activeForm === "string" && record.activeForm.trim() !== ""
    ? record.activeForm.trim()
    : undefined;
  return { text: text.trim(), status: taskStatus(record.status), activeText };
}

/** The task list a tool call carries; null for other tools, empty lists, and
 * failed writes (a denied `TodoWrite` never became the agent's list). */
export function parseTaskList(part: ChatPart): TaskList | null {
  if (part.type !== "tool" || !isTaskListTool(part.tool) || part.state?.status === "error") return null;
  const input = part.state?.input ?? {};
  const raw = [input.todos, input.plan].find(Array.isArray);
  if (!raw) return null;
  const items = raw.map(taskItem).filter((item): item is TaskItem => item !== null);
  if (items.length === 0) return null;
  return {
    items,
    done: items.filter((item) => item.status === "completed").length,
    total: items.filter((item) => item.status !== "cancelled").length,
    current: items.find((item) => item.status === "in_progress") ?? null,
  };
}

/** The last task-list part among `parts` — the one whose state is current;
 * earlier updates are superseded. */
export function lastTaskList(parts: ChatPart[]): { id: string; list: TaskList } | null {
  for (let index = parts.length - 1; index >= 0; index--) {
    const list = parseTaskList(parts[index]);
    if (list) return { id: parts[index].id, list };
  }
  return null;
}

export function taskAllDone(list: TaskList): boolean {
  return list.total > 0 && list.done === list.total;
}

/** The running turn's task list: the newest one in the tail assistant
 * message. Earlier turns' lists are history, not live progress. */
export function activeTurnTaskList(messages: ChatMessage[]): TaskList | null {
  const message = messages.at(-1);
  return message?.role === "assistant" ? lastTaskList(message.parts)?.list ?? null : null;
}
