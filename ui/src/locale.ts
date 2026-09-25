import { useSyncExternalStore } from "react";
import {
  getLocale,
  setLocale as setParaglideLocale,
  type Locale,
} from "./paraglide/runtime.js";
import { reportLocale } from "./api";

const listeners = new Set<() => void>();

export function setLocale(next: Locale): void {
  if (next === getLocale()) return;
  void setParaglideLocale(next, { reload: false });
  document.documentElement.lang = next;
  reportLocale(next);
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useLocale(): Locale {
  return useSyncExternalStore(subscribe, getLocale, getLocale);
}
