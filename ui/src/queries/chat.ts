import type { WorkspaceEvents } from "../workspace";
import type { Api } from "../api";
import type { WorkspaceClient } from "./client";

import { readLiveSnapshot, mergeLiveList } from "./live";
import { infiniteQueryOptions, queryOptions } from "@tanstack/react-query";
import type * as api from "../api";

export function createChatQueries(
  client: WorkspaceClient,
  api: Api,
  events: WorkspaceEvents,
) {
  const { workspaceKey, deletedSessionIds } = client;
  const { onChatEvent } = events;

  const listChatSessionsQuery = (projectId: string) => queryOptions({
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

  const getSessionExperimentsQuery = (sessionId: string) => queryOptions({
    queryKey: workspaceKey("getSessionExperiments", sessionId),
    queryFn: ({ signal }) => api.getSessionExperiments(sessionId, signal),
    staleTime: 0,
  });

  /** Scans the agents' own stores, so it is read once per picker open. */
  const listNativeChatsQuery = () => queryOptions({
    queryKey: workspaceKey("listNativeChats"),
    queryFn: ({ signal }) => api.listNativeChats(signal),
    staleTime: 0,
  });

  /** The `/resume` picker's one-shot list: no live merge, refetched each open. */
  const listAllChatSessionsQuery = () => queryOptions({
    queryKey: workspaceKey("listAllChatSessions"),
    queryFn: async ({ signal }) =>
      (await api.listAllChatSessions(signal)).filter((row) => !deletedSessionIds.has(row.id)),
    staleTime: 0,
  });

  const chatSearchQuery = (params: api.ChatSearchParams) => {
    const queryKey = workspaceKey("chatSearch", params.scope, params.sessionId ?? null, params.projectId ?? null, params.q);
    return infiniteQueryOptions({
      queryKey,
      initialPageParam: null as string | null,
      queryFn: ({ pageParam, signal }) => api.searchChatMessages(params, pageParam, signal),
      getNextPageParam: (last) => last.cursor,
      enabled: params.q.length > 0,
      staleTime: 0,
      refetchOnWindowFocus: false,
      refetchInterval: (query) => query.state.data?.pages[0]?.indexing ? 1000 : false,
      placeholderData: (previous, query) => {
        return params.q && query?.queryKey.slice(0, -1).every((part, index) => part === queryKey[index]) ? previous : undefined;
      },
    });
  };

  const getChatMessagesQuery = (sessionId: string) =>
    queryOptions({
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
        const optimistic = newUser
          ? []
          : (current?.messages.filter((message) =>
              message.id.startsWith("local-"),
            ) ?? []);
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

  async function readSidebarChatPage(filter: "active" | "archived" | "all", before: api.SidebarChatCursor | null, signal: AbortSignal) {
    const indicators = new Map<string, Partial<Pick<api.ChatSession, "busy" | "contextUsage">>>();
    const off = onChatEvent((event) => {
      if (event.type === "busy" || event.type === "usage") {
        indicators.set(event.sessionId, { ...indicators.get(event.sessionId),
          ...(event.type === "busy" ? { busy: event.busy } : { contextUsage: event.usage }),
        });
      }
    });
    try {
      const page = await api.listSidebarChatSessions(filter, before, signal);
      signal.throwIfAborted();
      return { ...page, sessions: page.sessions.map((session) => ({ ...session, ...indicators.get(session.id) })) };
    } finally { off(); }
  }

  /** The chat, author and first words of the message a ref cites. */
  const citedMessageQuery = (cited: string) => queryOptions({
    queryKey: workspaceKey("citedMessage", cited),
    queryFn: () => api.citedMessage(cited),
    staleTime: Infinity,
    retry: false,
  });

  const listChatActiveRunsQuery = () => queryOptions({
    queryKey: workspaceKey("listChatActiveRuns"),
    queryFn: ({ signal }) => api.listChatActiveRuns(signal),
    staleTime: Infinity,
  });

  return {
    citedMessageQuery,
    listChatSessionsQuery,
    getSessionExperimentsQuery,
    listNativeChatsQuery,
    listAllChatSessionsQuery,
    chatSearchQuery,
    getChatMessagesQuery,
    readSidebarChatPage,
    listChatActiveRunsQuery,
  };
}
