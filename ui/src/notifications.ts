import type { ChatMessage, ChatPart, Run, RunStatus } from "./api";

const STORAGE_KEY = "orx:notifications";

export const NOTIFICATION_KINDS = ["runs", "prompts", "turns"] as const;
export type NotificationKind = typeof NOTIFICATION_KINDS[number];

export const notificationsSupported = () => typeof Notification !== "undefined";

function storedKinds(): NotificationKind[] {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "[]");
    return Array.isArray(stored) ? NOTIFICATION_KINDS.filter((kind) => stored.includes(kind)) : [];
  } catch {
    return [];
  }
}

/** Opted in to this kind on this browser, and the browser still grants permission. */
export function notificationsEnabled(kind: NotificationKind): boolean {
  return notificationsSupported() && Notification.permission === "granted" && storedKinds().includes(kind);
}

export function setNotificationsEnabled(kind: NotificationKind, enabled: boolean): void {
  const kinds = storedKinds().filter((item) => item !== kind);
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(enabled ? [...kinds, kind] : kinds));
  } catch {
    // Non-fatal: the kind simply stays off.
  }
}

const runStatuses = new Map<string, RunStatus>();

/** Records the run's status and reports whether it just went from live to done
 * or failed. The first sighting never counts: the event stream replays every
 * run on connect, and those finished before we were watching. A run the user
 * asked to stop never counts either. */
export function runJustFinished(run: Pick<Run, "id" | "status" | "cancelRequested">): boolean {
  const previous = runStatuses.get(run.id);
  runStatuses.set(run.id, run.status);
  if (run.cancelRequested || (previous !== "starting" && previous !== "running")) return false;
  return run.status === "done" || run.status === "failed";
}

const notifiedPrompts = new Set<string>();
const unresolvedPrompts = new Map<string, Set<string>>();

/** Unresolved plan / permission / question cards in the message not yet
 * reported. Streaming re-sends the same message, so each card counts once. */
export function newPendingPrompts(sessionId: string, message: ChatMessage): ChatPart[] {
  const unresolved = unresolvedPrompts.get(sessionId) ?? new Set<string>();
  unresolvedPrompts.set(sessionId, unresolved);
  return message.parts.filter((part) => {
    if (part.type !== "prompt" || !part.prompt) return false;
    const key = `${message.id}:${part.id}`;
    if (part.prompt.resolved) {
      unresolved.delete(key);
      return false;
    }
    unresolved.add(key);
    if (notifiedPrompts.has(key)) return false;
    notifiedPrompts.add(key);
    return true;
  });
}

const busySessions = new Set<string>();

/** Records the session's busy state and reports whether its turn just ended.
 * A turn that ends on an unanswered card doesn't count: the card already
 * notified as a prompt. */
export function turnJustFinished(sessionId: string, busy: boolean): boolean {
  const wasBusy = busySessions.has(sessionId);
  if (busy) busySessions.add(sessionId);
  else busySessions.delete(sessionId);
  return wasBusy && !busy && !unresolvedPrompts.get(sessionId)?.size;
}

/** Show a system notification unless its kind is off or the dashboard already
 * has the user's attention. */
export function notify(kind: NotificationKind, tag: string, title: string, body: string, onClick: () => void): void {
  if (!notificationsEnabled(kind) || (document.visibilityState === "visible" && document.hasFocus())) return;
  try {
    // Same tag replaces instead of stacking, so several open tabs show one notification.
    const notification = new Notification(title, { tag, body });
    notification.onclick = () => {
      window.focus();
      notification.close();
      onClick();
    };
  } catch {
    // Some browsers only allow notifications from a service worker.
  }
}
