import type { ChatMessage, ChatPart, QueuedMessage } from "../api";

export interface ChatState {
  // Every branch of the transcript, not just the one on screen — switching
  // forks is then a pointer move rather than a refetch.
  messagesBySession: Record<string, ChatMessage[]>;
  busySessions: Set<string>;
  // Messages parked behind a running turn, per session, oldest first.
  queuedBySession: Record<string, QueuedMessage[]>;
  // Tip of the branch on screen, per session. Absent falls back to the whole
  // transcript, which is exactly right for a session that was never forked.
  activeLeafBySession: Record<string, string | null>;
}

export type Action =
  | { type: "activeLeaf"; sessionId: string; leafId: string | null }
  // Local-only; swept by upsertMessage's LOCAL_PREFIX filter when the next
  // server message lands, and gone on reload.
  | { type: "localError"; sessionId: string; text: string }
  // A `!` command just sent, shown running until the server's copy lands —
  // or, with `error`, the same card marked as never run.
  | { type: "localShell"; sessionId: string; id: string; command: string; error?: string }
  | { type: "upsertMessage"; sessionId: string; message: ChatMessage }
  | {
    type: "optimisticUser";
    sessionId: string;
    text: string;
    attachments: { url: string; mediaType: string; name?: string }[];
    annotations: { text: string }[];
  }
  | { type: "busy"; sessionId: string; busy: boolean }
  | { type: "setQueued"; sessionId: string; items: QueuedMessage[] }
  | { type: "forget"; sessionId: string };

export const LOCAL_PREFIX = "local-";
/** Server-side `USER_SHELL_TOOL`: a composer `!` command on a user message. */
export const SHELL_TOOL = "bash";

function upsertMessage(list: ChatMessage[], message: ChatMessage): ChatMessage[] {
  const i = list.findIndex((m) => m.id === message.id);
  if (i >= 0) {
    const next = list.slice();
    next[i] = message;
    return next;
  }
  // The server's copy of the user message replaces the optimistic local one.
  if (message.role !== "user") return [...list, message];
  return [...list.filter((m) => !m.id.startsWith(LOCAL_PREFIX)), message];
}

export function reducer(state: ChatState, action: Exclude<Action, { type: "busy" | "forget" }>): ChatState {
  switch (action.type) {
    case "upsertMessage": {
      const list = state.messagesBySession[action.sessionId] ?? [];
      // A re-emitted message (a prompt card resolving, a streaming flush) must
      // not drag the branch pointer backwards — only a message we have not seen
      // extends the branch it arrived on.
      const known = list.some((m) => m.id === action.message.id);
      const leaf = state.activeLeafBySession[action.sessionId] ?? null;
      const replacesOptimistic =
        action.message.role === "user" && leaf !== null && leaf.startsWith(LOCAL_PREFIX);
      // A known message still moves the pointer when it hangs off the leaf. That
      // is forward-only, and it repairs a seed that raced the turn's first flush
      // and would otherwise hide the reply for the rest of the turn.
      const extendsBranch = action.message.parentId != null && action.message.parentId === leaf;
      return {
        ...state,
        messagesBySession: {
          ...state.messagesBySession,
          [action.sessionId]: upsertMessage(list, action.message),
        },
        activeLeafBySession:
          known && !replacesOptimistic && !extendsBranch
            ? state.activeLeafBySession
            : { ...state.activeLeafBySession, [action.sessionId]: action.message.id },
      };
    }
    case "localError": {
      const list = state.messagesBySession[action.sessionId] ?? [];
      const msg: ChatMessage = {
        id: `${LOCAL_PREFIX}senderr-${Date.now()}`,
        role: "assistant",
        parts: [
          { id: "p0", type: "tool", tool: "error", state: { status: "error", error: action.text } },
        ],
        createdAt: Date.now(),
        // Sit on the branch that is showing, not at the root of a new one.
        parentId: state.activeLeafBySession[action.sessionId] ?? null,
      };
      return {
        ...state,
        messagesBySession: { ...state.messagesBySession, [action.sessionId]: [...list, msg] },
        activeLeafBySession: { ...state.activeLeafBySession, [action.sessionId]: msg.id },
      };
    }
    case "localShell": {
      const list = state.messagesBySession[action.sessionId] ?? [];
      // An error re-dispatch replaces the card in place; a new card appends without
      // the local sweep (its parent may be a local card; the server copy sweeps all).
      const known = list.find((m) => m.id === action.id);
      const msg: ChatMessage = {
        id: action.id,
        role: "user",
        parts: [
          {
            id: "p0",
            type: "tool",
            tool: SHELL_TOOL,
            state: {
              status: action.error === undefined ? "running" : "error",
              input: { command: action.command },
              error: action.error,
            },
          },
        ],
        createdAt: known?.createdAt ?? Date.now(),
        parentId: known ? known.parentId : state.activeLeafBySession[action.sessionId] ?? null,
      };
      return {
        ...state,
        messagesBySession: {
          ...state.messagesBySession,
          [action.sessionId]: known ? upsertMessage(list, msg) : [...list, msg],
        },
        activeLeafBySession: known
          ? state.activeLeafBySession
          : { ...state.activeLeafBySession, [action.sessionId]: msg.id },
      };
    }
    case "activeLeaf":
      return {
        ...state,
        activeLeafBySession: {
          ...state.activeLeafBySession,
          [action.sessionId]: action.leafId,
        },
      };
    case "optimisticUser": {
      const list = state.messagesBySession[action.sessionId] ?? [];
      const parts: ChatPart[] = action.text
        ? [{ id: "p0", type: "text", text: action.text }]
        : [];
      // Data URLs stand in until the server's copy arrives with file names.
      action.attachments.forEach((a, i) =>
        parts.push({ id: `img${i}`, type: "image", text: a.url, name: a.name }),
      );
      action.annotations.forEach((annotation, i) =>
        parts.push({
          id: `annotation${i}`,
          type: "annotation",
          text: annotation.text,
        }),
      );
      const msg: ChatMessage = {
        id: `${LOCAL_PREFIX}${Date.now()}`,
        role: "user",
        parts,
        createdAt: Date.now(),
        parentId: state.activeLeafBySession[action.sessionId] ?? null,
      };
      return {
        ...state,
        messagesBySession: { ...state.messagesBySession, [action.sessionId]: [...list, msg] },
        activeLeafBySession: { ...state.activeLeafBySession, [action.sessionId]: msg.id },
      };
    }
    case "setQueued": {
      return {
        ...state,
        queuedBySession: { ...state.queuedBySession, [action.sessionId]: action.items },
      };
    }
  }
}
