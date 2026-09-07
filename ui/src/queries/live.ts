import type { QueryClient, QueryKey } from "@tanstack/react-query";

const revisions = new WeakMap<object, number>();
const repairs = new WeakMap<object, Set<string>>();

export function markLiveUpdate(client: QueryClient, queryKey: QueryKey, id = "*") {
  for (const query of client.getQueryCache().findAll({ queryKey })) {
    revisions.set(query, (revisions.get(query) ?? 0) + 1);
    repairs.get(query)?.add(id);
  }
}

export function mergeLiveList<T extends { id: string }>(incoming: T[], current: T[] | undefined, changed: ReadonlySet<string>, prepend = true): T[] {
  if (!current) return incoming;
  const latest = new Map(current.map((row) => [row.id, row]));
  const ids = new Set(incoming.map((row) => row.id));
  const added = current.filter((row) => changed.has(row.id) && !ids.has(row.id));
  const rows = incoming.filter((row) => !changed.has(row.id) || latest.has(row.id))
    .map((row) => changed.has(row.id) ? latest.get(row.id) ?? row : row);
  return prepend ? [...added, ...rows] : [...rows, ...added];
}

export async function readLiveSnapshot<T>(
  client: QueryClient,
  queryKey: QueryKey,
  read: () => Promise<T>,
  merge: (incoming: T, current: T | undefined, changed: ReadonlySet<string>) => T = (incoming, current) => current ?? incoming,
) {
  const query = client.getQueryCache().find({ queryKey, exact: true });
  const revision = query ? revisions.get(query) : undefined;
  const data = await read();
  if (!query || revisions.get(query) === revision || query.state.fetchStatus !== "fetching" || client.getQueryCache().find({ queryKey, exact: true }) !== query) return data;
  // Publish a complete baseline so a streaming cold load can render during repair.
  client.setQueryData<T>(queryKey, (current) => current ?? data);
  const changed = new Set<string>();
  repairs.set(query, changed);
  try {
    const repaired = await read();
    return changed.size ? merge(repaired, client.getQueryData<T>(queryKey), changed) : repaired;
  } finally {
    if (repairs.get(query) === changed) repairs.delete(query);
  }
}
