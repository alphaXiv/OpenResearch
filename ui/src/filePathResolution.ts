import type { FileViewDef } from "./workspaceTabs";

/** Escape a string for literal use inside a RegExp. */
export function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

// Map a path an agent reported to a right-pane file tab. An artifact path under
// the compatibility <data dir>/files/<slug>/ layout is stripped to a relative
// path and tagged source:"artifacts". Otherwise it's a repo/worktree path stripped to
// repo-relative, keeping the session id when it points into a per-session
// worktree. Relative paths name files in the click context's checkout and
// inherit `contextSessionId`; the regex fallbacks encode the
// managed storage layouts from src/local/git.rs:
// worktrees/<project-id>/<session>/… and the legacy repos/<owner>/<repo>/….
export function parseFilePath(
  rawPath: string,
  repoPath?: string,
  contextSessionId?: string,
  artifactsDir?: string,
  slug?: string,
): FileViewDef | null {
  let path = rawPath;
  let sessionId: string | undefined;
  const clone = repoPath?.replace(/\/+$/, "");
  const artifacts = artifactsDir?.replace(/\/+$/, "");
  if (path.startsWith("artifacts/")) {
    path = path.slice("artifacts/".length);
    return path ? { path, source: "artifacts" } : null;
  }
  // A home-anchored path (`~` or `~/…`) is disk, never a repo file — the backend
  // expands the `~`, so hand it over verbatim.
  if (path === "~" || path.startsWith("~/")) return { path, source: "abs" };
  // `path` relative to `base` (`""` when equal), else null. macOS symlinks
  // `/tmp`→`/private/tmp` and `/var`→`/private/var`, so an agent-inlined path
  // and the stored dir can differ only by that prefix — strip it on both sides.
  const relUnder = (base: string): string | null => {
    const strip = (p: string) => p.replace(/^\/private(?=\/(?:tmp|var)(?:\/|$))/, "");
    const [p, b] = [strip(path), strip(base)];
    if (p === b) return "";
    return p.startsWith(`${b}/`) ? p.slice(b.length).replace(/^\/+/, "") : null;
  };
  // A relative path names a file in the click context's checkout; the absolute
  // branches below are keyed off the (non-canonical) stored dirs.
  const artifactRel = path.startsWith("/") && artifacts ? relUnder(artifacts) : null;
  const cloneRel = path.startsWith("/") && clone ? relUnder(clone) : null;
  if (!path.startsWith("/")) {
    sessionId = contextSessionId;
  } else if (artifactRel !== null) {
    // Artifact — prefix match against the non-canonical dir the backend
    // surfaced, which mirrors what the agent inlines.
    return artifactRel ? { path: artifactRel, source: "artifacts" } : null;
  } else if (cloneRel !== null) {
    path = cloneRel;
  } else {
    // Artifact fallback for a symlink-divergent path (e.g. /tmp vs
    // /private/tmp) where the exact prefix missed: match the …/files/<slug>/<rel>
    // layout, requiring the slug segment when we know it. (Legacy artifacts/ is
    // migrated to files/ in place, so it never appears in a live path.)
    const slugPat = slug ? escapeRegExp(slug) : "[^/]+";
    const fd = path.match(new RegExp(`/files/${slugPat}/(.+)$`));
    const wt = fd ? null : path.match(/\/openresearch\/worktrees\/[^/]+\/([^/]+)\/(.+)$/);
    const hub = fd || wt ? null : path.match(/\/openresearch\/repos\/[^/]+\/[^/]+\/(.+)$/);
    if (fd) {
      return { path: fd[1], source: "artifacts" };
    } else if (wt) {
      sessionId = wt[1];
      path = wt[2];
    } else if (hub) {
      path = hub[1];
    }
  }
  if (!path) return null;
  // An absolute path none of the checkout/artifacts branches recognized (e.g.
  // /Users/me/.ssh/config) reads straight off disk — the repo /file endpoint
  // only takes repo-relative paths and would reject it.
  if (path.startsWith("/")) return { path, source: "abs" };
  return { path, sessionId };
}

/** The git branch a code file tab is showing, for the header pill — a cited
 * experiment's branch (or any ref view) names that branch, and a worktree/clone
 * file falls back to the baseline branch, so a code tab always says which
 * branch its contents came from. Artifacts and absolute-path files have no
 * branch. */
export function fileBranchLabel(tab: FileViewDef, baselineBranch?: string): string | undefined {
  if (tab.source === "artifacts" || tab.source === "abs") return undefined;
  return tab.ref ?? tab.branchLabel ?? baselineBranch;
}
