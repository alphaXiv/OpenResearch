import { readLiveSnapshot, mergeLiveList } from "./live";
import { queryOptions } from "@tanstack/react-query";
import * as api from "../api";
import { workspaceKey, deletedSessionIds } from "./client";

export const listChatSessionsQuery = (projectId: string) => queryOptions({
  queryKey: workspaceKey("listChatSessions", projectId),
  queryFn: ({ signal, client, queryKey }) => readLiveSnapshot(client, queryKey, async (): Promise<api.ChatSession[]> => {
    const rows = await api.listChatSessions(projectId, signal);
    const previous = client.getQueryData<api.ChatSession[]>(queryKey);
    return rows.filter((row) => !deletedSessionIds.has(row.id)).map((row) => ({
      ...row, contextUsage: row.contextUsage ?? previous?.find((old) => old.id === row.id)?.contextUsage,
    }));
  }, mergeLiveList),
  staleTime: 30_000,
});

export const getChatMessagesQuery = (sessionId: string) => queryOptions({
  queryKey: workspaceKey("getChatMessages", sessionId),
  queryFn: async ({ signal, client, queryKey }) => {
    const before = client.getQueryData<Awaited<ReturnType<typeof api.getChatMessages>>>(queryKey);
    const data = await readLiveSnapshot(client, queryKey, () => api.getChatMessages(sessionId, signal), (incoming, current, changed) => {
      if (!current) return incoming;
      const messageIds = new Set([...changed].filter((id) => id.startsWith("message:")).map((id) => id.slice(8)));
      return {
        messages: mergeLiveList(incoming.messages, current.messages, messageIds, false),
        queued: changed.has("queued") ? current.queued : incoming.queued,
        activeLeafId: changed.has("branch") || messageIds.size ? current.activeLeafId : incoming.activeLeafId,
      };
    });
    signal.throwIfAborted();
    const current = client.getQueryData<Awaited<ReturnType<typeof api.getChatMessages>>>(queryKey);
    // A local turn or branch choice can precede its persisted server event.
    const newUser = before !== undefined && data.messages.some((message) => message.role === "user" && !before?.messages.some((old) => old.id === message.id));
    const optimistic = newUser ? [] : current?.messages.filter((message) => message.id.startsWith("local-")) ?? [];
    const localLeaf = optimistic.some((message) => message.id === current?.activeLeafId);
    return {
      ...data,
      messages: [...data.messages, ...optimistic.filter((message) => !data.messages.some((row) => row.id === message.id))],
      activeLeafId: current && (localLeaf || (!current.activeLeafId?.startsWith("local-") && current.activeLeafId !== before?.activeLeafId)) ? current.activeLeafId : data.activeLeafId,
    };
  },
  staleTime: Infinity,
  refetchOnMount: "always",
  refetchOnWindowFocus: "always",
});
