import type { Workspace } from "./workspace";

import { isDemoProjectId, type ChatSession } from "./api";
import { defaultTaskWorkspace } from "./workspaceTabs";
import { getTaskWorkspace, parseDestination, safeLocation, taskLocation } from "./workspaceState";

function validSessionLocation(location: string, sessions: ChatSession[]): boolean {
  const destination = parseDestination(location.split("?")[0]);
  return Boolean(destination && (!destination.sessionId || sessions.some((session) =>
    session.id === destination.sessionId && session.projectId === destination.projectId)));
}

export async function globalResumeLocation(workspace: Workspace): Promise<string> {
  const { queryClient: client } = workspace;
  const { getUiStateQuery, listProjectsQuery, listChatSessionsQuery } = workspace.queries;
  const { getRememberedGlobalWorkspace } = workspace.persistence;
  const [state, projects] = await Promise.all([client.fetchQuery(getUiStateQuery()), client.fetchQuery(listProjectsQuery())]);
  const location = safeLocation((getRememberedGlobalWorkspace() ?? state.workspace)?.lastLocation);
  const fallback = () => projects[0] ? projectResumeLocation(projects[0].id, workspace) : Promise.resolve("/projects");
  if (!location) return fallback();
  const destination = parseDestination(location.split("?")[0]);
  if (!destination?.projectId) return fallback();
  if (!projects.some((project) => project.id === destination.projectId)) return fallback();
  if (destination.sessionId && !validSessionLocation(location, await client.fetchQuery(listChatSessionsQuery(destination.projectId))))
    return fallback();
  return location;
}

export async function projectResumeLocation(projectId: string, workspace: Workspace): Promise<string> {
  const { queryClient: client } = workspace;
  const { getProjectUiStateQuery, listChatSessionsQuery, getUiStateQuery } = workspace.queries;
  const { getCachedProjectWorkspace } = workspace.projects;
  const [loaded, sessions] = await Promise.all([client.fetchQuery(getProjectUiStateQuery(projectId)), client.fetchQuery(listChatSessionsQuery(projectId))]);
  const state = getCachedProjectWorkspace(projectId) ?? loaded;
  const location = safeLocation(state?.lastLocation);
  if (location && parseDestination(location.split("?")[0])?.projectId === projectId
    && validSessionLocation(location, sessions)) return location;
  const newest = sessions.find((session) => !session.archived && !session.sideParentSessionId);
  const defaults = isDemoProjectId(projectId) ? defaultTaskWorkspace(newest?.id, !(await client.fetchQuery(getUiStateQuery())).tourCompleted) : undefined;
  const task = getTaskWorkspace(state, newest?.id ?? "new");
  const rememberedPane = task ? task.active : defaults?.active;
  return taskLocation(projectId, newest?.id ?? null, rememberedPane);
}
