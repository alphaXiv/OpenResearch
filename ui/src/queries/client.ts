import { QueryClient, type QueryFilters, type QueryKey, type DataTag } from "@tanstack/react-query";

import { m } from "../paraglide/messages.js";

/** The connection to orx up dropped before an answer arrived. */
export class ConnectionLostError extends Error {}

/** One cache for every machine; each workspace's keys start with its own scope. */
export const createQueryClient = () =>
  new QueryClient({
    defaultOptions: {
      queries: {
        staleTime: 30_000,
        // A hidden machine's screen stops observing, so this is how long switching back stays instant.
        gcTime: 3_600_000,
        retry: false,
        networkMode: "always",
        refetchOnReconnect: true,
      },
      mutations: { retry: false, networkMode: "always" },
    },
  });

export function createWorkspaceClient(queryClient: QueryClient, identity: string, basePath = "") {
  const scope = ["workspace", identity] as const;
  const lifetime = new AbortController();
  const listeners = new Set<() => void>();
  const deletedSessionIds = new Set<string>();
  const workspaceScope = () => scope;
  const workspaceKey = (family: string, ...args: readonly unknown[]) =>
    [...scope, family, ...args] as const;
  const isCurrentScope = (key: readonly unknown[]) =>
    !lifetime.signal.aborted && key[1] === identity;

  function url(path: string): string {
    if (/^(https?:)?\/\//i.test(path)) {
      const target = new URL(path, location.href);
      if (target.origin !== location.origin) return path;
      path = target.pathname + target.search + target.hash;
    }
    if (path.startsWith("/_orx/workspaces/")) {
      if (basePath && path.startsWith(`${basePath}/`)) return path;
      throw new Error("Resource belongs to another workspace");
    }
    return path.startsWith("/api/") || path.startsWith("/_orx/")
      ? `${basePath}${path}`
      : path;
  }

  function request(path: string, init: RequestInit = {}) {
    lifetime.signal.throwIfAborted();
    const signal = init.signal
      ? AbortSignal.any([init.signal, lifetime.signal])
      : lifetime.signal;
    // Browsers report a dropped connection as a bare TypeError ("Load failed", "Failed to fetch").
    return fetch(url(path), { ...init, signal }).catch((error: unknown) => {
      if (error instanceof TypeError && !signal.aborted) throw new ConnectionLostError(m.request_connection_lost(), { cause: error });
      throw error;
    });
  }

  function socketUrl(path: string) {
    const target = new URL(url(path), location.href);
    target.protocol = target.protocol === "https:" ? "wss:" : "ws:";
    return target;
  }

  async function cancelAndRemove(filters: QueryFilters) {
    await queryClient.cancelQueries(filters, { revert: false });
    queryClient.removeQueries(filters);
  }

  function setScopedQueryData<T>(key: DataTag<QueryKey, T, unknown>, updater: T | undefined | ((previous: T | undefined) => T | undefined)) {
    if (isCurrentScope(key)) return queryClient.setQueryData<T, typeof key, T>(key, updater);
  }

  function replaceWorkspace() {
    lifetime.abort();
    void cancelAndRemove({ queryKey: scope });
    deletedSessionIds.clear();
    listeners.forEach((listener) => listener());
  }

  const subscribeReplacement = (listener: () => void) => {
    listeners.add(listener);
    return () => { listeners.delete(listener); };
  };

  return {
    identity,
    queryClient,
    workspaceScope,
    workspaceKey,
    isCurrentScope,
    deletedSessionIds,
    url,
    request,
    socketUrl,
    cancelAndRemove,
    setScopedQueryData,
    replaceWorkspace,
    subscribeReplacement,
  };
}

export type WorkspaceClient = ReturnType<typeof createWorkspaceClient>;
