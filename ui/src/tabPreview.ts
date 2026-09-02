import type { KeyboardEvent, MouseEvent } from "react";

export type TabOpenIntent = "preview" | "keepOpen";

export interface TabPreviewState {
  order: readonly string[];
  previewKey: string | null;
}

export interface TabOpenTransition extends TabPreviewState {
  replacedKey: string | null;
}

export interface TabCloseTransition extends TabPreviewState {
  fallbackKey: string | null;
}

/** Open a content tab while preserving the single reusable preview slot. */
export function openTab(
  state: TabPreviewState,
  key: string,
  intent: TabOpenIntent,
): TabOpenTransition {
  if (state.order.includes(key)) {
    return {
      order: state.order,
      previewKey: intent === "keepOpen" && state.previewKey === key ? null : state.previewKey,
      replacedKey: null,
    };
  }

  if (intent === "keepOpen") {
    return {
      order: [...state.order, key],
      previewKey: state.previewKey,
      replacedKey: null,
    };
  }

  if (state.previewKey) {
    const index = state.order.indexOf(state.previewKey);
    if (index !== -1) {
      const order = [...state.order];
      order[index] = key;
      return { order, previewKey: key, replacedKey: state.previewKey };
    }
  }

  return {
    order: [...state.order, key],
    previewKey: key,
    replacedKey: null,
  };
}

export function closeTab(
  state: TabPreviewState,
  key: string,
  history: readonly string[] = [],
): TabCloseTransition {
  const remainingHistory = history.filter((item) => item !== key);
  return {
    order: state.order.filter((item) => item !== key),
    previewKey: state.previewKey === key ? null : state.previewKey,
    fallbackKey: remainingHistory[remainingHistory.length - 1] ?? null,
  };
}

export function openIntentForKey(key: string): TabOpenIntent | null {
  if (key === "Enter") return "keepOpen";
  if (key === " ") return "preview";
  return null;
}

interface TabOpenGestureOptions {
  stopPropagation?: boolean;
}

/** Map click/Space to preview and double-click/middle-click/Enter to keep open. */
export function tabOpenGestureHandlers<T extends HTMLElement>(
  open: (intent: TabOpenIntent) => void,
  options: TabOpenGestureOptions = {},
) {
  const stopPropagation = (event: MouseEvent<T> | KeyboardEvent<T>) => {
    if (options.stopPropagation) event.stopPropagation();
  };
  return {
    onClick: (event: MouseEvent<T>) => {
      stopPropagation(event);
      open("preview");
    },
    onDoubleClick: (event: MouseEvent<T>) => {
      stopPropagation(event);
      open("keepOpen");
    },
    onAuxClick: (event: MouseEvent<T>) => {
      if (event.button !== 1) return;
      event.preventDefault();
      stopPropagation(event);
      open("keepOpen");
    },
    onKeyDown: (event: KeyboardEvent<T>) => {
      const intent = openIntentForKey(event.key);
      if (!intent) return;
      event.preventDefault();
      stopPropagation(event);
      open(intent);
    },
  };
}
