import { ReactQueryDevtoolsPanel } from "@tanstack/react-query-devtools";
import { useEffect, useSyncExternalStore } from "react";
import { useResolvedTheme } from "../theme";

// Kept across reloads, which the dev server triggers on many edits.
const OPEN_KEY = "query-devtools-open";
/** Percent of the window the panel takes from the app, which keeps its controls above it. */
const PANEL_SHARE = 40;

const listeners = new Set<() => void>();

export function setQueryDevtoolsOpen(open: boolean) {
  localStorage.setItem(OPEN_KEY, open ? "on" : "off");
  for (const listener of listeners) listener();
}

export function useQueryDevtoolsOpen(): boolean {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => localStorage.getItem(OPEN_KEY) === "on",
  );
}

/** Docks TanStack Query's cache inspector along the bottom of the window. For dev builds. */
export function QueryDevtools() {
  const theme = useResolvedTheme();
  const open = useQueryDevtoolsOpen();

  useEffect(() => {
    const root = document.getElementById("root");
    if (!open || !root) return;
    root.style.height = `${100 - PANEL_SHARE}%`;
    return () => {
      root.style.height = "";
    };
  }, [open]);

  if (!open) return null;
  return (
    <div className="fixed inset-x-0 bottom-0 z-50 border-t border-border" style={{ height: `${PANEL_SHARE}%` }}>
      <ReactQueryDevtoolsPanel theme={theme} style={{ height: "100%" }} onClose={() => setQueryDevtoolsOpen(false)} />
    </div>
  );
}
