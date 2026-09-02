/** Structural on purpose, so this stays dependency-free and directly testable;
 * the `ChatMessage` wire type satisfies it. */
export interface TreeNode {
  id: string;
  /** Absent or null on a branch root. */
  parentId?: string | null;
  role: string;
}

export interface ForkPosition {
  count: number;
  index: number;
  prevId?: string;
  nextId?: string;
}

/** The branch ending at `leafId`, oldest first. Without a usable leaf the list
 * is returned unchanged — right for a transcript that was never forked, and it
 * keeps the array identity memoized consumers depend on. */
export function activePath<T extends TreeNode>(
  messages: T[],
  leafId: string | null | undefined,
): T[] {
  if (!leafId) return messages;
  const byId = new Map(messages.map((m) => [m.id, m]));
  let current = byId.get(leafId);
  // A leaf naming a message we don't have (a delete, a stale pointer) must not
  // blank the transcript.
  if (!current) return messages;
  const path: T[] = [];
  while (current) {
    path.push(current);
    current = current.parentId ? byId.get(current.parentId) : undefined;
  }
  return path.reverse();
}

/** Same role throughout, so a pager never steps onto the other side of a turn. */
function turnForks<T extends TreeNode>(
  message: T,
  byId: Map<string, T>,
  children: Map<string | null, T[]>,
): T[] {
  let anchor: T | undefined = message;
  while (anchor && anchor.role !== "user") {
    anchor = anchor.parentId ? byId.get(anchor.parentId) : undefined;
  }
  const parent = message.role === "user" ? (message.parentId ?? null) : (anchor?.id ?? null);
  const siblings = children.get(parent)?.filter((m) => m.role === message.role);
  return siblings?.length ? siblings : [message];
}

/** Fork positions for each of `bearers`, keyed by message id. `isLocal` ids are
 * excluded — an optimistic bubble beside a real one would read as a second fork
 * of that turn. */
export function forkPositions<T extends TreeNode>(
  messages: T[],
  path: T[],
  bearers: T[],
  isLocal: (id: string) => boolean,
): Map<string, ForkPosition> {
  const known = messages.filter((m) => !isLocal(m.id));
  const byId = new Map(known.map((m) => [m.id, m]));
  const children = new Map<string | null, T[]>();
  for (const message of known) {
    const key = message.parentId ?? null;
    const siblings = children.get(key);
    if (siblings) siblings.push(message);
    else children.set(key, [message]);
  }
  const onPath = new Set(path.map((m) => m.id));
  const positions = new Map<string, ForkPosition>();
  for (const bearer of bearers) {
    const forks = turnForks(bearer, byId, children);
    const index = forks.findIndex((fork) => onPath.has(fork.id));
    positions.set(bearer.id, {
      count: forks.length,
      index,
      prevId: index > 0 ? forks[index - 1].id : undefined,
      nextId: index < forks.length - 1 ? forks[index + 1].id : undefined,
    });
  }
  return positions;
}
