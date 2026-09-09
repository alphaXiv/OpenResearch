import type { ChatSession } from "../api";
import { markLiveUpdate } from "./live";
import {
  refreshHarnesses,
  getUpdateStatusQuery,
} from "./settings";
import { dispatchChat } from "./chatStore";
import { useEffect } from "react";
import {
  emitEntity,
  onDataDirMove,
  onProjectActivityEvent,
  onChatEvent,
  onHarnessAuth,
  onUpdateStatus,
  useOrxEventStream,
} from "../events";
import { queryClient, workspaceScope, isCurrentScope, deletedSessionIds } from "./client";
import { listProjectsQuery, listRunsQuery, listExperimentsQuery } from "./projects";
import { listChatSessionsQuery } from "./chat";

import { artifactFamilies, liveFamilies, invalidateFamilies, removeSession } from "./invalidation";

function upsert<T extends { id: string; updatedAt: number }>(rows: T[] | undefined, next: T) {
  if (!rows) return undefined;
  const old = rows.find((row) => row.id === next.id);
  if (old && old.updatedAt > next.updatedAt) return rows;
  return old ? rows.map((row) => row.id === next.id ? next : row) : [...rows, next];
}

export function QueryEvents() {
  const scope = workspaceScope();
  useOrxEventStream({
    onRun(run) {
      if (!isCurrentScope(scope)) return;
      const old = queryClient.getQueryData(listRunsQuery(run.projectId).queryKey)?.find((row) => row.id === run.id);
      markLiveUpdate(queryClient, listRunsQuery(run.projectId).queryKey, run.id);
      queryClient.setQueryData(listRunsQuery(run.projectId).queryKey, (rows) => upsert(rows, run));
      if (old?.commitSha !== run.commitSha) {
        invalidateFamilies(["getRunDiff"], scope, (query) => query.queryKey[3] === run.id);
        invalidateFamilies(["getExperimentDiff"], scope, (query) => query.queryKey[3] === run.experimentId);
      }
      emitEntity.onRun(run);
    },
    onExperiment(experiment) {
      if (!isCurrentScope(scope)) return;
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
    onReconnect() {
      if (!isCurrentScope(scope)) return;
      invalidateFamilies(liveFamilies, scope);
      emitEntity.onReconnect();
    },
  });
  useEffect(() => {
    let activityTimer: ReturnType<typeof setTimeout> | undefined;
    const offActivity = onProjectActivityEvent(() => {
      activityTimer ??= setTimeout(() => {
        activityTimer = undefined;
        invalidateFamilies(["listProjectActivity"], scope);
      }, 100);
    });
    const offMove = onDataDirMove((event) => { if (event.type === "done") invalidateFamilies(["getDataDir"], scope); });
    const offChat = onChatEvent((event) => {
      if (!isCurrentScope(scope)) return;
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
          return upsert(rows, { ...event.session, contextUsage: event.session.contextUsage ?? old.contextUsage });
        });
      } else if (event.type === "sessionDeleted") {
        removeSession(event.sessionId);
      } else if (event.type === "busy" || event.type === "usage") {
        queryClient.setQueriesData<ChatSession[]>({ queryKey: [...scope, "listChatSessions"] }, (rows) => rows?.map((row) => row.id !== event.sessionId ? row : event.type === "busy" ? { ...row, busy: event.busy } : { ...row, contextUsage: event.usage }));
      }
    });
    const offAuth = onHarnessAuth(() => { if (isCurrentScope(scope)) void refreshHarnesses(true).catch(() => {}); });
    const offUpdate = onUpdateStatus((status) => {
      if (!isCurrentScope(scope)) return;
      markLiveUpdate(queryClient, getUpdateStatusQuery().queryKey);
      queryClient.setQueryData(getUpdateStatusQuery().queryKey, status);
    });
    return () => { clearTimeout(activityTimer); offActivity(); offMove(); offChat(); offAuth(); offUpdate(); };
  }, [scope[1]]);
  return null;
}
