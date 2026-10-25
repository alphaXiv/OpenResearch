import type { FileLocationRequest } from "./api";
import type { LoadedFile } from "./queries/files";

/** Bind OS actions to the source/path that actually answered the preview.
 * In particular artifacts can fall back to artifacts/x in the worktree, and
 * a pruned session can read from the clone. Never recompute those fallbacks. */
export function loadedFileLocation(loaded: LoadedFile, sessionId?: string, ref?: string): FileLocationRequest {
  if (loaded.source === "checkout") return {
    path: loaded.file.path,
    source: "repo",
    sessionId: loaded.file.root === "worktree" ? sessionId : undefined,
    ref: loaded.file.root === "branch" ? ref : undefined,
  };
  return { path: loaded.file.path, source: loaded.source === "artifact" ? "artifacts" : "abs" };
}

/** Merge search hits into a capped tree without discarding its original entries.
 * Ancestor placeholders allow a hit outside the initial cap to be located. */
export function withArtifactEntries<T extends { path: string; name: string; isDir: boolean; size: number; modifiedAt: number; children?: T[] }>(entries: T[], additions: T[]): T[] {
  const result = entries.map(e => ({ ...e, children: e.children ? withArtifactEntries(e.children, []) : undefined })) as T[];
  for (const entry of additions) {
    const segments = entry.path.split("/");
    let branch = result;
    for (let i = 0; i < segments.length; i++) {
      const path = segments.slice(0, i + 1).join("/");
      const existing = branch.find(e => e.path === path);
      if (i === segments.length - 1) {
        if (!existing) branch.push({ ...entry } as T);
      } else {
        const parent = existing ?? { path, name: segments[i], isDir: true, size: 0, modifiedAt: 0, children: [] } as unknown as T;
        if (!existing) branch.push(parent);
        parent.children ??= [];
        branch = parent.children;
      }
    }
  }
  const sort = (rows: T[]) => {
    rows.sort((a,b) => Number(b.isDir) - Number(a.isDir) || a.name.localeCompare(b.name));
    rows.forEach(e => { if (e.children) sort(e.children); });
  };
  sort(result);
  return result;
}
