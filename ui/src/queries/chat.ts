import type { WorkspaceEvents } from "../workspace";
import type { AgentSelection, Api, ChatReportChoice, ChatSession, ChatSessionPatch, HarnessId, NativeChat, PromptAnswer, SidebarCard, SidebarChatPage, SidebarChatTotals, TurnOptions } from "../api";
import type { ChatEvent } from "../events";
import type { WorkspaceClient } from "./client";

import { readLiveSnapshot, mergeLiveList, markLiveUpdate } from "./live";
import { mutations } from "./mutations";
import type { Workspace } from "../workspace";
import { CancelledError, infiniteQueryOptions, queryOptions, type InfiniteData, type QueryClient, type QueryKey } from "@tanstack/react-query";
import type * as api from "../api";
import type { ShownRun } from "../runMonitor/types";

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
    }, (incoming, current, changed) => {
      const summaries = new Map(incoming.map((row) => [row.id, row.awaitingInput]));
      return mergeLiveList(incoming, current, changed).map((row) => ({ ...row, awaitingInput: summaries.get(row.id) ?? row.awaitingInput }));
    }),
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

  async function refreshChatMessages(sessionId: string) {
    const options = { ...getChatMessagesQuery(sessionId), staleTime: 0 };
    // A reconnect seed needs a new read without rejecting callers of a pre-gap cold read.
    if (client.queryClient.isFetching({ queryKey: options.queryKey, exact: true })) {
      await client.queryClient.fetchQuery(options).catch(() => {});
    }
    return client.queryClient.fetchQuery(options).catch((error: unknown) => {
      if (error instanceof CancelledError && error.silent) return client.queryClient.fetchQuery(options);
      throw error;
    });
  }

  /** Cards that arrived during a list's fetch, by query; replayed onto the whole list once the fetch lands. */
  const cardsDuringFetch = new Set<string>();

  async function readSidebarChatPage(filter: SidebarFilter, before: api.SidebarChatCursor | null, signal: AbortSignal) {
    const queryKey = workspaceKey("listSidebarChatSessions", filter);
    const query = client.queryClient.getQueryCache().find({ queryKey, exact: true });
    if (query && !cardsDuringFetch.has(query.queryHash)) {
      cardsDuringFetch.add(query.queryHash);
      const cards = new Map<string, SidebarCard>();
      let totals: SidebarChatTotals | undefined;
      const offCards = onChatEvent((event) => {
        if (event.type !== "sidebar") return;
        for (const card of event.cards) cards.set(card.sessionId, card);
        totals = event.totals;
      });
      // The fetch builds the list from the pages it started with, so a card applied meanwhile is put back on top.
      const offFetch = client.queryClient.getQueryCache().subscribe((event) => {
        if (event.query !== query || event.type !== "updated" || query.state.fetchStatus !== "idle") return;
        offCards();
        offFetch();
        cardsDuringFetch.delete(query.queryHash);
        if (event.action.type === "success") replaySidebarCards(client.queryClient, queryKey, [...cards.values()], totals);
      });
    }

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

  const listSidebarChatSessionsQuery = (filter: SidebarFilter) => infiniteQueryOptions({
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

  /** `source` is "active" against an older server: its runs carry no monitor, and none can be fetched. */
  const listChatShownRunsQuery = () => queryOptions({
    queryKey: workspaceKey("listChatShownRuns"),
    queryFn: async ({ signal, client, queryKey }) => {
      const read = await readLiveSnapshot(client, queryKey, async (): Promise<ShownRuns> => {
        const shown = await api.listChatShownRuns(signal);
        return shown ? { source: "shown", ...shown } : { source: "active", runs: await api.listChatActiveRuns(signal) };
      }, (incoming) => incoming);
      // The list has no revision of its own, so each run keeps whichever copy is newer, the cache's or the reply's.
      const cached = new Map(client.getQueryData<ShownRuns>(queryKey)?.runs.map((item) => [item.run.id, item.run]));
      return {
        ...read,
        runs: read.runs.map((item) => {
          const kept = cached.get(item.run.id);
          return kept && kept.updatedAt > item.run.updatedAt ? { ...item, run: kept } : item;
        }),
      };
    },
    staleTime: Infinity,
  });

  /** Kept current by `chat.report` events. */
  const getChatReportQuery = (sessionId: string) => queryOptions({
    queryKey: workspaceKey("getChatReport", sessionId),
    queryFn: ({ signal, client, queryKey }) => readLiveSnapshot(client, queryKey, () => api.getChatReport(sessionId, signal)),
    staleTime: Infinity,
  });

  /** Read again each time the offer shows, since setting up an agent adds models. */
  const listChatReportModelsQuery = (sessionId: string) => queryOptions({
    queryKey: workspaceKey("listChatReportModels", sessionId),
    queryFn: ({ signal }) => api.listChatReportModels(sessionId, signal),
    staleTime: 0,
  });

  return {
    citedMessageQuery,
    listChatSessionsQuery,
    getSessionExperimentsQuery,
    listNativeChatsQuery,
    listAllChatSessionsQuery,
    chatSearchQuery,
    getChatMessagesQuery,
    refreshChatMessages,
    readSidebarChatPage,
    listSidebarChatSessionsQuery,
    listChatShownRunsQuery,
    getChatReportQuery,
    listChatReportModelsQuery,
  };
}

type ShownRuns = { source: "shown" | "active"; runs: ShownRun[]; failedSessions?: string[] };

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

type SidebarFilter = "active" | "archived" | "all";
type SidebarList = InfiniteData<SidebarChatPage>;
type PendingWrite = { fields: Partial<ChatSession>; moving: boolean };

/**
 * Chat writes in flight, by workspace and chat: fields a card read before the write landed must not undo, and
 * whether the write moves the chat between sections in a way no card has shown yet.
 */
const pendingWrites = new Map<unknown, Map<string, PendingWrite[]>>();
const pendingIn = (identity: unknown) => pendingWrites.get(identity) ?? new Map<string, PendingWrite[]>();

/**
 * Puts `row` where the server would list it: replaced in place, dropped when the list no longer holds it, or
 * joined in sort order. One past the loaded pages waits for paging to reach it, unless the server lists it
 * first because it is at work.
 */
function putSidebarRow(data: SidebarList, filter: SidebarFilter, id: string, row: ChatSession | null, atWork = false): SidebarList {
  const belongs = row !== null && (filter === "all" || row.archived === (filter === "archived"));
  const listed = data.pages.some((page) => page.sessions.some((session) => session.id === id));
  if (listed) {
    return {
      ...data,
      pages: data.pages.map((page) => ({
        ...page,
        sessions: page.sessions.flatMap((session) => session.id !== id ? [session] : belongs ? [row] : []),
      })),
    };
  }
  if (row === null || !belongs) return data;

  // A page holds everything down to its cursor, even once the cursor's own row has moved out.
  let placed = false;
  const pages = data.pages.map((page, index) => {
    if (placed) return page;
    const last = index === data.pages.length - 1;
    if (page.next && sortsAfter(row, page.next) && !(last && atWork)) return page;
    placed = true;
    const at = page.sessions.findIndex((session) => sortsAfter(session, row));
    const end = at < 0 ? page.sessions.length : at;
    return { ...page, sessions: [...page.sessions.slice(0, end), row, ...page.sessions.slice(end)] };
  });
  return { ...data, pages };
}

/** Whether `a` lists after `b`, in the server's newest-first order. */
const sortsAfter = (a: { updatedAt: number; id: string }, b: { updatedAt: number; id: string }) =>
  a.updatedAt < b.updatedAt || a.updatedAt === b.updatedAt && a.id < b.id;

/** The server's counts with the moves in flight that it has not shown yet. */
function withPendingMoves(identity: unknown, totals: SidebarChatTotals): SidebarChatTotals {
  let { active, archived } = totals;
  for (const writes of pendingIn(identity).values()) {
    for (const { fields, moving } of writes) {
      if (!moving) continue;
      const step = fields.archived ? 1 : -1;
      active -= step;
      archived += step;
    }
  }
  return { active, archived };
}

function withCounts(data: SidebarList, filter: SidebarFilter, totals: SidebarChatTotals): SidebarList {
  const [first, ...rest] = data.pages;
  if (!first) return data;
  const total = filter === "all" ? totals.active + totals.archived : totals[filter];
  return { ...data, pages: [{ ...first, total, totals }, ...rest] };
}

/** Moves one chat between the sections' counts; a server without `totals` counts each list on its own. */
function shiftCounts(data: SidebarList, filter: SidebarFilter, archived: boolean): SidebarList {
  const [first, ...rest] = data.pages;
  if (!first) return data;
  const step = archived ? 1 : -1;
  if (first.totals) return withCounts(data, filter, { active: first.totals.active - step, archived: first.totals.archived + step });
  if (first.total === undefined || filter === "all") return data;
  return { ...data, pages: [{ ...first, total: first.total + (filter === "archived" ? step : -step) }, ...rest] };
}

/** Edits each loaded sidebar list under `family`, keeping one a resync invalidated due for its read. */
export function editSidebarLists(client: QueryClient, family: QueryKey, edit: (data: SidebarList, filter: SidebarFilter) => SidebarList) {
  for (const query of client.getQueryCache().findAll({ queryKey: family })) {
    const data = query.state.data as SidebarList | undefined;
    if (!data) continue;
    const invalidated = query.state.isInvalidated;
    client.setQueryData(query.queryKey, edit(data, query.queryKey.at(-1) as SidebarFilter));
    if (invalidated) query.invalidate();
  }
}

function findSidebarRow(client: QueryClient, family: QueryKey, id: string) {
  return client.getQueriesData<SidebarList>({ queryKey: family })
    .flatMap(([, data]) => data?.pages.flatMap((page) => page.sessions) ?? [])
    .find((row) => row.id === id);
}

function withPending(identity: unknown, row: ChatSession) {
  return Object.assign({ ...row }, ...(pendingIn(identity).get(row.id) ?? []).map((write) => write.fields));
}

/** A card that shows a write's move means the server's counts include it. */
function acknowledge(identity: unknown, { sessionId, session }: SidebarCard) {
  for (const write of pendingIn(identity).get(sessionId) ?? []) if (session?.archived === write.fields.archived) write.moving = false;
}

const putCard = (identity: unknown, data: SidebarList, filter: SidebarFilter, card: SidebarCard) =>
  putSidebarRow(data, filter, card.sessionId, card.session && withPending(identity, card.session), card.atWork);

export function applySidebarCards(client: QueryClient, scope: QueryKey, { cards, totals }: Extract<ChatEvent, { type: "sidebar" }>) {
  const identity = scope[1];
  // Every card of the read first: its counts already include each move these cards show.
  cards.forEach((card) => acknowledge(identity, card));
  const counts = withPendingMoves(identity, totals);
  editSidebarLists(client, [...scope, "listSidebarChatSessions"], (data, filter) =>
    withCounts(cards.reduce((list, card) => putCard(identity, list, filter, card), data), filter, counts));
}

/** Puts back on a freshly fetched list the cards and writes in flight it may predate. */
function replaySidebarCards(client: QueryClient, queryKey: QueryKey, cards: SidebarCard[], cardTotals: SidebarChatTotals | undefined) {
  const identity = queryKey[1];
  const filter = queryKey.at(-1) as SidebarFilter;
  cards.forEach((card) => acknowledge(identity, card));
  client.setQueryData<SidebarList>(queryKey, (data) => {
    if (!data) return data;
    let next = cards.reduce((list, card) => putCard(identity, list, filter, card), data);
    for (const id of pendingIn(identity).keys()) {
      const row = next.pages.flatMap((page) => page.sessions).find((session) => session.id === id);
      if (row) next = putSidebarRow(next, filter, id, withPending(identity, row));
    }
    const totals = cardTotals ?? next.pages[0]?.totals;
    return totals ? withCounts(next, filter, withPendingMoves(identity, totals)) : next;
  });
}

/** Joins the workspace's chat writes, so a turn sent after a setting starts with it. */
export const mutPatchChatSession = mutations.define({
  id: "chat/session",
  mutate: (ws: Workspace, session: Pick<ChatSession, "id" | "projectId">, changes: ChatSessionPatch | "stop-and-archive") =>
    inChatTurn("chat/session", ws, session.id, () =>
      changes === "stop-and-archive" ? ws.api.stopAndArchiveChat(session.id) : ws.api.patchChatSession(session.id, changes)),
  optimistic({ helpers, args: [ws, session, changes], onSuccess, onRestore }) {
    holdChatTurn("chat/session", ws, session.id, onRestore);
    const cache = helpers(ws.queryClient);
    const options = ws.queries.listChatSessionsQuery(session.projectId);
    const { agent: _agent, notifyParent, ...fields }: ChatSessionPatch = changes === "stop-and-archive" ? { archived: true } : changes;
    const shown: Partial<ChatSession> = notifyParent === false ? { ...fields, delegate: null } : fields;
    cache.arrayUpdate(options, {
      filter: (row) => row.id === session.id,
      update: (row) => ({ ...row, ...shown }),
    });

    // Only a chat the sidebar lists has a card to move; a side chat or delegate is in the project's list alone.
    const family = ws.queries.listSidebarChatSessionsQuery("all").queryKey.slice(0, -1);
    const known = findSidebarRow(ws.queryClient, family, session.id);
    const movedTo = known && known.archived !== shown.archived ? shown.archived : undefined;
    const write: PendingWrite = { fields: shown, moving: movedTo !== undefined };
    const pending = pendingWrites.get(ws.identity) ?? new Map<string, PendingWrite[]>();
    pendingWrites.set(ws.identity, pending.set(session.id, [...pending.get(session.id) ?? [], write]));
    const settle = () => {
      const rest = pending.get(session.id)?.filter((other) => other !== write) ?? [];
      if (rest.length) pending.set(session.id, rest);
      else pending.delete(session.id);
    };
    if (known) {
      editSidebarLists(ws.queryClient, family, (data, filter) => {
        const placed = putSidebarRow(data, filter, session.id, { ...known, ...shown });
        return movedTo === undefined ? placed : shiftCounts(placed, filter, movedTo);
      });
      onRestore(() => {
        const unshown = write.moving;
        editSidebarLists(ws.queryClient, family, (data, filter) => {
          const placed = putSidebarRow(data, filter, session.id, known);
          return unshown ? shiftCounts(placed, filter, known.archived) : placed;
        });
        // A refused write is rare; reading the lists again settles whatever changed while it was in flight.
        void ws.queryClient.invalidateQueries({ queryKey: family });
      });
    }
    onRestore(settle);

    onSuccess((saved) => {
      settle();
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
  optimistic({ helpers, args: [ws, session], onRestore }) {
    const cache = helpers(ws.queryClient);
    cache.arrayRemove(ws.queries.listChatSessionsQuery(session.projectId), (row) => row.id === session.id);
    const family = ws.queries.listSidebarChatSessionsQuery("all").queryKey.slice(0, -1);
    const known = findSidebarRow(ws.queryClient, family, session.id);
    editSidebarLists(ws.queryClient, family, (data, filter) => putSidebarRow(data, filter, session.id, null));
    onRestore(() => {
      if (known) editSidebarLists(ws.queryClient, family, (data, filter) => putSidebarRow(data, filter, session.id, known));
      // A refused delete is rare; reading the lists again settles whatever changed while it was in flight.
      void ws.queryClient.invalidateQueries({ queryKey: family });
    });
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
  mutate: (ws: Workspace, parentSessionId: string, agent: AgentSelection) => ws.api.openSideChat(parentSessionId, agent),
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

export const mutStopBackgroundTask = mutations.define({
  id: "chat/background/stop",
  mutate: (ws: Workspace, sessionId: string, taskId: string) => ws.api.stopBackgroundTask(sessionId, taskId),
  optimistic: () => {},
  key: ({ args: [ws, sessionId, taskId] }) => [ws.identity, sessionId, taskId],
  describe: "stop the background task",
  describeResult: null,
});

export const mutSetChatLimitTimer = mutations.define({
  id: "chat/limit-timer",
  mutate: (ws: Workspace, sessionId: string, stopped: boolean) => ws.api.setChatLimitTimer(sessionId, stopped),
  optimistic: () => {},
  key: ({ args: [ws, sessionId] }) => [ws.identity, sessionId],
  describe: "change the usage reset timer",
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

/** The server clears the failure at once and updates once the gap since the last update passes. */
export const mutRefreshChatReport = mutations.define({
  id: "chat/report/refresh",
  mutate: (ws: Workspace, sessionId: string) => ws.api.refreshChatReport(sessionId),
  optimistic({ helpers, args: [ws, sessionId] }) {
    helpers(ws.queryClient).updateExisting(ws.queries.getChatReportQuery(sessionId), (report) => report && { ...report, error: null });
  },
  key: ({ args: [ws, sessionId] }) => [ws.identity, sessionId],
  describe: "update the status",
  describeResult: null,
});

/** Turning it off hides the card at once; turning it on shows it when the server's reply or event lands. */
export const mutSetChatReportEnabled = mutations.define({
  id: "chat/report/enabled",
  mutate: (ws: Workspace, sessionId: string, enabled: boolean) => ws.api.setChatReportEnabled(sessionId, enabled),
  optimistic({ helpers, args: [ws, sessionId, enabled] }) {
    const query = ws.queries.getChatReportQuery(sessionId);
    helpers(ws.queryClient).updateExisting(query, (report) => report && { ...report, enabled });
    helpers(ws.queryClient).refetchOnSettled(query);
  },
  key: ({ args: [ws, sessionId] }) => [ws.identity, sessionId],
  describe: "change the chat's status",
  describeResult: null,
});

/** The status starts writing as soon as a model is picked; the server's event fills it. */
export const mutChooseChatReportModel = mutations.define({
  id: "chat/report/model",
  mutate: (ws: Workspace, sessionId: string, choice: ChatReportChoice) => ws.api.chooseChatReportModel(sessionId, choice),
  optimistic({ helpers, args: [ws, sessionId] }) {
    helpers(ws.queryClient).updateExisting(ws.queries.getChatReportQuery(sessionId), (report) => report && { ...report, needsModel: null, writing: true, error: null });
  },
  key: ({ args: [ws, sessionId] }) => [ws.identity, sessionId],
  describe: "choose the status's model",
  describeResult: null,
});
