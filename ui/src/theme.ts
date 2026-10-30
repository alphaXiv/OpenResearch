import { useSyncExternalStore } from "react";
import { syncDesktopAppearance } from "./desktopTitlebar";

export type ThemePreference = "system" | "light" | "dark";

const STORAGE_KEY = "orx:theme";

function readStoredPreference(): ThemePreference {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === "light" || stored === "dark" || stored === "system") {
      return stored;
    }
  } catch {
    // localStorage may be unavailable (private mode, etc.); fall back to system.
  }
  return "system";
}

let preference: ThemePreference = readStoredPreference();
const listeners = new Set<() => void>();

function resolveTheme(pref: ThemePreference): "light" | "dark" {
  if (pref !== "system") return pref;
  return window.matchMedia("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

function applyResolvedTheme(): void {
  document.documentElement.dataset.theme = resolveTheme(preference);
  syncDesktopAppearance(preference);
}

export function setThemePreference(next: ThemePreference): void {
  preference = next;
  try {
    localStorage.setItem(STORAGE_KEY, next);
  } catch {
    // Non-fatal: the in-memory preference still applies for this session.
  }
  applyResolvedTheme();
  for (const listener of listeners) listener();
}

export function getThemePreference(): ThemePreference {
  return preference;
}

// Keep "system" mode in sync when the OS theme flips while the app is open,
// regardless of which view is mounted.
window
  .matchMedia("(prefers-color-scheme: dark)")
  .addEventListener("change", () => {
    if (preference === "system") applyResolvedTheme();
  });

// The inline script in index.html sets the initial data-theme before paint;
// re-assert it here in case that script was blocked.
applyResolvedTheme();

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function subscribeResolved(listener: () => void): () => void {
  const observer = new MutationObserver(listener);
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  return () => observer.disconnect();
}

const COLORED_MARKDOWN_KEY = "orx:colored-markdown";

function readColoredMarkdown(): boolean {
  try {
    return localStorage.getItem(COLORED_MARKDOWN_KEY) === "on";
  } catch {
    return false;
  }
}

let coloredMarkdown = readColoredMarkdown();
document.documentElement.dataset.coloredMarkdown = coloredMarkdown ? "on" : "off";

/** Whether chat markdown colors bold, italic, and bold-italic; off until turned on. */
export function useColoredMarkdown(): [boolean, (next: boolean) => void] {
  const value = useSyncExternalStore(
    subscribe,
    () => coloredMarkdown,
    () => false,
  );
  const set = (next: boolean) => {
    coloredMarkdown = next;
    try {
      localStorage.setItem(COLORED_MARKDOWN_KEY, next ? "on" : "off");
    } catch {
      // Non-fatal: the in-memory preference still applies for this session.
    }
    document.documentElement.dataset.coloredMarkdown = next ? "on" : "off";
    for (const listener of listeners) listener();
  };
  return [value, set];
}

/** The theme actually showing, including "system" resolved against the OS. */
export function useResolvedTheme(): "light" | "dark" {
  return useSyncExternalStore(
    subscribeResolved,
    () => (document.documentElement.dataset.theme === "dark" ? "dark" : "light"),
    () => "light",
  );
}

/** Current theme preference, reflected on <html data-theme> and persisted. */
export function useThemePreference(): [
  ThemePreference,
  (next: ThemePreference) => void,
] {
  const value = useSyncExternalStore(
    subscribe,
    () => preference,
    () => preference,
  );
  return [value, setThemePreference];
}
