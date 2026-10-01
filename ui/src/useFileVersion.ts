import { useQuery, queryOptions } from "@tanstack/react-query";
import { workspaceKey } from "./queries/client";

export const fileVersionQuery = (url: string) => queryOptions({
  queryKey: workspaceKey("fileVersion", url),
  staleTime: 2_000,
  refetchOnMount: "always",
  refetchOnWindowFocus: "always",
  refetchInterval: 2_000,
  queryFn: async ({ signal }) => {
    const response = await fetch(url, { method: "HEAD", cache: "no-store", signal });
    if (response.status === 404) return "missing";
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return response.headers.get("etag") ?? response.headers.get("content-length");
  },
});

export function useFileVersion(url: string, enabled = true) {
  const query = useQuery({ ...fileVersionQuery(url), enabled, subscribed: enabled });
  return query.data ?? null;
}
