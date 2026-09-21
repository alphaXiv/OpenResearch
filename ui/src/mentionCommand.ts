/** Basename of a mention path — the last `/`-segment, or the whole string
 * when there is none (e.g. a bare `@session:id` token — T7's form, not
 * this track's, but harmless to label the same way). */
export function mentionBasename(path: string): string {
  const idx = path.lastIndexOf("/");
  return idx === -1 ? path : path.slice(idx + 1);
}

export interface MentionContext {
  query: string;
  start: number;
  end: number;
}

/** The in-progress `@...` token under the caret, wherever it was typed —
 * mirrors `slashCommandContext`'s whitespace-run scan, but a mention's query
 * may contain `/` (it's a path, not a single-segment name). `@session:` and
 * `@message:` prefixes are T7's own menu, so they don't open this one. */
export function mentionContext(text: string, cursor: number): MentionContext | null {
  if (cursor < 0 || cursor > text.length) return null;
  let start = cursor;
  while (start > 0 && !/\s/.test(text[start - 1])) start -= 1;
  if (text[start] !== "@") return null;
  let end = cursor;
  while (end < text.length && !/\s/.test(text[end])) end += 1;
  const query = text.slice(start + 1, end);
  if (/^(session|message):/i.test(query)) return null;
  return { query, start, end };
}

/** Replace the `@query` token under the caret with the chosen path, leaving
 * the rest of the message where it was typed — mirrors `insertSlashCommand`.
 * A mention's chip label (the basename) is never wider than the path text
 * it replaces, so unlike a skill's display name it never needs extra
 * reserved margin — one space is always enough. */
export function insertMention(
  text: string,
  context: MentionContext,
  path: string,
): { text: string; cursor: number } {
  const margin = " ";
  const before = text.slice(0, context.start);
  let after = text.slice(context.end);
  if (!after) {
    after = margin;
  } else if (!after.startsWith("\n")) {
    const leading = /^[ \t]+/.exec(after)?.[0];
    after = leading ? after : margin + after;
  }
  const gap = /^[ \t]+/.exec(after)?.[0].length ?? 0;
  return {
    text: `${before}@${path}${after}`,
    cursor: before.length + path.length + 1 + gap,
  };
}

interface MentionSegment {
  text: string;
  mention: boolean;
}

/** Trailing punctuation trimmed off a chipped token, matching the server's
 * tokenizer (`src/local/chat/mentions.rs`'s `is_trailing_punct`) so a chip
 * never swallows the period that ends its sentence. */
const TRAILING_MENTION_PUNCT = /[.,;:!?)\]}"']+$/;

/** Split a message into plain runs and whole `@path` tokens, so both the
 * composer and the transcript can chip them in place. Like
 * `splitCommandTokens`, a token must be its own whitespace-delimited run —
 * `(@src/a.py)` isn't chipped, the same gap `splitCommandTokens` leaves for
 * `(/write)`. */
export function splitMentionTokens(text: string): MentionSegment[] {
  const segments: MentionSegment[] = [];
  let plain = "";
  for (const run of text.split(/(\s+)/)) {
    const match = /^@(\S+)$/.exec(run);
    const path = match?.[1]?.replace(TRAILING_MENTION_PUNCT, "");
    if (path) {
      if (plain) segments.push({ text: plain, mention: false });
      plain = "";
      const tokenLength = 1 + path.length;
      segments.push({ text: run.slice(0, tokenLength), mention: true });
      if (tokenLength < run.length) plain += run.slice(tokenLength);
    } else {
      plain += run;
    }
  }
  if (plain) segments.push({ text: plain, mention: false });
  return segments;
}

/** Subsequence fuzzy score: every character of `query` must appear in
 * `target` in order; contiguous runs score higher than scattered ones.
 * `null` when `query` isn't a subsequence of `target` at all. */
function fuzzyScore(query: string, target: string): number | null {
  if (!query) return 0;
  let qi = 0;
  let score = 0;
  let lastMatch = -1;
  for (let ti = 0; ti < target.length && qi < query.length; ti += 1) {
    if (target[ti] === query[qi]) {
      score += lastMatch === ti - 1 ? 2 : 1;
      lastMatch = ti;
      qi += 1;
    }
  }
  return qi === query.length ? score : null;
}

/** Rank `entries` (repo-relative paths) against `query`, a basename match
 * always ahead of a path-only match, each ordered by fuzzy score, ties
 * broken by shorter then alphabetical. An empty query returns the first
 * `limit` entries as given (`getCodeTreeQuery`'s are already sorted). */
export function rankMentionMatches(entries: string[], query: string, limit = 50): string[] {
  const trimmed = query.trim().toLowerCase();
  if (!trimmed) return entries.slice(0, limit);
  const ranked: { path: string; score: number }[] = [];
  for (const path of entries) {
    const baseScore = fuzzyScore(trimmed, mentionBasename(path).toLowerCase());
    const pathScore = baseScore === null ? fuzzyScore(trimmed, path.toLowerCase()) : null;
    if (baseScore === null && pathScore === null) continue;
    ranked.push({ path, score: baseScore !== null ? baseScore + 1000 : (pathScore ?? 0) });
  }
  ranked.sort((a, b) => b.score - a.score || a.path.length - b.path.length || a.path.localeCompare(b.path));
  return ranked.slice(0, limit).map((r) => r.path);
}
