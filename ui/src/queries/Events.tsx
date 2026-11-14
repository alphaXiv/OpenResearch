import { useWorkspace } from "../workspace";
import type { InfiniteData } from "@tanstack/react-query";
import type { ChatSession, SidebarChatPage } from "../api";
import type { RunMonitor, RunMonitorEvent } from "../runMonitor/types";
import { markLiveUpdate } from "./live";

import { inputSummaryChanged } from "./chatState";
import { useEffect, useMemo } from "react";

import { artifactFamilies } from "./invalidation";

/** The cached monitor with `event` applied, or null when the event can't be applied to it and the run must be read again. */
export function applyMonitorEvent(cached: RunMonitor, event: RunMonitorEvent): RunMonitor | null {
  if (event.reset || event.generation !== cached.generation) return null;
  if (event.rev <= cached.rev) return cached;
  const appends = Object.entries(event.append ?? {}).filter(([, points]) => points.length > 0);
  const rowAppends = Object.entries(event.appendRows ?? {}).filter(([, rows]) => rows.length > 0);
  // Past the fold's row cap a table's oldest rows leave as new ones come.
  const rowDrops = Object.entries(event.dropRows ?? {}).filter(([, left]) => left > 0);
  const grew = appends.length > 0 || rowAppends.length > 0 || rowDrops.length > 0;
  if (grew && event.appendFrom !== cached.seriesRev) return null;
  if (!grew && event.seriesRev !== cached.seriesRev) return null;
  const base = { ...cached, ...event.patch };
  const series = { ...base.series };
  for (const [name, points] of appends) series[name] = [...(series[name] ?? []), ...points];
  const rows = { ...base.rows };
  for (const [name, left] of rowDrops) if (rows[name]) rows[name] = rows[name].slice(left);
  for (const [name, added] of rowAppends) rows[name] = [...(rows[name] ?? []), ...added];
  return {
    ...base,
    rev: event.rev,
    asOf: event.asOf,
    seriesRev: event.seriesRev,
    series,
    rows,
    latest: event.latest,
    counts: event.counts,
    progress: event.progress,
    estimate: event.estimate,
    cost: event.cost,
    sentence: event.sentence,
    ...(event.watch === undefined ? {} : { watch: event.watch }),
    ...(event.flag === undefined ? {} : { flag: event.flag }),
  };
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
    const refreshSidebar = () => {
      sidebarTimer ??= setTimeout(() => {
        sidebarTimer = undefined;
        invalidateFamilies(["listSidebarChatSessions", "listChatSessions"], scope);
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
      if (inputSummaryChanged(event, promptStates)) refreshSidebar();
      if (event.type === "branch" || event.type === "sessionDeleted" || event.type === "busy" && !event.busy) invalidateFamilies(["chatSearch"], scope);
      if (event.type === "session") markLiveUpdate(queryClient, listChatSessionsQuery(event.session.projectId).queryKey, event.session.id);
      else if (event.type === "busy" || event.type === "usage") markLiveUpdate(queryClient, [...scope, "listChatSessions"], event.sessionId);
      if (event.type === "message" || event.type === "queued" || event.type === "branch") {
        dispatchChat("", event.type === "message"
          ? { type: "upsertMessage", sessionId: event.sessionId, message: event.message }
          : event.type === "queued" ? { type: "setQueued", sessionId: event.sessionId, items: event.items }
            : { type: "activeLeaf", sessionId: event.sessionId, leafId: event.activeLeafId }, true);
      }
      if (event.type === "session" && !deletedSessionIds.has(event.session.id)) {
        queryClient.setQueryData(listChatSessionsQuery(event.session.projectId).queryKey, (rows) => {
          if (!rows) return undefined;
          const old = rows.find((row) => row.id === event.session.id);
          if (!old) return [event.session, ...rows];
          return upsert(rows, { ...event.session, contextUsage: event.session.contextUsage ?? old.contextUsage, awaitingInput: event.session.awaitingInput ?? old.awaitingInput });
        });
      } else if (event.type === "sessionDeleted") {
        removeSession(event.sessionId);
      } else if (event.type === "busy" || event.type === "usage") {
        const update = (row: ChatSession) => row.id !== event.sessionId ? row : event.type === "busy" ? { ...row, busy: event.busy } : { ...row, contextUsage: event.usage };
        queryClient.setQueriesData<ChatSession[]>({ queryKey: [...scope, "listChatSessions"] }, (rows) => rows?.map(update));
        queryClient.setQueriesData<InfiniteData<SidebarChatPage>>({ queryKey: [...scope, "listSidebarChatSessions"] }, (data) => data && {
          ...data, pages: data.pages.map((page) => ({ ...page, sessions: page.sessions.map(update) })),
        });
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
