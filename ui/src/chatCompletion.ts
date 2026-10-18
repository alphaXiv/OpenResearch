import type { ChatSession } from "./api";

/** The first catalog is a baseline, not a backlog of notifications. */
export class ChatCompletionTracker {
  readonly sessions = new Map<string, ChatSession>();
  readonly completionAfter = new Map<string, number>();
  initialized = false;

  snapshot(sessions: ChatSession[]): ChatSession[] {
    const completed = sessions.filter((session) => this.update(session));
    this.initialized = true;
    return completed;
  }

  update(session: ChatSession): boolean {
    const previous = this.sessions.get(session.id);
    this.sessions.set(session.id, { ...session });
    if (!this.initialized || session.busy || !session.activeLeafId) return false;
    if (this.completionAfter.get(session.id) === Infinity && previous?.activeLeafId === session.activeLeafId) return false;
    const completed = !previous || previous.busy || previous.activeLeafId !== session.activeLeafId;
    // While busy, metadata can be renamed after the assistant's completion
    // timestamp. Only idle-to-idle catch-up needs the last observed update time.
    if (completed) this.completionAfter.set(session.id, previous?.busy ? session.createdAt : previous?.updatedAt ?? session.createdAt);
    return completed;
  }

  busy(id: string): void {
    const session = this.sessions.get(id);
    if (session) this.sessions.set(id, { ...session, busy: true });
  }

  branch(id: string, activeLeafId: string | null): void {
    const session = this.sessions.get(id);
    if (session) this.sessions.set(id, { ...session, activeLeafId });
    this.completionAfter.set(id, Infinity);
  }
}
