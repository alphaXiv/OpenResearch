import type { ChatActiveRun, ChatSession } from "./api";
import { isUsageLimitError } from "./chatRendering";

/** What a chat is doing, as the inbox sidebar shows it. `since` is when the state began. */
export type ChatStatus =
  | { kind: "input" }
  | { kind: "queued"; since: number; runs: number }
  | { kind: "running"; since: number; runs: number }
  | { kind: "working"; since: number | null }
  | { kind: "background"; since: number; tasks: number }
  | { kind: "limited" }
  | { kind: "failed"; error: string | null }
  | { kind: "done"; since: number };

/** Whether a status belongs in the inbox's Working section; a chat waiting on the user stays in the inbox. */
export const chatStatusWorking = (status: ChatStatus) =>
  status.kind === "queued" || status.kind === "running" || status.kind === "working" || status.kind === "background";

/** A running experiment outranks the agent's own work: it is what the chat waits on. */
export function chatStatus(session: ChatSession, busy: boolean, waiting: boolean, runs: readonly ChatActiveRun[]): ChatStatus {
  if (waiting) return { kind: "input" };

  const running = runs.filter((item) => item.run.status === "running");
  if (running.length) {
    const since = Math.min(...running.map((item) => item.run.startedAt ?? item.run.createdAt));
    return { kind: "running", since, runs: running.length };
  }
  if (runs.length) return { kind: "queued", since: Math.min(...runs.map((item) => item.run.createdAt)), runs: runs.length };

  const turn = session.lastTurn;
  const turnLive = turn && (turn.state === "preparing" || turn.state === "retrying" || turn.state === "running");
  if (busy) return { kind: "working", since: turnLive ? turn.startedAt : null };
  if (session.background) return { kind: "background", since: session.background.since, tasks: session.background.count };

  if (turn?.state === "failed") {
    return isUsageLimitError(turn.errorKind, turn.error ?? "") ? { kind: "limited" } : { kind: "failed", error: turn.error };
  }
  return { kind: "done", since: session.updatedAt };
}
