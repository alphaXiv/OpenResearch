import type { WorkspaceEvents } from "../workspace";
import type { Api, ChatSession, ChatSessionPatch, HarnessId, NativeChat, PromptAnswer, SidebarChatPage, TurnOptions } from "../api";
import type { WorkspaceClient } from "./client";

import { readLiveSnapshot, mergeLiveList, markLiveUpdate } from "./live";
import { mutations } from "./mutations";
import type { Workspace } from "../workspace";
import { infiniteQueryOptions, queryOptions, type InfiniteData } from "@tanstack/react-query";
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

  const listSidebarChatSessionsQuery = (filter: "active" | "archived" | "all") => infiniteQueryOptions({
    queryKey: workspaceKey("listSidebarChatSessions", filter),
    queryFn: ({ pageParam, signal }) => readSidebarChatPage(filter, pageParam, signal),
    initialPageParam: null as api.SidebarChatCursor | null,
    getNextPageParam: (page) => page.next ?? undefined,
  });

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
    listSidebarChatSessionsQuery,
    listChatActiveRunsQuery,
  };
}

type ChatTurn = ReturnType<Workspace["chat"]["reserve"]>;
const heldTurns = new Map<string, ChatTurn[]>();

/**
 * Takes a write's place among the chat's writes and sends when it is made, from `optimistic`. The library runs one
 * definition's calls for a key one after another, so joining `chat.serialize` from `mutate` would let a send made
 * while an earlier write was pending go first.
 */
function holdChatTurn(id: string, ws: Workspace, sessionId: string, onRestore: (restore: () => void) => void) {
  const key = `${id}\0${ws.identity}\0${sessionId}`;
  const turn = ws.chat.reserve();
  heldTurns.set(key, [...heldTurns.get(key) ?? [], turn]);
  onRestore(() => {
    const rest = heldTurns.get(key)?.filter((held) => held !== turn) ?? [];
    if (rest.length) heldTurns.set(key, rest);
    else heldTurns.delete(key);
    turn.release();
  });
}

/** Runs `write` in the place `holdChatTurn` took for this call; the library calls `mutate` in the order calls were made. */
function inChatTurn<T>(id: string, ws: Workspace, sessionId: string, write: () => Promise<T>) {
  const key = `${id}\0${ws.identity}\0${sessionId}`;
  const [turn, ...rest] = heldTurns.get(key) ?? [];
  if (rest.length) heldTurns.set(key, rest);
  else heldTurns.delete(key);
  return turn!.run(write);
}

/** Joins the workspace's chat writes, so a turn sent after a setting starts with it. */
export const mutPatchChatSession = mutations.define({
  id: "chat/session",
  mutate: (ws: Workspace, session: Pick<ChatSession, "id" | "projectId">, changes: ChatSessionPatch) =>
    inChatTurn("chat/session", ws, session.id, () => ws.api.patchChatSession(session.id, changes)),
  optimistic({ helpers, args: [ws, session, changes], onSuccess, onRestore }) {
    holdChatTurn("chat/session", ws, session.id, onRestore);
    const { agent: _agent, notifyParent, ...fields } = changes;
    const shown: Partial<ChatSession> = notifyParent === false ? { ...fields, delegate: null } : fields;
    const options = ws.queries.listChatSessionsQuery(session.projectId);
    helpers(ws.queryClient).arrayUpdate(options, {
      filter: (row) => row.id === session.id,
      update: (row) => ({ ...row, ...shown }),
    });
    onSuccess((saved) => {
      markLiveUpdate(ws.queryClient, options.queryKey, saved.id);
      ws.setScopedQueryData(options.queryKey, (rows) => rows?.map((row) => row.id === saved.id ? saved : row));
    });
  },
  key: ({ args: [ws, session] }) => [ws.identity, session.id],
  describe: "update a chat",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutDeleteChatSession = mutations.define({
  id: "chat/session/delete",
  mutate: (ws: Workspace, session: Pick<ChatSession, "id" | "projectId">) => ws.api.deleteChatSession(session.id),
  optimistic({ helpers, args: [ws, session] }) {
    const cache = helpers(ws.queryClient);
    cache.arrayRemove(ws.queries.listChatSessionsQuery(session.projectId), (row) => row.id === session.id);
    for (const filter of ["active", "archived", "all"] as const) {
      const { queryKey } = ws.queries.listSidebarChatSessionsQuery(filter);
      cache.updateExisting<InfiniteData<SidebarChatPage>>({ queryKey }, (data) => ({
        ...data,
        pages: data.pages.map((page) => ({ ...page, sessions: page.sessions.filter((row) => row.id !== session.id) })),
      }));
    }
  },
  key: ({ args: [ws, session] }) => [ws.identity, session.id],
  describe: "delete the chat",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutCreateChatSession = mutations.define({
  id: "chat/session/create",
  mutate: (ws: Workspace, projectId: string, harness: HarnessId, options: NonNullable<Parameters<Api["createChatSession"]>[2]>) =>
    ws.api.createChatSession(projectId, harness, options),
  optimistic: () => {},
  key: ({ args: [ws, projectId] }) => [ws.identity, projectId],
  describe: "start the chat",
  describeResult: null,
});

export const mutOpenSideChat = mutations.define({
  id: "chat/session/side",
  mutate: (ws: Workspace, parentSessionId: string) => ws.api.openSideChat(parentSessionId),
  optimistic: () => {},
  key: ({ args: [ws, parentSessionId] }) => [ws.identity, parentSessionId],
  describe: "open a side chat",
  describeResult: null,
});

export const mutImportNativeChat = mutations.define({
  id: "chat/session/import",
  mutate: (ws: Workspace, projectId: string, chat: NativeChat) => ws.api.importNativeChat(projectId, chat),
  optimistic({ helpers, args: [ws, , chat] }) {
    helpers(ws.queryClient).arrayRemove(ws.queries.listNativeChatsQuery(), (row) => row.harness === chat.harness && row.nativeId === chat.nativeId);
  },
  key: ({ args: [ws, projectId] }) => [ws.identity, projectId],
  describe: "import the chat",
  describeResult: null,
});

/** `configuration` with one ACP option set to `value`, in whichever shape the agent reported it. */
function withNativeValue(configuration: unknown, optionId: string, value: string): unknown {
  if (typeof configuration !== "object" || configuration === null) return configuration;
  const config: Record<string, unknown> = { ...configuration };
  if (Array.isArray(config.configOptions)) {
    config.configOptions = config.configOptions.map((option: unknown) =>
      typeof option === "object" && option !== null && "id" in option && option.id === optionId ? { ...option, currentValue: value } : option);
    return config;
  }
  const [section, field] = optionId === "model" ? ["models", "currentModelId"] : ["modes", "currentModeId"];
  const current = config[section];
  if (typeof current === "object" && current !== null) config[section] = { ...current, [field]: value };
  return config;
}

export const mutSetChatConfiguration = mutations.define({
  id: "chat/session/configuration",
  mutate: (ws: Workspace, session: Pick<ChatSession, "id" | "projectId">, optionId: string, value: string) =>
    inChatTurn("chat/session/configuration", ws, session.id, () => ws.api.setChatConfiguration(session.id, optionId, value)),
  optimistic({ helpers, args: [ws, session, optionId, value], onSuccess, onRestore }) {
    holdChatTurn("chat/session/configuration", ws, session.id, onRestore);
    const options = ws.queries.listChatSessionsQuery(session.projectId);
    helpers(ws.queryClient).arrayUpdate(options, {
      filter: (row) => row.id === session.id,
      update: (row) => ({ ...row, nativeConfiguration: withNativeValue(row.nativeConfiguration, optionId, value) }),
    });
    onSuccess((saved) => {
      markLiveUpdate(ws.queryClient, options.queryKey, saved.id);
      ws.setScopedQueryData(options.queryKey, (rows) => rows?.map((row) => row.id === saved.id ? saved : row));
    });
  },
  key: ({ args: [ws, session] }) => [ws.identity, session.id],
  describe: "change the agent's setting",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutCompactChatSession = mutations.define({
  id: "chat/session/compact",
  mutate: (ws: Workspace, sessionId: string) => ws.api.compactChatSession(sessionId),
  optimistic: () => {},
  key: ({ args: [ws, sessionId] }) => [ws.identity, sessionId],
  describe: "compact the chat",
  describeResult: null,
});

export const mutRunShellCommand = mutations.define({
  id: "chat/shell",
  mutate: (ws: Workspace, sessionId: string, command: string) => ws.api.runShellCommand(sessionId, command),
  optimistic: () => {},
  key: ({ args: [ws, sessionId] }) => [ws.identity, sessionId],
  describe: "run the command",
  describeResult: null,
});

export const mutInterruptChat = mutations.define({
  id: "chat/interrupt",
  mutate: (ws: Workspace, sessionId: string) => ws.api.interruptChat(sessionId),
  optimistic: () => {},
  key: ({ args: [ws, sessionId] }) => [ws.identity, sessionId],
  describe: "stop the turn",
  describeResult: null,
});

export const mutRecoverChatTurn = mutations.define({
  id: "chat/turn/recover",
  mutate: (ws: Workspace, sessionId: string, turnId: string, action: "retry" | "continue", options: TurnOptions) =>
    ws.api.recoverChatTurn(sessionId, turnId, action, options),
  optimistic: () => {},
  key: ({ args: [ws, sessionId] }) => [ws.identity, sessionId],
  describe: "recover the turn",
  describeResult: null,
});

export const mutForkChatTurn = mutations.define({
  id: "chat/turn/fork",
  mutate: (ws: Workspace, session: Pick<ChatSession, "id" | "projectId">, messageId: string, text: string) =>
    inChatTurn("chat/turn/fork", ws, session.id, () => ws.api.forkChatTurn(session.id, messageId, text)),
  optimistic({ helpers, args: [ws, session], onRestore }) {
    holdChatTurn("chat/turn/fork", ws, session.id, onRestore);
    helpers(ws.queryClient).arrayUpdate(ws.queries.listChatSessionsQuery(session.projectId), {
      filter: (row) => row.id === session.id,
      update: (row) => ({ ...row, busy: true }),
    });
  },
  key: ({ args: [ws, session] }) => [ws.identity, session.id],
  describe: "re-send the message",
  describeResult: null,
  refetchOnSuccess: false,
});

/** The server descends to the newest tip under `leafId` and echoes the real leaf over `chat.branch`. */
export const mutSelectChatBranch = mutations.define({
  id: "chat/branch",
  mutate: (ws: Workspace, sessionId: string, leafId: string) =>
    inChatTurn("chat/branch", ws, sessionId, () => ws.api.selectChatBranch(sessionId, leafId)),
  optimistic({ helpers, args: [ws, sessionId, leafId], onRestore }) {
    holdChatTurn("chat/branch", ws, sessionId, onRestore);
    helpers(ws.queryClient).updateExisting(ws.queries.getChatMessagesQuery(sessionId), (snapshot) => ({ ...snapshot, activeLeafId: leafId }));
  },
  key: ({ args: [ws, sessionId] }) => [ws.identity, sessionId],
  describe: "switch fork",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutCancelQueuedMessage = mutations.define({
  id: "chat/queue/cancel",
  mutate: (ws: Workspace, sessionId: string, itemId: string) => ws.api.cancelQueuedMessage(sessionId, itemId),
  optimistic({ helpers, args: [ws, sessionId, itemId] }) {
    helpers(ws.queryClient).updateExisting(ws.queries.getChatMessagesQuery(sessionId), (snapshot) => ({
      ...snapshot,
      queued: snapshot.queued.filter((item) => item.id !== itemId),
    }));
  },
  key: ({ args: [ws, sessionId] }) => [ws.identity, sessionId],
  describe: "remove the queued message",
  describeResult: null,
});

export const mutRetryQueuedMessage = mutations.define({
  id: "chat/queue/retry",
  mutate: (ws: Workspace, sessionId: string, itemId: string) => ws.api.retryQueuedMessage(sessionId, itemId),
  optimistic({ helpers, args: [ws, sessionId, itemId] }) {
    helpers(ws.queryClient).updateExisting(ws.queries.getChatMessagesQuery(sessionId), (snapshot) => ({
      ...snapshot,
      queued: snapshot.queued.map((item) => item.id === itemId ? { ...item, dispatchState: "queued" as const, error: null, nextRetryAt: null } : item),
    }));
  },
  key: ({ args: [ws, sessionId] }) => [ws.identity, sessionId],
  describe: "retry the queued message",
  describeResult: null,
});

/** Marks the resumed turn busy; the card stays mounted, holding its draft, until the server resolves it. Refetches the
 * transcript on success too: a stale card's answer no-ops server-side and nothing re-broadcasts. */
export const mutRespondChat = mutations.define({
  id: "chat/respond",
  mutate: (ws: Workspace, session: Pick<ChatSession, "id" | "projectId">, answer: PromptAnswer) =>
    inChatTurn("chat/respond", ws, session.id, () => ws.api.respondChat(session.id, answer)),
  optimistic({ helpers, args: [ws, session], onRestore }) {
    holdChatTurn("chat/respond", ws, session.id, onRestore);
    const cache = helpers(ws.queryClient);
    cache.arrayUpdate(ws.queries.listChatSessionsQuery(session.projectId), {
      filter: (row) => row.id === session.id,
      update: (row) => ({ ...row, busy: true }),
    });
    cache.refetchOnSettled(ws.queries.getChatMessagesQuery(session.id));
  },
  key: ({ args: [ws, session] }) => [ws.identity, session.id],
  describe: "send the answer",
  describeResult: null,
});
