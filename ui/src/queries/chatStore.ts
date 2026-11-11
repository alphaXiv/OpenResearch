import type { WorkspaceQueries } from "../workspace";
import type { WorkspaceClient } from "./client";
import { useCallback, useMemo, useSyncExternalStore } from "react";
import { useQuery, type QueryCacheNotifyEvent, type QueryClient } from "@tanstack/react-query";

import { markLiveUpdate } from "./live";
import { reducer, type Action, type ChatState } from "./chatState";

type Snapshot = Awaited<ReturnType<import("../api").Api["getChatMessages"]>>;

function isChatDataEvent(event: QueryCacheNotifyEvent) {
  return (
    (event.query.queryKey[2] === "listChatSessions" ||
      event.query.queryKey[2] === "getChatMessages") &&
    (event.type === "removed" ||
      (event.type === "updated" && event.action.type === "success"))
  );
}

/** Counts chat data changes on every machine sharing `queryClient`; one subscription per cache, so a dropped machine's store is not kept alive by it. */
const chatRevisions = new WeakMap<QueryClient, { value: number }>();
function chatRevision(queryClient: QueryClient) {
  const existing = chatRevisions.get(queryClient);
  if (existing) return existing;
  const revision = { value: 0 };
  queryClient.getQueryCache().subscribe((event) => { if (isChatDataEvent(event)) revision.value++; });
  chatRevisions.set(queryClient, revision);
  return revision;
}

export function createChatStore(
  client: WorkspaceClient,
  queries: WorkspaceQueries,
) {
  const { queryClient, workspaceScope, isCurrentScope, deletedSessionIds } =
    client;
  const { getChatMessagesQuery, listChatSessionsQuery } = queries;

  const revision = chatRevision(queryClient);
  const subscribeChat = (notify: () => void) => queryClient.getQueryCache().subscribe((event) => { if (isChatDataEvent(event) && isCurrentScope(event.query.queryKey)) notify(); });
  const getRevision = () => revision.value;

  let writeTail: Promise<unknown> = Promise.resolve();
  /** Holds the next place among session writes and sends; `run` fills it, `release` gives it up. One must be called. */
  function reserve() {
    let fill!: (write: (() => Promise<unknown>) | null) => void;
    const slot = new Promise<(() => Promise<unknown>) | null>((resolve) => { fill = resolve; });
    const result = writeTail.catch(() => {}).then(() => slot).then((write) => {
      if (!write) return;
      if (!isCurrentScope(workspaceScope())) throw new DOMException("Workspace changed", "AbortError");
      return write();
    });
    writeTail = result.catch(() => {});
    return {
      run<T>(write: () => Promise<T>) {
        fill(write);
        return result as Promise<T>;
      },
      release: () => fill(null),
    };
  }

  /** Runs session writes and sends one at a time, so a turn starts with the settings changed before it. */
  const serialize = <T>(write: () => Promise<T>) => reserve().run(write);

  function readChatState(projectId: string, activeId: string | null): ChatState {
    const sessions = queryClient.getQueryData(listChatSessionsQuery(projectId).queryKey) ?? [];
    const busySessions = new Set(sessions.filter((session) => session.busy).map((session) => session.id));
    const state: ChatState = { messagesBySession: {}, queuedBySession: {}, activeLeafBySession: {}, busySessions };
    const visible = new Set(sessions.map((session) => session.id));
    if (activeId) visible.add(activeId);
    for (const id of visible) {
      const snapshot = queryClient.getQueryData(getChatMessagesQuery(id).queryKey);
      if (!snapshot) continue;
      state.messagesBySession[id] = snapshot.messages;
      state.queuedBySession[id] = snapshot.queued;
      state.activeLeafBySession[id] = snapshot.activeLeafId;
    }
    return state;
  }

  function dispatchChat(projectId: string, action: Action, existingOnly = false) {
    if (action.type === "busy") {
      markLiveUpdate(queryClient, listChatSessionsQuery(projectId).queryKey, action.sessionId);
      queryClient.setQueryData(listChatSessionsQuery(projectId).queryKey, (rows) => rows?.map((row) => row.id === action.sessionId ? { ...row, busy: action.busy } : row));
      return;
    }
    const options = getChatMessagesQuery(action.sessionId);
    if (deletedSessionIds.has(action.sessionId)) return;
    markLiveUpdate(queryClient, options.queryKey, action.type === "upsertMessage" ? `message:${action.message.id}` : action.type === "setQueued" ? "queued" : action.type === "activeLeaf" ? "branch" : "local");
    const previous = queryClient.getQueryData(options.queryKey);
    if (existingOnly && !previous) return;
    const state: ChatState = {
      messagesBySession: previous ? { [action.sessionId]: previous.messages } : {},
      queuedBySession: previous ? { [action.sessionId]: previous.queued } : {},
      activeLeafBySession: previous ? { [action.sessionId]: previous.activeLeafId } : {},
      busySessions: new Set(),
    };
    const next = reducer(state, action);
    const snapshot: Snapshot = {
      messages: next.messagesBySession[action.sessionId] ?? [],
      queued: next.queuedBySession[action.sessionId] ?? [],
      activeLeafId: next.activeLeafBySession[action.sessionId] ?? null,
    };
    queryClient.setQueryData(options.queryKey, snapshot);
  }

  function useChatState(projectId: string, activeId: string | null) {
    const enabled = Boolean(activeId) && !deletedSessionIds.has(activeId ?? "");
    const history = useQuery({ ...getChatMessagesQuery(activeId ?? ""), enabled, subscribed: enabled });
    const dataRevision = useSyncExternalStore(subscribeChat, getRevision);
    const state = useMemo(() => readChatState(projectId, activeId), [projectId, activeId, dataRevision, history.data]);
    const generation = workspaceScope()[1];
    const dispatch = useCallback((action: Action) => {
      if (isCurrentScope(["workspace", generation])) dispatchChat(projectId, action);
    }, [projectId, generation]);
    return [state, dispatch, history] as const;
  }

  return {
    reserve,
    serialize,
    subscribeChat,
    readChatState,
    dispatchChat,
    useChatState,
  };
}
