import { type SetStateAction, useCallback, useMemo } from "react";
import { useQuery } from "@tanstack/react-query";

import { setScopedQueryData } from "./queries/client";
import { listChatSessionsQuery } from "./queries/chat";
import { listProjectsQuery, getUiStateQuery } from "./queries/projects";
import { type Project, type UiState } from "./api";
import { m } from "./paraglide/messages.js";

/** Loads the top-level, project-scoped data App needs before it can render:
 * the project list, ui state, and the current project's chat sessions — plus
 * a combined startup error and a retry that re-fetches all three. */
export function useAppData(projectId: string, locale: string) {
  const sessionsQuery = useQuery(listChatSessionsQuery(projectId));
  const sessions = useMemo(() => sessionsQuery.data?.map((session) => session.id) ?? null, [sessionsQuery.data]);

  const projectsOptions = useMemo(() => listProjectsQuery(), [projectId]);
  const projectsQuery = useQuery(projectsOptions);
  const projects = projectsQuery.data ?? null;
  const setProjects = useCallback((value: SetStateAction<Project[] | null>) => {
    setScopedQueryData(projectsOptions.queryKey, (current) => {
      const next = typeof value === "function" ? value(current ?? null) : value;
      return next ?? undefined;
    });
  }, [projectsOptions]);

  const uiStateOptions = useMemo(() => getUiStateQuery(), [projectId]);
  const uiStateQuery = useQuery(uiStateOptions);
  const uiState = uiStateQuery.data ?? null;
  const setUiState = useCallback((value: SetStateAction<UiState | null>) => {
    setScopedQueryData(uiStateOptions.queryKey, (current) => {
      const next = typeof value === "function" ? value(current ?? null) : value;
      return next ?? undefined;
    });
  }, [uiStateOptions]);

  const failedStartupItems = [
    !sessionsQuery.data && sessionsQuery.error ? m.chat_all_sessions() : null,
    !projectsQuery.data && projectsQuery.error ? m.app_projects() : null,
    !uiStateQuery.data && uiStateQuery.error ? m.app_settings() : null,
  ].filter((item) => item !== null);
  const startupError = failedStartupItems.length
    ? m.app_startup_load_failed({ items: new Intl.ListFormat(locale).format(failedStartupItems) })
    : null;

  const loadInitialState = () => {
    void projectsQuery.refetch();
    void uiStateQuery.refetch();
    void sessionsQuery.refetch();
  };

  return {
    sessionsQuery, sessions,
    projectsQuery, projects, setProjects,
    uiStateQuery, uiState, setUiState,
    startupError, loadInitialState,
  };
}
