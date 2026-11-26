import { useWorkspace } from "../workspace";
import type { ChatSession } from "../api";
import type { RunMonitor, RunMonitorEvent } from "../runMonitor/types";
import { markLiveUpdate } from "./live";

import { inputSummaryChanged } from "./chatState";
import { useEffect, useMemo } from "react";

import { artifactFamilies, streamsSidebarRows } from "./invalidation";
import { applySidebarCards, editSidebarLists } from "./chat";

/** The cached monitor with `event` applied, or null when the event can't be applied to it and the run must be read again. */
export function applyMonitorEvent(cached: RunMonitor, event: RunMonitorEvent): RunMonitor | null {
  if (event.reset || event.generation !== cached.generation) return null;
  if (event.rev <= cached.rev) return cached;
  const { appendFrom, append, appendRows, dropRows, ...body } = event;
  const appends = Object.entries(append);
  const rowAppends = Object.entries(appendRows);
  const rowDrops = Object.entries(dropRows);
  const grew = appends.length > 0 || rowAppends.length > 0 || rowDrops.length > 0;
  if ((grew ? appendFrom : body.seriesRev) !== cached.seriesRev) return null;
  const series = { ...cached.series };
  for (const [name, points] of appends) series[name] = [...(series[name] ?? []), ...points];
  const rows = { ...cached.rows };
  for (const [name, left] of rowDrops) if (rows[name]) rows[name] = rows[name].slice(left);
  for (const [name, added] of rowAppends) rows[name] = [...(rows[name] ?? []), ...added];
  return { ...body, view: cached.view, parent: cached.parent, series, rows };
}

function upsert<T extends { id: string; updatedAt: number }>(rows: T[] | undefined, next: T) {
  if (!rows) return undefined;
  const old = rows.find((row) => row.id === next.id);
  if (old && old.updatedAt > next.updatedAt) return rows;
  return old ? rows.map((row) => row.id === next.id ? next : row) : [...rows, next];
}

export function QueryEvents() {
  const { workspaceScope, isCurrentScope, queryClient, deletedSessionIds } = useWorkspace();
  const { invalidateFamilies, repairLiveQueries, removeSession } = useWorkspace().invalidation;
  const {
    useOrxEventStream,
    emitEntity,
    onProjectActivityEvent,
    onDataDirMove,
    onChatEvent,
    onHarnessAuth,
    onHarnessCatalog,
    onUpdateStatus,
  } = useWorkspace().events;
  const {
    listRunsQuery,
    listExperimentsQuery,
    listProjectsQuery,
    listChatSessionsQuery,
    listChatShownRunsQuery,
    getRunMonitorQuery,
    getChatReportQuery,
    refreshHarnesses,
    refreshHarness,
    getHarnessesQuery,
    getUpdateStatusQuery,
  } = useWorkspace().queries;
  const { dispatchChat } = useWorkspace().chat;

  const scope = workspaceScope();
  const sessionExperiments = useMemo(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    return {
      invalidate() {
        timer ??= setTimeout(() => {
          timer = undefined;
          invalidateFamilies(["getSessionExperiments"], scope);
        }, 200);
      },
      dispose() { clearTimeout(timer); timer = undefined; },
    };
  }, [scope[1]]);
  useEffect(() => () => sessionExperiments.dispose(), [sessionExperiments]);
  useOrxEventStream({
    onRun(run) {
      if (!isCurrentScope(scope)) return;
      sessionExperiments.invalidate();
      const old = queryClient.getQueryData(listRunsQuery(run.projectId).queryKey)?.find((row) => row.id === run.id);
      markLiveUpdate(queryClient, listRunsQuery(run.projectId).queryKey, run.id);
      queryClient.setQueryData(listRunsQuery(run.projectId).queryKey, (rows) => upsert(rows, run));
      markLiveUpdate(queryClient, listChatShownRunsQuery().queryKey, run.id);
      queryClient.setQueryData(listChatShownRunsQuery().queryKey, (shown) => shown && {
        ...shown,
        runs: shown.runs.map((item) => item.run.id === run.id && item.run.updatedAt <= run.updatedAt ? { ...item, run } : item),
      });
      if (old?.commitSha !== run.commitSha) {
        invalidateFamilies(["getRunDiff"], scope, (query) => query.queryKey[3] === run.id);
        invalidateFamilies(["getExperimentDiff"], scope, (query) => query.queryKey[3] === run.experimentId);
      }
      emitEntity.onRun(run);
    },
    onRunMonitor(event) {
      if (!isCurrentScope(scope)) return;
      // A look newer than its chat's summary may have written a new one, which rides on the shown runs.
      const looked = event.watch?.looks.filter((look) => look.said !== "error" && look.said !== "interrupted").at(-1)?.id;
      const shown = queryClient.getQueryData(listChatShownRunsQuery().queryKey)?.runs.find((item) => item.run.id === event.runId);
      if (looked !== undefined && shown && Number(looked) > Number(shown.watch?.lookId ?? 0)) {
        void queryClient.invalidateQueries({ queryKey: listChatShownRunsQuery().queryKey, exact: true });
      }
      const { queryKey } = getRunMonitorQuery(event.runId);
      markLiveUpdate(queryClient, queryKey);
      const cached = queryClient.getQueryData(queryKey);
      if (!cached) return;
      const next = applyMonitorEvent(cached, event);
      if (next) queryClient.setQueryData(queryKey, next);
      else void queryClient.invalidateQueries({ queryKey, exact: true });
    },
    onExperiment(experiment) {
      if (!isCurrentScope(scope)) return;
      sessionExperiments.invalidate();
      markLiveUpdate(queryClient, listExperimentsQuery(experiment.projectId).queryKey, experiment.id);
      queryClient.setQueryData(listExperimentsQuery(experiment.projectId).queryKey, (rows) => upsert(rows, experiment));
      invalidateFamilies(["getProjectStarterPrompts"], scope, (query) => query.queryKey[3] === experiment.projectId);
      invalidateFamilies(["getExperimentDiff"], scope, (query) => query.queryKey[3] === experiment.id);
      invalidateFamilies(["getRunDiff"], scope, (query) => queryClient.getQueryData(listRunsQuery(experiment.projectId).queryKey)?.some((run) => run.experimentId === experiment.id && run.id === query.queryKey[3]) ?? false);
    },
    onProject(project) {
      if (!isCurrentScope(scope)) return;
      markLiveUpdate(queryClient, listProjectsQuery().queryKey, project.id);
      queryClient.setQueryData(listProjectsQuery().queryKey, (rows) => upsert(rows, project));
    },
    onArtifacts(projectId) {
      if (!isCurrentScope(scope)) return;
      invalidateFamilies(artifactFamilies, scope, (query) => query.queryKey[3] === projectId);
      invalidateFamilies(["resolvedFile"], scope, (query) => query.queryKey[3] === projectId && query.queryKey[5] === "artifacts");
      const projectRuns = queryClient.getQueryData(listRunsQuery(projectId).queryKey) ?? [];
      const shownRuns = queryClient.getQueryData(listChatShownRunsQuery().queryKey)?.runs ?? [];
      const runIds = new Set(projectRuns.map((run) => run.id));
      for (const { run } of shownRuns) if (run.projectId === projectId) runIds.add(run.id);
      for (const id of runIds) {
        const { queryKey } = getRunMonitorQuery(id);
        const state = queryClient.getQueryState(queryKey);
        const media = state?.data?.media ?? {};
        if (state?.fetchStatus !== "fetching" && !Object.values(media).some((gallery) => gallery.error?.kind !== "unsupported")) continue;
        markLiveUpdate(queryClient, queryKey);
        void queryClient.invalidateQueries({ queryKey, exact: true });
      }
    },
    onUsageChanged() {
      invalidateFamilies(["getUsage"], scope);
    },
    onReconnect() {
      if (!isCurrentScope(scope)) return;
      void repairLiveQueries();
      emitEntity.onReconnect();
    },
  });
  useEffect(() => {
    let activityTimer: ReturnType<typeof setTimeout> | undefined;
    let sidebarTimer: ReturnType<typeof setTimeout> | undefined;
    const promptStates = new Map<string, string>();
    // A server that streams sidebar cards keeps the sidebar's lists current itself.
    const chatLists = () => streamsSidebarRows(queryClient, scope) ? ["listChatSessions"] : ["listSidebarChatSessions", "listChatSessions"];
    const refreshSidebar = () => {
      sidebarTimer ??= setTimeout(() => {
        sidebarTimer = undefined;
        invalidateFamilies(chatLists(), scope);
      }, 100);
    };
    const refreshActivity = () => {
      activityTimer ??= setTimeout(() => {
        activityTimer = undefined;
        invalidateFamilies(["listProjectActivity"], scope);
      }, 100);
    };
    const offActivity = onProjectActivityEvent(refreshActivity);
    const offMove = onDataDirMove((event) => { if (event.type === "done") invalidateFamilies(["getDataDir"], scope); });
    const offChat = onChatEvent((event) => {
      if (!isCurrentScope(scope)) return;
      if (event.type === "sidebar") applySidebarCards(queryClient, scope, event);
      if (inputSummaryChanged(event, promptStates)) refreshSidebar();
      else if (event.type === "busy" && !streamsSidebarRows(queryClient, scope)) invalidateFamilies(["listSidebarChatSessions"], scope);
      if (event.type === "background") invalidateFamilies(chatLists(), scope);
      if (event.type === "branch" || event.type === "sessionDeleted" || event.type === "busy" && !event.busy) invalidateFamilies(["chatSearch"], scope);
      if (event.type === "session") markLiveUpdate(queryClient, listChatSessionsQuery(event.session.projectId).queryKey, event.session.id);
      else if (event.type === "busy" || event.type === "usage") markLiveUpdate(queryClient, [...scope, "listChatSessions"], event.sessionId);
      if (event.type === "message" || event.type === "queued" || event.type === "branch") {
        dispatchChat("", event.type === "message"
          ? { type: "upsertMessage", sessionId: event.sessionId, message: event.message }
          : event.type === "queued" ? { type: "setQueued", sessionId: event.sessionId, items: event.items }
            : { type: "activeLeaf", sessionId: event.sessionId, leafId: event.activeLeafId }, true);
      }
      if (event.type === "report") {
        const { queryKey } = getChatReportQuery(event.sessionId);
        markLiveUpdate(queryClient, queryKey);
        queryClient.setQueryData(queryKey, (report) => report === undefined ? undefined : event.report);
      }
      if (event.type === "session" && !deletedSessionIds.has(event.session.id)) {
        // A chat that moved into another project leaves its old project's list.
        for (const [key, rows] of queryClient.getQueriesData<ChatSession[]>({ queryKey: [...scope, "listChatSessions"] })) {
          if (key[3] !== event.session.projectId && rows?.some((row) => row.id === event.session.id)) {
            queryClient.setQueryData(key, rows.filter((row) => row.id !== event.session.id));
          }
        }
        queryClient.setQueryData(listChatSessionsQuery(event.session.projectId).queryKey, (rows) => {
          if (!rows) return undefined;
          const old = rows.find((row) => row.id === event.session.id);
          if (!old) return [event.session, ...rows];
          return upsert(rows, { ...event.session, contextUsage: event.session.contextUsage ?? old.contextUsage, awaitingInput: event.session.awaitingInput ?? old.awaitingInput });
        });
      } else if (event.type === "sessionDeleted") {
        removeSession(event.sessionId);
      } else if (event.type === "busy" || event.type === "usage") {
        const update = (row: ChatSession) => row.id !== event.sessionId ? row : event.type === "busy" ? { ...row, busy: event.busy } : { ...row, contextUsage: event.usage ?? undefined };
        queryClient.setQueriesData<ChatSession[]>({ queryKey: [...scope, "listChatSessions"] }, (rows) => rows?.map(update));
        editSidebarLists(queryClient, [...scope, "listSidebarChatSessions"], (data) => ({
          ...data, pages: data.pages.map((page) => ({ ...page, sessions: page.sessions.map(update) })),
        }));
      }
    });
    const offAuth = onHarnessAuth(({ harness }) => {
      if (!isCurrentScope(scope)) return;
      // Re-probe only the harness whose sign-in changed; a full sweep waits on the slowest CLI.
      void (harness === "*" ? refreshHarnesses(true) : refreshHarness(harness)).catch(() => {});
    });
    // The catalog fill already ran server-side; a plain read serves the
    // updated cache — forcing a refresh here would spawn a second sweep. The
    // query's 5-minute staleTime would swallow a bare fetchQuery, so bypass it.
    const offCatalog = onHarnessCatalog(() => { if (isCurrentScope(scope)) void queryClient.fetchQuery({ ...getHarnessesQuery(), staleTime: 0 }).catch(() => {}); });
    const offUpdate = onUpdateStatus((status) => {
      if (!isCurrentScope(scope)) return;
      markLiveUpdate(queryClient, getUpdateStatusQuery().queryKey);
      queryClient.setQueryData(getUpdateStatusQuery().queryKey, status);
    });
    return () => { clearTimeout(activityTimer); clearTimeout(sidebarTimer); offActivity(); offMove(); offChat(); offAuth(); offCatalog(); offUpdate(); };
  }, [scope[1]]);
  return null;
}
