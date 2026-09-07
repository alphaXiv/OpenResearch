import { getProjectUiState, getUiState, isDemoProjectId, listChatSessions, listProjects, type ChatSession } from "./api";
import { defaultTaskWorkspace } from "./workspaceTabs";
import { getTaskWorkspace, parseDestination, safeLocation, taskLocation } from "./workspaceState";
import { getRememberedGlobalWorkspace } from "./workspacePersistence";
import { getCachedProjectWorkspace } from "./useProjectWorkspace";

function validSessionLocation(location: string, sessions: ChatSession[]): boolean {
  const destination = parseDestination(location.split("?")[0]);
  return Boolean(destination && (!destination.sessionId || sessions.some((session) =>
    session.id === destination.sessionId && session.projectId === destination.projectId)));
}

export async function globalResumeLocation(): Promise<string> {
  const [state, projects] = await Promise.all([getUiState(), listProjects()]);
  const location = safeLocation((getRememberedGlobalWorkspace() ?? state.workspace)?.lastLocation);
  if (!location) return "/projects";
  const destination = parseDestination(location.split("?")[0]);
  if (!destination?.projectId) return "/projects";
  if (!projects.some((project) => project.id === destination.projectId)) return "/projects";
  if (destination.sessionId && !validSessionLocation(location, await listChatSessions(destination.projectId)))
    return "/projects";
  return location;
}

export async function projectResumeLocation(projectId: string): Promise<string> {
  const [loaded, sessions] = await Promise.all([getProjectUiState(projectId), listChatSessions(projectId)]);
  const state = getCachedProjectWorkspace(projectId) ?? loaded;
  const location = safeLocation(state?.lastLocation);
  if (location && parseDestination(location.split("?")[0])?.projectId === projectId
    && validSessionLocation(location, sessions)) return location;
  const newest = sessions.find((session) => !session.archived);
  const defaults = isDemoProjectId(projectId) ? defaultTaskWorkspace(newest?.id, !(await getUiState()).tourCompleted) : undefined;
  const task = getTaskWorkspace(state, newest?.id ?? "new");
  const rememberedPane = task ? task.active : defaults?.active;
  return taskLocation(projectId, newest?.id ?? null, rememberedPane);
}
