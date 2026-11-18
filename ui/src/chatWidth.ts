import { useSyncExternalStore } from "react";

/** How wide text columns run: a capped measure, or `full` to follow the pane. */
export type ChatWidth = "balanced" | "wide" | "full";

export const CHAT_WIDTHS: readonly ChatWidth[] = ["balanced", "wide", "full"];

const STORAGE_KEY = "orx:chat-width";

export function parseChatWidth(value: unknown): ChatWidth {
  return CHAT_WIDTHS.find((width) => width === value) ?? "balanced";
}

function readStoredChatWidth(): ChatWidth {
  try {
    return parseChatWidth(localStorage.getItem(STORAGE_KEY));
  } catch {
    // localStorage may be unavailable (private mode, etc.); use the default.
    return "balanced";
  }
}

let chatWidth = readStoredChatWidth();

/** CSS keys off this attribute, so it applies before the first paint of any pane. */
function applyChatWidth(next: ChatWidth): void {
  document.documentElement.dataset.chatWidth = next;
}

applyChatWidth(chatWidth);

const listeners = new Set<() => void>();

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function setChatWidth(next: ChatWidth): void {
  chatWidth = next;
  try {
    localStorage.setItem(STORAGE_KEY, next);
  } catch {
    // Non-fatal: the in-memory preference still applies for this session.
  }
  applyChatWidth(next);
  for (const listener of listeners) listener();
}

/** The chat width preference, reflected on <html data-chat-width> and persisted. */
export function useChatWidth(): [ChatWidth, (next: ChatWidth) => void] {
  const value = useSyncExternalStore(
    subscribe,
    () => chatWidth,
    () => chatWidth,
  );
  return [value, setChatWidth];
}
