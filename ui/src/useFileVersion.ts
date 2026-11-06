import { useQuery, queryOptions } from "@tanstack/react-query";
import type { WorkspaceClient } from "./queries/client";
import { useWorkspace } from "./workspace";

export const fileVersionQuery = (client: WorkspaceClient, url: string) => queryOptions({
  queryKey: client.workspaceKey("fileVersion", url),
  staleTime: 2_000,
  refetchOnMount: "always",
  refetchOnWindowFocus: "always",
  refetchInterval: 2_000,
  queryFn: async ({ signal }) => {
    const response = await client.request(url, { method: "HEAD", cache: "no-store", signal });
    if (response.status === 404) return "missing";
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return response.headers.get("etag") ?? response.headers.get("content-length");
  },
});

export function useFileVersion(url: string, enabled = true) {
  const client = useWorkspace();
  const query = useQuery({ ...fileVersionQuery(client, url), enabled, subscribed: enabled });
  return query.data ?? null;
}
