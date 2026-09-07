import { useCallback, useMemo, useSyncExternalStore } from "react";
import { useQuery, type QueryCacheNotifyEvent } from "@tanstack/react-query";
import { queryClient, workspaceScope, isCurrentScope, deletedSessionIds } from "./client";
import { getChatMessagesQuery, listChatSessionsQuery } from "./chat";
import { markLiveUpdate } from "./live";
import { reducer, type Action, type ChatState } from "./chatState";

type Snapshot = Awaited<ReturnType<typeof import("../api").getChatMessages>>;
let revision = 0;
export function isChatDataEvent(event: QueryCacheNotifyEvent) {
  return (event.query.queryKey[2] === "listChatSessions" || event.query.queryKey[2] === "getChatMessages")
    && (event.type === "removed" || (event.type === "updated" && event.action.type === "success"));
}
queryClient.getQueryCache().subscribe((event) => { if (isChatDataEvent(event)) revision++; });
export const subscribeChat = (notify: () => void) => queryClient.getQueryCache().subscribe((event) => { if (isChatDataEvent(event)) notify(); });
const getRevision = () => revision;

export function readChatState(projectId: string, activeId: string | null): ChatState {
  const sessions = queryClient.getQueryData(listChatSessionsQuery(projectId).queryKey) ?? [];
  const busySessions = new Set(sessions.filter((session) => session.busy).map((session) => session.id));
  const state: ChatState = { messagesBySession: {}, queuedBySession: {}, activeLeafBySession: {}, busySessions };
  const visible = new Set(busySessions);
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

export function dispatchChat(projectId: string, action: Action, existingOnly = false) {
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

export function useChatState(projectId: string, activeId: string | null) {
  const enabled = Boolean(activeId) && !deletedSessionIds.has(activeId ?? "");
  const history = useQuery({ ...getChatMessagesQuery(activeId ?? ""), enabled, subscribed: enabled });
  const dataRevision = useSyncExternalStore(subscribeChat, getRevision);
  const state = useMemo(() => readChatState(projectId, activeId), [projectId, activeId, dataRevision]);
  const generation = workspaceScope()[1];
  const dispatch = useCallback((action: Action) => {
    if (isCurrentScope(["workspace", generation])) dispatchChat(projectId, action);
  }, [projectId, generation]);
  return [state, dispatch, history] as const;
}
