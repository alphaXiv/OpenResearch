import type { Api } from "../api";
import type { WorkspaceClient } from "./client";
import { readLiveSnapshot, mergeLiveList } from "./live";
import { queryOptions } from "@tanstack/react-query";
import type * as api from "../api";

export function createProjectQueries(client: WorkspaceClient, api: Api) {
  const { workspaceKey } = client;

  const listProjectsQuery = () => queryOptions({
    queryKey: workspaceKey("listProjects"),
    queryFn: ({ signal, client, queryKey }) => readLiveSnapshot(client, queryKey, () => api.listProjects(signal), mergeLiveList),
    staleTime: 30_000,
  });

  const listProjectActivityQuery = () => queryOptions({
    queryKey: workspaceKey("listProjectActivity"),
    queryFn: ({ signal }) => api.listProjectActivity(signal),
    staleTime: 30_000,
  });

  const getUiStateQuery = () => queryOptions({
    queryKey: workspaceKey("getUiState"),
    queryFn: ({ signal }) => api.getUiState(signal),
    staleTime: 300_000,
  });

  const getProjectUiStateQuery = (projectId: string) => queryOptions({
    queryKey: workspaceKey("getProjectUiState", projectId),
    queryFn: ({ signal }) => api.getProjectUiState(projectId, signal),
    staleTime: 300_000,
  });

  const getProjectPathStatusQuery = (path = "") => queryOptions({
    queryKey: workspaceKey("getProjectPathStatus", path),
    queryFn: ({ signal }) => api.getProjectPathStatus(path, signal),
    staleTime: 30_000,
  });

  const searchPapersQuery = (q: string) => queryOptions({
    queryKey: workspaceKey("searchPapers", q),
    queryFn: ({ signal }) => api.searchPapers(q, signal),
    staleTime: 300_000,
  });

  const githubAccountQuery = () => queryOptions({
    queryKey: workspaceKey("githubAccount"),
    queryFn: ({ signal }) => api.githubAccount(signal),
    staleTime: 300_000,
  });

  const githubOrganizationsQuery = () => queryOptions({
    queryKey: workspaceKey("githubOrganizations"),
    queryFn: ({ signal }) => api.githubOrganizations(signal),
    staleTime: 300_000,
  });

  const githubProjectRepoPreviewQuery = (name: string) => queryOptions({
    queryKey: workspaceKey("githubProjectRepoPreview", name),
    queryFn: ({ signal }) => api.githubProjectRepoPreview(name, signal),
    staleTime: 30_000,
  });

  const repoAccessQuery = (owner: string, repo: string) => queryOptions({
    queryKey: workspaceKey("repoAccess", owner, repo),
    queryFn: ({ signal }) => api.repoAccess(owner, repo, signal),
    staleTime: 30_000,
  });

  const resolvePaperQuery = (id: string) => queryOptions({
    queryKey: workspaceKey("resolvePaper", id),
    queryFn: ({ signal }) => api.resolvePaper(id, signal),
    staleTime: 3_600_000,
  });

  const getProjectStarterPromptsQuery = (projectId: string, harness: api.HarnessId, model: string | null, locale: string) => queryOptions({
    queryKey: workspaceKey("getProjectStarterPrompts", projectId, harness, model, locale),
    queryFn: ({ signal }) => api.getProjectStarterPrompts(projectId, harness, model, locale, signal),
    staleTime: 300_000,
    refetchOnWindowFocus: false,
    retryOnMount: false,
  });

  const listExperimentsQuery = (projectId: string) => queryOptions({
    queryKey: workspaceKey("listExperiments", projectId),
    queryFn: ({ signal, client, queryKey }) => readLiveSnapshot(client, queryKey, () => api.listExperiments(projectId, signal), mergeLiveList),
    staleTime: 30_000,
  });

  const listRunsQuery = (projectId: string) => queryOptions({
    queryKey: workspaceKey("listRuns", projectId),
    queryFn: ({ signal, client, queryKey }) => readLiveSnapshot(client, queryKey, () => api.listRuns(projectId, signal), mergeLiveList),
    staleTime: 30_000,
  });

  return {
    listProjectsQuery,
    listProjectActivityQuery,
    getUiStateQuery,
    getProjectUiStateQuery,
    getProjectPathStatusQuery,
    searchPapersQuery,
    githubAccountQuery,
    githubOrganizationsQuery,
    githubProjectRepoPreviewQuery,
    repoAccessQuery,
    resolvePaperQuery,
    getProjectStarterPromptsQuery,
    listExperimentsQuery,
    listRunsQuery,
  };
}
