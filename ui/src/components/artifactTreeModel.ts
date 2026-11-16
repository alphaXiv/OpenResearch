import type { ArtifactEntry } from "../api";

/** How siblings in the artifacts tree order: by modified time (either
 * direction) or by name — the server's default. */
export type ArtifactOrder = "newest" | "oldest" | "name";

const byName = (a: ArtifactEntry, b: ArtifactEntry) =>
  a.name.localeCompare(b.name) || a.path.localeCompare(b.path);

const byModified = (a: ArtifactEntry, b: ArtifactEntry) =>
  a.modifiedAt - b.modifiedAt || byName(a, b);

/** Dirs stay first — the tree's structural order — then the chosen key,
 * applied recursively so every folder sorts the same way. */
export function sortArtifactEntries(entries: ArtifactEntry[], order: ArtifactOrder): ArtifactEntry[] {
  const compare =
    order === "name"
      ? byName
      : order === "newest"
        ? (a: ArtifactEntry, b: ArtifactEntry) => byModified(b, a)
        : byModified;
  return entries
    .map((entry) => (entry.children ? { ...entry, children: sortArtifactEntries(entry.children, order) } : entry))
    .sort((a, b) => Number(b.isDir) - Number(a.isDir) || compare(a, b));
}

/** Keep entries whose name matches the query (case-insensitive substring); a
 * matching folder keeps its whole subtree, and ancestors of anything matching
 * stay so the tree still nests. An empty query keeps everything. */
export function filterArtifactEntries(entries: ArtifactEntry[], query: string): ArtifactEntry[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return entries;
  const matches = (name: string) => name.toLowerCase().includes(needle);
  const keep = (entry: ArtifactEntry): ArtifactEntry | null => {
    if (!entry.isDir) return matches(entry.name) ? entry : null;
    const children = entry.children ?? [];
    if (matches(entry.name)) return entry;
    const kept = children.map(keep).filter((child): child is ArtifactEntry => child !== null);
    if (kept.length === 0) return null;
    return kept.length === children.length ? entry : { ...entry, children: kept };
  };
  return entries.map(keep).filter((entry): entry is ArtifactEntry => entry !== null);
}
