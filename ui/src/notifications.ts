import type { ChatMessage, ChatPart, Run, RunStatus } from "./api";

const STORAGE_KEY = "orx:notifications";

export const notificationsSupported = () => typeof Notification !== "undefined";

/** Opted in on this browser and the browser still grants permission. */
export function notificationsEnabled(): boolean {
  if (!notificationsSupported() || Notification.permission !== "granted") return false;
  try {
    return localStorage.getItem(STORAGE_KEY) === "on";
  } catch {
    return false;
  }
}

export function setNotificationsEnabled(enabled: boolean): void {
  try {
    if (enabled) localStorage.setItem(STORAGE_KEY, "on");
    else localStorage.removeItem(STORAGE_KEY);
  } catch {
    // Non-fatal: notifications simply stay off.
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

/** Unresolved plan / permission / question cards in the message not yet
 * reported. Streaming re-sends the same message, so each card counts once. */
export function newPendingPrompts(message: ChatMessage): ChatPart[] {
  return message.parts.filter((part) => {
    const key = `${message.id}:${part.id}`;
    if (part.type !== "prompt" || !part.prompt || part.prompt.resolved || notifiedPrompts.has(key)) return false;
    notifiedPrompts.add(key);
    return true;
  });
}

/** Show a system notification unless the dashboard already has the user's attention. */
export function notify(tag: string, title: string, body: string, onClick: () => void): void {
  if (!notificationsEnabled() || (document.visibilityState === "visible" && document.hasFocus())) return;
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
