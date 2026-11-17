import type { ArtifactEntry } from "../api";

/** How siblings in the artifacts tree order: by modified time (either
 * direction) or by name — the server's default. */
export type ArtifactOrder = "newest" | "oldest" | "name";

/** Dirs stay first — the tree's structural order — then the chosen key,
 * applied recursively so every folder sorts the same way. Ties keep the
 * server's order, and `name` is the server's order as is. */
export function sortArtifactEntries(entries: ArtifactEntry[], order: ArtifactOrder): ArtifactEntry[] {
  if (order === "name") return entries;
  const direction = order === "newest" ? -1 : 1;
  return entries
    .map((entry) => (entry.children ? { ...entry, children: sortArtifactEntries(entry.children, order) } : entry))
    .sort((a, b) => Number(b.isDir) - Number(a.isDir) || direction * (a.modifiedAt - b.modifiedAt));
}

/** Keep entries whose name matches the query (case-insensitive substring); a
 * matching folder keeps its whole subtree, and ancestors of anything matching
 * stay so the tree still nests. An empty query keeps everything. */
export function filterArtifactEntries(entries: ArtifactEntry[], query: string): ArtifactEntry[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return entries;
  const matches = (name: string) => name.toLowerCase().includes(needle);
  const keep = (entry: ArtifactEntry): ArtifactEntry | null => {
    if (matches(entry.name)) return entry;
    if (!entry.isDir) return null;
    const kept = (entry.children ?? []).map(keep).filter((child): child is ArtifactEntry => child !== null);
    return kept.length === 0 ? null : { ...entry, children: kept };
  };
  return entries.map(keep).filter((entry): entry is ArtifactEntry => entry !== null);
}
