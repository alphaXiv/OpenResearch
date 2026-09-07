import { QueryClient, type QueryFilters, type QueryKey, type DataTag } from "@tanstack/react-query";

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: { staleTime: 30_000, gcTime: 600_000, retry: false, networkMode: "always", refetchOnReconnect: true },
    mutations: { retry: false, networkMode: "always" },
  },
});

export const deletedSessionIds = new Set<string>();

let identity: string | null = null;
let generation = 0;
const listeners = new Set<() => void>();
export const getWorkspaceGeneration = () => generation;
export const subscribeWorkspace = (listener: () => void) => {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
};
export const replaceWorkspace = () => { if (identity !== null) activateWorkspace(identity, true); };
export const workspaceScope = () => ["workspace", generation] as const;
export const workspaceKey = (family: string, ...args: readonly unknown[]) =>
  [...workspaceScope(), family, ...args] as const;

export function activateWorkspace(next: string, replace = false) {
  if (identity === next && !replace) return false;
  const previous = workspaceScope();
  identity = next;
  generation++;
  deletedSessionIds.clear();
  void cancelAndRemove({ queryKey: previous });
  listeners.forEach((listener) => listener());
  return true;
}

export const isCurrentScope = (scope: readonly unknown[]) => scope[1] === generation;

export async function cancelAndRemove(filters: QueryFilters) {
  await queryClient.cancelQueries(filters, { revert: false });
  queryClient.removeQueries(filters);
}

export function setScopedQueryData<T>(key: DataTag<QueryKey, T, unknown>, updater: T | undefined | ((previous: T | undefined) => T | undefined)) {
  if (isCurrentScope(key)) return queryClient.setQueryData<T, typeof key, T>(key, updater);
}
