import { useRef } from "react";

/** Keeps referential identity across renders while the map's contents are
 * unchanged (same size, same key→value pairs) — so a derived map can sit in a
 * memo/callback dependency list without recomputing on every render that
 * happens to rebuild an equal map. */
export function useStableStringMap(next: Map<string, string>): Map<string, string> {
  const current = useRef(next);
  const unchanged = current.current.size === next.size
    && [...next].every(([key, value]) => current.current.get(key) === value);
  if (!unchanged) current.current = next;
  return current.current;
}
