import type { Query, QueryFilters } from "@tanstack/react-query";
import type { ChatSession, Experiment, Run } from "../api";
import { queryClient, workspaceScope, isCurrentScope, deletedSessionIds, cancelAndRemove } from "./client";
import { markLiveUpdate } from "./live";

export const isCommit = (ref: unknown) => typeof ref === "string" && /^[0-9a-f]{40}$/i.test(ref);

export function isImmutableQuery(query: Query) {
  const family = query.queryKey[2];
  if (family === "resolvedFile") return isCommit(query.queryKey[7]);
  const options = family === "getProjectFile" ? query.queryKey[5] : family === "getCodeTree" ? query.queryKey[4] : null;
  return typeof options === "object" && options !== null && "ref" in options && isCommit(options.ref);
}

export function invalidateFamilies(families: readonly string[], scope = workspaceScope(), predicate?: QueryFilters["predicate"], cancelReads = false) {
  if (!isCurrentScope(scope)) return;
  const filters = {
    queryKey: scope,
    predicate: (query: Query) => families.includes(String(query.queryKey[2])) && !isImmutableQuery(query) && (!predicate || predicate(query)),
  };
  if (cancelReads) {
    void queryClient.cancelQueries(filters).then(() => {
      if (isCurrentScope(scope)) void queryClient.invalidateQueries(filters);
    });
  } else void queryClient.invalidateQueries(filters);
}

const fileBodies = ["resolvedFile", "getProjectFile", "getArtifactFileText", "getArtifactFileMetadata"];
export const artifactFamilies = ["getArtifacts", "getArtifactFileText", "getArtifactFileMetadata"];
export const liveFamilies = ["listProjects", "listProjectActivity", "listExperiments", "listRuns", "listChatSessions", "getChatMessages", "getHarnesses", "getUpdateStatus", ...artifactFamilies, ...fileBodies, "getAbsoluteFile", "fileVersion", "getCodeTree", "getSessionWorktree", "getRunDiff", "getExperimentDiff"];

export function invalidateProjectFiles(projectId: string, scope = workspaceScope(), cancelReads = false) {
  const sessions = queryClient.getQueryData<ChatSession[]>([...scope, "listChatSessions", projectId]) ?? [];
  const runs = queryClient.getQueryData<Run[]>([...scope, "listRuns", projectId]) ?? [];
  const experiments = queryClient.getQueryData<Experiment[]>([...scope, "listExperiments", projectId]) ?? [];
  invalidateFamilies([...fileBodies, ...artifactFamilies, "getRunDiff", "getExperimentDiff", "getCodeTree", "getSessionWorktree", "fileVersion", "getProjectGitStatus", "getOverleafState", "getOverleafStatus"], scope, (query) => {
    const family = query.queryKey[2];
    if (family === "getRunDiff") return runs.some((run) => run.id === query.queryKey[3]);
    if (family === "getExperimentDiff") return experiments.some((experiment) => experiment.id === query.queryKey[3]);
    if (family === "getSessionWorktree") return sessions.some((session) => session.id === query.queryKey[3]);
    if (family === "fileVersion") return typeof query.queryKey[3] === "string" && query.queryKey[3].startsWith(`/api/projects/${projectId}/`);
    return query.queryKey[3] === projectId;
  }, cancelReads);
}

export function removeSession(sessionId: string) {
  const scope = workspaceScope();
  deletedSessionIds.add(sessionId);
  markLiveUpdate(queryClient, [...scope, "listChatSessions"], sessionId);
  queryClient.setQueriesData<ChatSession[]>({ queryKey: [...scope, "listChatSessions"] }, (rows) => rows?.filter((row) => row.id !== sessionId));
  for (const family of ["getChatMessages", "getSessionWorktree"]) void cancelAndRemove({ queryKey: [...scope, family, sessionId] });
}

export function removeProject(projectId: string) {
  const scope = workspaceScope();
  const sessions = queryClient.getQueryData<ChatSession[]>([...scope, "listChatSessions", projectId]);
  for (const session of sessions ?? []) removeSession(session.id);
  void cancelAndRemove({ queryKey: scope, predicate: (query) => query.queryKey.slice(3).includes(projectId) });
}

const settingsFamilies: Record<string, readonly string[]> = {
  hf: ["getHfSettings", "getComputeSettings", "getEnvVars"],
  tinker: ["getTinkerSettings", "getComputeSettings", "getEnvVars"],
  k8s: ["getK8sSettings", "getComputeSettings"],
  modal: ["getModalSettings", "getComputeSettings", "getEnvVars"],
  slurm: ["getSlurmSettings", "getComputeSettings"],
  ray: ["getRaySettings", "getComputeSettings"],
  env: ["getEnvVars", "getHfSettings", "getTinkerSettings", "getModalSettings", "getSlurmSettings", "getRaySettings", "getK8sSettings", "getOpenResearchSettings", "getComputeSettings", "getHarnesses"],
  "data-dir": ["getDataDir"],
  ssh: ["getSshHosts", "getSshConfig", "getSshMasterStatus", "getComputeSettings"],
  compute: ["getComputeSettings"],
  git: ["githubAccount", "repoAccess", "getProjectGitStatus"],
  profile: ["getProfile"],
  "lit-sources": ["getLitSources"],
  projects: ["getProjectDefaults", "getProjectGitStatus"],
  telemetry: ["getTelemetry"],
};

export function invalidateWrite(url: string, scope: ReturnType<typeof workspaceScope>) {
  if (!isCurrentScope(scope)) return;
  const invalidate = (families: readonly string[], target = scope, predicate?: QueryFilters["predicate"]) => invalidateFamilies(families, target, predicate, true);
  const path = url.split("?")[0];
  if (/\/(ui-state|open|prewarm|validate|preflight)$/.test(path)) return;
  const setting = /^\/api\/settings\/([^/]+)/.exec(path)?.[1];
  if (setting) { invalidate(settingsFamilies[setting] ?? [], scope); return; }
  if (path === "/api/user-skills") { invalidate(["listUserSkills", "getSkills", "getSkillContent"], scope); return; }
  if (path === "/api/latex-templates") { invalidate(["listLatexTemplates"], scope); return; }
  if (path === "/api/overleaf/token") { invalidate(["getOverleafSettings", "getOverleafState", "getOverleafStatus"], scope); return; }
  if (path.startsWith("/api/update/")) { invalidate(["getUpdateStatus", "getLocalMachine"], scope); return; }
  if (path.startsWith("/api/remote/") || path.startsWith("/_orx/")) {
    invalidate(["listRemoteSessions"], scope);
    void queryClient.invalidateQueries({ queryKey: ["gateway", "runtime"] });
    return;
  }
  if (path.startsWith("/api/chat/")) {
    if (path === "/api/chat/sessions") invalidate(["getProjectStarterPrompts", "listProjectActivity"], scope);
    return;
  }
  const project = /^\/api\/projects\/([^/]+)(.*)$/.exec(path);
  if (project) {
    const [, projectId, action] = project;
    if (action.startsWith("/file")) { invalidateProjectFiles(projectId, scope, true); return; }
    invalidate(["listProjects", "listProjectActivity"], scope);
    invalidate(["getProjectGitStatus", "getCodeTree", "getProjectStarterPrompts"], scope, (query) => query.queryKey[3] === projectId);
    return;
  }
  if (path === "/api/projects" || path === "/api/onboarding/complete") invalidate(["listProjects", "listProjectActivity", "getUiState", "getProfile"], scope);
}
