/** A draft opening with `!` is a shell command, not a message (Claude Code's bash mode). */
export const BASH_PREFIX = "!";

export function bashCommand(draft: string): string | null {
  if (!draft.startsWith(BASH_PREFIX)) return null;
  return draft.slice(BASH_PREFIX.length).trim();
}

/** The draft with the shell prefix dropped, for leaving the mode in place. */
export function withoutBashPrefix(draft: string): string {
  return draft.startsWith(BASH_PREFIX) ? draft.slice(BASH_PREFIX.length) : draft;
}
