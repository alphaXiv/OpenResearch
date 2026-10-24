import type { ChatMessage, ChatPart, Run, RunStatus } from "./api";

export const NOTIFICATION_KINDS = ["runs", "prompts", "turns"] as const;
export type NotificationKind = typeof NOTIFICATION_KINDS[number];

export const notificationsSupported = () => typeof Notification !== "undefined" && window.isSecureContext;
export const NOTIFICATIONS_CHANGED = "orx:notifications-changed";
const PERMISSION_ASKED_KEY = "orx:notification-permission-asked";
let permissionRequestArmed = false;

export function notificationSettingsSnapshot(): string {
  return notificationsSupported() ? Notification.permission : "unsupported";
}

/** The browser permission is the only switch. Granted means every kind is on. */
export function notificationsEnabled(_kind: NotificationKind): boolean {
  return notificationsSupported() && Notification.permission === "granted";
}

/** Ask once, from the next click or keypress. Loading the page is not a user gesture,
 * and Chrome and Firefox will not show the prompt without one. */
export function requestNotificationPermissionOnce(): void {
  if (permissionRequestArmed || !notificationsSupported() || Notification.permission !== "default") return;
  try {
    if (localStorage.getItem(PERMISSION_ASKED_KEY) === "1") return;
  } catch {
    return;
  }
  permissionRequestArmed = true;
  const ask = (event: Event) => {
    if (!event.isTrusted) return;
    window.removeEventListener("pointerdown", ask, true);
    window.removeEventListener("keydown", ask, true);
    try { localStorage.setItem(PERMISSION_ASKED_KEY, "1"); } catch { /* Ask this once even if storage is blocked. */ }
    void Notification.requestPermission().catch(() => {}).finally(() => {
      window.dispatchEvent(new Event(NOTIFICATIONS_CHANGED));
    });
  };
  window.addEventListener("pointerdown", ask, true);
  window.addEventListener("keydown", ask, true);
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
export function newPendingPrompts(sessionId: string, message: ChatMessage): ChatPart[] {
  return message.parts.filter((part) => {
    if (part.type !== "prompt" || !part.prompt) return false;
    const key = `${sessionId}:${message.id}:${part.id}`;
    if (part.prompt.resolved) {
      return false;
    }
    if (notifiedPrompts.has(key)) return false;
    notifiedPrompts.add(key);
    return true;
  });
}

export function resetNotificationTracking(): void {
  runStatuses.clear();
  notifiedPrompts.clear();
}

const DELIVERED_KEY = "orx:notifications-delivered";
const delivered = new Set<string>();

/** Persist a bounded delivery ledger under one cross-tab lock. A stable tag alone
 * replaces a visible toast but can still play its sound once in every tab. */
export async function notify(kind: NotificationKind, tag: string, title: string, body: string, onClick: () => void, valid = () => true): Promise<boolean> {
  const send = () => {
    if (!valid() || !notificationsEnabled(kind) || delivered.has(tag)) return false;
    let history: string[] = [];
    try {
      const stored: unknown = JSON.parse(localStorage.getItem(DELIVERED_KEY) ?? "[]");
      if (Array.isArray(stored)) history = stored.filter((key): key is string => typeof key === "string");
    } catch { /* A blocked store must not break the event stream. */ }
    if (history.includes(tag)) return false;
    try {
      const notification = new Notification(title, { tag, body, icon: "/favicon.svg" });
      notification.onclick = () => {
        window.focus();
        notification.close();
        onClick();
      };
      delivered.add(tag);
      if (delivered.size > 512) delivered.delete(delivered.values().next().value!);
      try { localStorage.setItem(DELIVERED_KEY, JSON.stringify([...history, tag].slice(-512))); } catch { /* Same-tab deduplication remains available. */ }
      return true;
    } catch {
      // Some browsers only allow notifications from a service worker.
      return false;
    }
  };
  try {
    return navigator.locks ? await navigator.locks.request(DELIVERED_KEY, send) : send();
  } catch {
    return false;
  }
}
