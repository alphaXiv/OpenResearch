import type { ChatSession } from "./api";

export type DelegateStatus = "running" | "asking" | "approval" | "finished" | "failed" | "stopped";

/**
 * A delegate is running while it or any delegate of its own is: a turn that
 * ended waiting on them is not its answer. One that has not run a turn yet is
 * about to, since its task is queued. What it waits on comes first.
 */
export function delegateStatus(session: ChatSession, sessions: ChatSession[]): DelegateStatus {
  if (session.delegate?.asking === "question") return "asking";
  if (session.delegate?.asking) return "approval";
  if (session.busy || delegatesOf(sessions, session.id).some((child) => delegateStatus(child, sessions) === "running")) {
    return "running";
  }
  switch (session.delegate?.state) {
    case "completed":
      return "finished";
    case "failed":
      return "failed";
    case "interrupted":
      return "stopped";
    default:
      return "running";
  }
}

/** The chats `parentId` delegated to, oldest first. */
export function delegatesOf(sessions: ChatSession[], parentId: string): ChatSession[] {
  return sessions
    .filter((session) => session.delegate?.parentId === parentId)
    .sort((a, b) => a.createdAt - b.createdAt || a.id.localeCompare(b.id));
}

/** Every delegate below `rootId`, each followed by its own, with its level under the root. */
export function delegateLineage(sessions: ChatSession[], rootId: string): { session: ChatSession; level: number }[] {
  const rows: { session: ChatSession; level: number }[] = [];
  const visit = (parentId: string, level: number) => {
    for (const session of delegatesOf(sessions, parentId)) {
      rows.push({ session, level });
      visit(session.id, level + 1);
    }
  };
  visit(rootId, 1);
  return rows;
}

/** The model as a person would name it: no provider prefix or free-tier suffix. */
export function delegateModelLabel(session: ChatSession): string {
  const model = session.model?.split("/").pop() ?? "";
  return model.replace(/-free$/, "");
}
