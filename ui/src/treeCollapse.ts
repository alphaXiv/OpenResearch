/** Branch activity the auto-tidy rule reads, keyed by experiment id. */
export interface BranchActivity {
  live: boolean;
  failed: boolean;
  /** Latest experiment or run timestamp (ms). */
  lastActivity: number;
}

export interface CollapsibleNode {
  id: string;
  children: CollapsibleNode[];
}

/** A finished side branch with no activity for this long folds away. */
export const STALE_BRANCH_MS = 24 * 60 * 60 * 1000;

/** Ids whose subtrees auto-tidy folds: side branches that are not running and
 * either ended in a failure or went quiet. Roots and the path to the most
 * recent activity always stay open. Only the topmost foldable node of a branch
 * is returned; callers reveal the path to selected or matching nodes. */
export function autoCollapsedIds(
  roots: CollapsibleNode[],
  activity: (id: string) => BranchActivity | undefined,
  now: number,
): Set<string> {
  const summary = new Map<CollapsibleNode, { live: boolean; last: number }>();
  const summarize = (node: CollapsibleNode) => {
    const own = activity(node.id);
    const result = { live: own?.live ?? false, last: own?.lastActivity ?? 0 };
    for (const child of node.children) {
      const sub = summarize(child);
      result.live ||= sub.live;
      result.last = Math.max(result.last, sub.last);
    }
    summary.set(node, result);
    return result;
  };
  const newest = roots.reduce((max, root) => Math.max(max, summarize(root).last), 0);

  const collapsed = new Set<string>();
  const visit = (node: CollapsibleNode, isRoot: boolean) => {
    const info = summary.get(node)!;
    const frontier = info.last === newest && newest > 0;
    const own = activity(node.id);
    const foldable = own !== undefined && !isRoot && node.children.length > 0 && !info.live && !frontier
      && (own.failed || now - info.last > STALE_BRANCH_MS);
    if (foldable) {
      collapsed.add(node.id);
      return;
    }
    node.children.forEach((child) => visit(child, false));
  };
  roots.forEach((root) => visit(root, true));
  return collapsed;
}

/** Folded ids after manual choices: manual folds always apply, manual opens
 * override auto-tidy, and nothing hides a node the user must see. */
export function effectiveCollapsedIds({
  manualCollapsed, manualExpanded, auto, reveal,
}: {
  manualCollapsed: readonly string[];
  manualExpanded: readonly string[];
  auto: ReadonlySet<string> | null;
  /** Ancestors of selected / matching nodes: never folded. */
  reveal: ReadonlySet<string>;
}): Set<string> {
  const expanded = new Set(manualExpanded);
  const result = new Set(manualCollapsed);
  if (auto) for (const id of auto) if (!expanded.has(id)) result.add(id);
  for (const id of reveal) result.delete(id);
  return result;
}

/** Flip one node's fold state, recording it as a manual choice. */
export function toggleCollapsed(
  state: { collapsed: string[]; expanded: string[] },
  id: string,
  currentlyCollapsed: boolean,
): { collapsed: string[]; expanded: string[] } {
  const collapsed = state.collapsed.filter((other) => other !== id);
  const expanded = state.expanded.filter((other) => other !== id);
  if (currentlyCollapsed) expanded.push(id);
  else collapsed.push(id);
  return { collapsed, expanded };
}
