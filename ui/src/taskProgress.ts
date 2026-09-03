import type { ChatMessage, ChatPart } from "./api";

/** One step of the agent's own task list. `activeText` is the present-tense
 * form Claude Code sends alongside each item ("Running the tests"). */
export interface TaskItem {
  text: string;
  status: TaskStatus;
  activeText?: string;
  /** Claude Code task number, the key its TaskUpdate calls address. */
  id?: string;
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

// Whole-list writes: OpenCode `todowrite`, Claude Code's legacy `TodoWrite`,
// Codex `update_plan`. Incremental: Claude Code's TaskCreate / TaskUpdate /
// TaskList, folded in order (TaskGet is a read, hidden like the others).
const WHOLE_LIST_TOOLS = new Set(["todowrite", "update_plan"]);
const INCREMENTAL_TOOLS = new Set(["taskcreate", "taskupdate", "tasklist", "taskget"]);

export function isTaskListTool(tool: string | undefined): boolean {
  if (!tool) return false;
  const base = toolBaseName(tool);
  return WHOLE_LIST_TOOLS.has(base) || INCREMENTAL_TOOLS.has(base);
}

function taskStatus(raw: unknown): TaskStatus | null {
  const status = typeof raw === "string" ? raw.toLowerCase() : "";
  if (status === "in_progress" || status === "inprogress") return "in_progress";
  if (status === "completed") return "completed";
  if (status === "cancelled") return "cancelled";
  if (status === "pending") return "pending";
  return null;
}

function optionalText(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() !== "" ? value.trim() : undefined;
}

function taskItem(raw: unknown): TaskItem | null {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const record = Object.fromEntries(Object.entries(raw));
  const text = optionalText(record.content) ?? optionalText(record.step);
  if (!text) return null;
  return { text, status: taskStatus(record.status) ?? "pending", activeText: optionalText(record.activeForm) };
}

function toTaskList(items: TaskItem[]): TaskList | null {
  if (items.length === 0) return null;
  return {
    items,
    done: items.filter((item) => item.status === "completed").length,
    total: items.filter((item) => item.status !== "cancelled").length,
    current: items.find((item) => item.status === "in_progress") ?? null,
  };
}

/** The task list a whole-list tool call carries; null for other tools, empty
 * lists, and failed writes (a denied write never became the agent's list). */
export function parseTaskList(part: ChatPart): TaskList | null {
  if (part.type !== "tool" || !part.tool || !WHOLE_LIST_TOOLS.has(toolBaseName(part.tool))) return null;
  if (part.state?.status === "error") return null;
  const input = part.state?.input ?? {};
  const raw = [input.todos, input.plan].find(Array.isArray);
  if (!raw) return null;
  return toTaskList(raw.map(taskItem).filter((item): item is TaskItem => item !== null));
}

// TaskCreate's result: "Task #3 created successfully: …".
const TASK_NUMBER = /#(\d+)/;
// TaskList's lines: "#3 [completed] Inspect the loader (owner) [blocked by #1]".
const TASK_LINE = /^#(\d+)\s+\[([^\]]+)\]\s+(.+?)(?:\s+\([^)]*\))?(?:\s+\[blocked by[^\]]*\])?$/;

function nextTaskId(items: TaskItem[]): string {
  return String(items.reduce((max, item) => Math.max(max, Number(item.id) || 0), 0) + 1);
}

function applyIncremental(items: TaskItem[], base: string, part: ChatPart): TaskItem[] {
  const input = part.state?.input ?? {};
  const output = part.state?.output ?? "";
  switch (base) {
    case "taskcreate": {
      const text = optionalText(input.subject) ?? optionalText(input.description);
      if (!text) return items;
      // The id arrives with the result; until then the harness's own rule
      // (highest id + 1) predicts it.
      const id = TASK_NUMBER.exec(output)?.[1] ?? nextTaskId(items);
      const item: TaskItem = { id, text, status: "pending", activeText: optionalText(input.activeForm) };
      return [...items.filter((existing) => existing.id !== id), item];
    }
    case "taskupdate": {
      const id = typeof input.taskId === "string" || typeof input.taskId === "number" ? String(input.taskId) : null;
      if (!id) return items;
      if (typeof input.status === "string" && input.status.toLowerCase() === "deleted") {
        return items.filter((existing) => existing.id !== id);
      }
      return items.map((existing) =>
        existing.id === id
          ? {
              ...existing,
              status: taskStatus(input.status) ?? existing.status,
              text: optionalText(input.subject) ?? existing.text,
              activeText: optionalText(input.activeForm) ?? existing.activeText,
            }
          : existing,
      );
    }
    case "tasklist": {
      // The harness's own listing is authoritative when it parses.
      if (/^No tasks found/i.test(output.trim())) return [];
      const listed = output.split("\n").flatMap((line) => {
        const match = TASK_LINE.exec(line.trim());
        const status = match ? taskStatus(match[2]) : null;
        if (!match || !status) return [];
        const prior = items.find((existing) => existing.id === match[1]);
        return [{ id: match[1], text: match[3], status, activeText: prior?.activeText }];
      });
      return listed.length > 0 ? listed : items;
    }
    default:
      return items;
  }
}

/** Fold every task-list call in `parts` onto `prior` (the list as it stood
 * before this message). Failed calls are skipped; a message without task
 * calls returns `prior` itself, so memoized consumers see a stable value. */
export function foldTaskList(parts: ChatPart[], prior: TaskList | null): TaskList | null {
  let items = prior?.items ?? [];
  let touched = false;
  for (const part of parts) {
    if (part.type !== "tool" || !part.tool || part.state?.status === "error") continue;
    const base = toolBaseName(part.tool);
    if (WHOLE_LIST_TOOLS.has(base)) {
      const list = parseTaskList(part);
      if (list) items = list.items;
      touched = touched || list !== null;
    } else if (INCREMENTAL_TOOLS.has(base)) {
      items = applyIncremental(items, base, part);
      touched = true;
    }
  }
  return touched ? toTaskList(items) : prior;
}

/** The card anchor for a message: its last successful task-list call, with
 * the list as it stands after the whole message. Earlier calls are superseded. */
export function lastTaskList(parts: ChatPart[], prior: TaskList | null): { id: string; list: TaskList } | null {
  for (let index = parts.length - 1; index >= 0; index--) {
    const part = parts[index];
    if (part.type !== "tool" || !isTaskListTool(part.tool) || part.state?.status === "error") continue;
    const list = foldTaskList(parts, prior);
    return list ? { id: part.id, list } : null;
  }
  return null;
}

export function taskAllDone(list: TaskList): boolean {
  return list.total > 0 && list.done === list.total;
}

/** For each assistant message on the branch that touches the task list, the
 * list as it stood before it — what its incremental calls build on. Other
 * messages are absent (null), keeping their memoized render untouched. */
export function priorTaskLists(messages: ChatMessage[]): Map<string, TaskList | null> {
  const priors = new Map<string, TaskList | null>();
  let list: TaskList | null = null;
  for (const message of messages) {
    if (message.role !== "assistant") continue;
    const next = foldTaskList(message.parts, list);
    if (next !== list) priors.set(message.id, list);
    list = next;
  }
  return priors;
}

/** The running turn's task list: the tail assistant message's list, shown
 * only when that turn touched the list. Earlier turns' lists are history. */
export function activeTurnTaskList(messages: ChatMessage[]): TaskList | null {
  const message = messages.at(-1);
  if (message?.role !== "assistant") return null;
  return lastTaskList(message.parts, priorTaskLists(messages).get(message.id) ?? null)?.list ?? null;
}
