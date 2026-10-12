import type { Experiment, Run } from "./api";

export interface RouteNode {
  experiment: Experiment;
  parentId: string | null;
  children: string[];
  context: boolean;
  boundary: "missing" | "cycle" | null;
}

const compare = (a: string, b: string) => a < b ? -1 : a > b ? 1 : 0;

/** Retain real ancestors across scope/archive filters; never invent an edge. */
export function buildRoutes(experiments: Experiment[], sessionId: string | null, showArchived: boolean) {
  const ordered = [...experiments].sort((a, b) => a.createdAt - b.createdAt || compare(a.id, b.id));
  const nodes = new Map<string, RouteNode>(ordered.map((experiment) => [experiment.id, {
    experiment, parentId: experiment.parentExperimentId ?? null, children: [], context: false, boundary: null,
  }]));
  for (const node of nodes.values()) {
    if (node.parentId && !nodes.has(node.parentId)) {
      node.parentId = null;
      node.boundary = "missing";
    }
  }
  // Corrupt legacy/imported relationships must not hang the tree. Break one
  // edge per cycle deterministically and label it instead of calling it a root.
  const visited = new Set<string>();
  for (const id of nodes.keys()) {
    const path: string[] = [];
    const positions = new Map<string, number>();
    let current: string | null = id;
    while (current && !visited.has(current)) {
      const position = positions.get(current);
      if (position !== undefined) {
        const breakId = path.slice(position).sort(compare)[0];
        const node = nodes.get(breakId)!;
        node.parentId = null;
        node.boundary = "cycle";
        break;
      }
      positions.set(current, path.length);
      path.push(current);
      current = nodes.get(current)!.parentId;
    }
    for (const item of path) visited.add(item);
  }
  const targets = new Set(ordered.filter((experiment) =>
    (!sessionId || experiment.chatSessionId === sessionId) && (showArchived || !experiment.archived),
  ).map((experiment) => experiment.id));
  const included = new Set<string>();
  for (const id of targets) {
    let current: string | null = id;
    while (current && !included.has(current)) {
      included.add(current);
      current = nodes.get(current)!.parentId;
    }
  }
  const roots: string[] = [];
  for (const [id, node] of nodes) {
    if (!included.has(id)) { nodes.delete(id); continue; }
    node.context = !targets.has(id);
    if (node.parentId) nodes.get(node.parentId)!.children.push(id);
    else roots.push(id);
  }
  return { nodes, roots };
}

export interface RouteRow {
  node: RouteNode;
  depth: number;
  position: number;
  size: number;
  /** Which ancestor levels have another sibling below this row. */
  continuation: boolean[];
}

export function routeRows(routes: ReturnType<typeof buildRoutes>, collapsed: ReadonlySet<string>): RouteRow[] {
  const rows: RouteRow[] = [];
  const stack = routes.roots.map((id, index) => ({ id, depth: 0, position: index + 1, size: routes.roots.length, continuation: [] as boolean[] })).reverse();
  while (stack.length) {
    const item = stack.pop()!;
    const node = routes.nodes.get(item.id)!;
    rows.push({ node, ...item });
    if (collapsed.has(item.id)) continue;
    for (let index = node.children.length - 1; index >= 0; index--) {
      stack.push({ id: node.children[index], depth: item.depth + 1, position: index + 1, size: node.children.length,
        continuation: item.depth === 0 ? [false] : [...item.continuation.slice(0, -1), item.position < item.size, false] });
    }
  }
  return rows;
}

/** Return the nearest visible ancestor after folding or filtering. */
export function visibleRouteId(id: string | null, routes: ReturnType<typeof buildRoutes>, rows: RouteRow[]) {
  const visible = new Set(rows.map((row) => row.node.experiment.id));
  let current = id;
  while (current && !visible.has(current)) current = routes.nodes.get(current)?.parentId ?? null;
  return current ?? rows[0]?.node.experiment.id ?? null;
}

/** Match the table's live-first policy, with deterministic ties on API refresh. */
export function routeRun(runs: Run[]): Run | null {
  const sorted = [...runs].sort((a, b) => b.createdAt - a.createdAt || compare(a.id, b.id));
  return sorted.find((run) => run.status === "starting" || run.status === "running") ?? sorted[0] ?? null;
}

export function parseRouteFolds(value: unknown): Set<string> {
  return new Set(Array.isArray(value) ? value.filter((id): id is string => typeof id === "string") : []);
}
