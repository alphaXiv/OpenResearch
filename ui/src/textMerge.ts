// Fold a file's change on disk into an editor buffer that has moved on since
// the file was loaded — what keeps a collaborator's typing, arriving through
// the Overleaf live channel, from replacing the user's own.

export interface Change {
  start: number;
  /** Length of the replaced span in the base text. */
  removed: number;
  inserted: string;
}

/** One span covering everything that differs between `base` and `next`. */
export function change(base: string, next: string): Change | null {
  if (base === next) return null;
  let start = 0;
  const limit = Math.min(base.length, next.length);
  while (start < limit && base[start] === next[start]) start++;
  let end = 0;
  while (end < limit - start && base[base.length - 1 - end] === next[next.length - 1 - end]) end++;
  return {
    start,
    removed: base.length - start - end,
    inserted: next.slice(start, next.length - end),
  };
}

/**
 * The buffer with the disk change applied, or null when both edited the same
 * span — then the choice is the user's, as with a git pull.
 */
export function mergeText(base: string, mine: string, theirs: string): string | null {
  if (mine === theirs) return mine;
  const ours = change(base, mine);
  const remote = change(base, theirs);
  if (!remote) return mine;
  if (!ours) return theirs;
  if (ours.start + ours.removed <= remote.start) {
    const shift = ours.inserted.length - ours.removed;
    const at = remote.start + shift;
    return mine.slice(0, at) + remote.inserted + mine.slice(at + remote.removed);
  }
  if (remote.start + remote.removed <= ours.start) {
    return mine.slice(0, remote.start) + remote.inserted + mine.slice(remote.start + remote.removed);
  }
  return null;
}
