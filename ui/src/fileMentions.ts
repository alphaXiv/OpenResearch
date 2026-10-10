export interface FileMentionContext {
  query: string;
  start: number;
  end: number;
}

export interface FileMentionMatch {
  path: string;
  directory: boolean;
}

/** A match with its sort keys computed once per listing, not per keystroke. */
export interface FileMentionCandidate extends FileMentionMatch {
  lower: string;
  lowerName: string;
  depth: number;
}

const MAX_MATCHES = 50;

/** The `@query` token under the caret, when `@` opens the token — so an email
 * address like `me@example.com` never reads as a mention. */
export function fileMentionContext(text: string, cursor: number): FileMentionContext | null {
  if (cursor < 0 || cursor > text.length) return null;
  let start = cursor;
  while (start > 0 && !/\s/.test(text[start - 1])) start -= 1;
  if (text[start] !== "@") return null;
  let end = cursor;
  while (end < text.length && !/\s/.test(text[end])) end += 1;
  return { query: text.slice(start + 1, end), start, end };
}

export function splitMentionPath(path: string): { name: string; parent: string } {
  const trimmed = path.endsWith("/") ? path.slice(0, -1) : path;
  const slash = trimmed.lastIndexOf("/");
  return {
    name: path.slice(slash + 1),
    parent: slash === -1 ? "" : trimmed.slice(0, slash),
  };
}

function candidate(path: string, directory: boolean): FileMentionCandidate {
  const lower = path.toLowerCase();
  const segments = (directory ? lower.slice(0, -1) : lower).split("/");
  return { path, directory, lower, lowerName: segments[segments.length - 1], depth: segments.length };
}

/** Repo-relative files plus every directory above them (trailing `/`). */
export function mentionCandidates(entries: string[]): FileMentionCandidate[] {
  const directories = new Set<string>();
  for (const entry of entries) {
    for (let i = entry.indexOf("/"); i !== -1; i = entry.indexOf("/", i + 1)) {
      directories.add(entry.slice(0, i + 1));
    }
  }
  return [
    ...[...directories].map((path) => candidate(path, true)),
    ...entries.map((path) => candidate(path, false)),
  ];
}

/** Rank by where the query lands: name prefix, then within the name, then
 * anywhere in the path; shallower and shorter paths first within a rank. */
export function matchFileMentions(candidates: FileMentionCandidate[], query: string): FileMentionMatch[] {
  const needle = query.toLowerCase();
  // An empty query or a typed directory lists that level, not the whole tree.
  const listing = needle === "" || needle.endsWith("/");
  const ranked: { match: FileMentionCandidate; rank: number }[] = [];
  for (const match of candidates) {
    const { lower, lowerName: name } = match;
    if (listing) {
      if (!lower.startsWith(needle) || lower === needle) continue;
      const rest = lower.slice(needle.length);
      const slash = rest.indexOf("/");
      if (slash !== -1 && slash !== rest.length - 1) continue;
      ranked.push({ match, rank: match.directory ? 0 : 1 });
      continue;
    }
    const rank = name.startsWith(needle) ? 0 : name.includes(needle) ? 1 : lower.includes(needle) ? 2 : -1;
    if (rank !== -1) ranked.push({ match, rank });
  }
  ranked.sort(
    (a, b) =>
      a.rank - b.rank
      || (listing ? 0 : a.match.depth - b.match.depth || a.match.path.length - b.match.path.length)
      || (a.match.lower < b.match.lower ? -1 : a.match.lower > b.match.lower ? 1 : 0)
      || (a.match.path < b.match.path ? -1 : a.match.path > b.match.path ? 1 : 0),
  );
  return ranked.slice(0, MAX_MATCHES).map(({ match }) => ({ path: match.path, directory: match.directory }));
}

/** Replace the `@query` token with the picked path. A file gets a trailing
 * space; an unquoted directory keeps the caret inside the token so the menu continues. */
export function insertFileMention(
  text: string,
  context: FileMentionContext,
  match: FileMentionMatch,
): { text: string; cursor: number } {
  // Quoted the way Claude Code writes them, so the agent reads one path.
  const quoted = /\s/.test(match.path);
  const ends = quoted || !match.directory;
  const before = `${text.slice(0, context.start)}@${quoted ? `"${match.path}"` : match.path}`;
  let after = text.slice(context.end);
  if (ends && !/^\s/.test(after)) after = ` ${after}`;
  return {
    text: before + after,
    cursor: before.length + (ends && after.startsWith(" ") ? 1 : 0),
  };
}
