import picomatch from "picomatch/posix.js";
import { splitLineSuffix } from "./markdownTarget.ts";

export type FileResolution =
  | { kind: "exact"; path: string; line?: number }
  /** Fuzzy or glob matches: none is certain, so the reader picks. `total` counts matches beyond the listed `paths`. */
  | { kind: "choices"; paths: string[]; total: number; line?: number }
  | { kind: "none" };

const MAX_CHOICES = 200;

export const GLOB_CHARS = /[*?{]/;

/** Repo-relative, path-shaped text: no spaces or scheme, and a directory or an extension. */
export function looksLikeFilePath(text: string): boolean {
  if (text.length > 200 || !/^[\w.@+\-/*?[\]{},:#]+$/.test(text) || text.includes("://")) return false;
  const { path } = splitLineSuffix(text);
  if (path.startsWith("/") || path.endsWith("/") || path.split("/").includes("..")) return false;
  return path.includes("/") || /\.[A-Za-z0-9]{1,8}$/.test(path);
}

const byDepthThenName = (a: string, b: string) =>
  a.split("/").length - b.split("/").length || (a < b ? -1 : a > b ? 1 : 0);

function choices(matches: string[], line: number | undefined): FileResolution {
  if (matches.length === 0) return { kind: "none" };
  return { kind: "choices", paths: matches.sort(byDepthThenName).slice(0, MAX_CHOICES), total: matches.length, line };
}

const exactIndexes = new WeakMap<readonly string[], Set<string>>();

/** What a path-shaped `text` points at among the repo's file `entries`; only an exact path is certain. */
export function resolveFileReference(entries: readonly string[], text: string): FileResolution {
  const { path: typed, line } = splitLineSuffix(text);
  const path = typed.replace(/^(?:\.\/)+/, "");

  if (GLOB_CHARS.test(path)) {
    // A pattern without a slash matches names at any depth, like `.gitignore`.
    const matcher = picomatch(path, { nocase: true, dot: true, basename: !path.includes("/") });
    return choices(entries.filter((entry) => matcher(entry)), line);
  }

  let exact = exactIndexes.get(entries);
  if (!exact) exactIndexes.set(entries, exact = new Set(entries));
  if (exact.has(path)) return { kind: "exact", path, line };

  // A looser spelling: another case, or the path missing its leading directories.
  const lower = path.toLowerCase();
  return choices(entries.filter((entry) => {
    const entryLower = entry.toLowerCase();
    return entryLower === lower || entryLower.endsWith(`/${lower}`);
  }), line);
}
