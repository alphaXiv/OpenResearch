import { readLiveSnapshot, mergeLiveList } from "./live";
import { queryOptions } from "@tanstack/react-query";
import * as api from "../api";
import { workspaceKey } from "./client";

export const listProjectsQuery = () => queryOptions({
  queryKey: workspaceKey("listProjects"),
  queryFn: ({ signal, client, queryKey }) => readLiveSnapshot(client, queryKey, () => api.listProjects(signal), mergeLiveList),
  staleTime: 30_000,
});

export const listProjectActivityQuery = () => queryOptions({
  queryKey: workspaceKey("listProjectActivity"),
  queryFn: ({ signal }) => api.listProjectActivity(signal),
  staleTime: 30_000,
});

export const getUiStateQuery = () => queryOptions({
  queryKey: workspaceKey("getUiState"),
  queryFn: ({ signal }) => api.getUiState(signal),
  staleTime: 300_000,
});

export const getProjectUiStateQuery = (projectId: string) => queryOptions({
  queryKey: workspaceKey("getProjectUiState", projectId),
  queryFn: ({ signal }) => api.getProjectUiState(projectId, signal),
  staleTime: 300_000,
});

export const getProjectPathStatusQuery = (path = "") => queryOptions({
  queryKey: workspaceKey("getProjectPathStatus", path),
  queryFn: ({ signal }) => api.getProjectPathStatus(path, signal),
  staleTime: 30_000,
});

export const searchPapersQuery = (q: string) => queryOptions({
  queryKey: workspaceKey("searchPapers", q),
  queryFn: ({ signal }) => api.searchPapers(q, signal),
  staleTime: 300_000,
});

export const githubAccountQuery = () => queryOptions({
  queryKey: workspaceKey("githubAccount"),
  queryFn: ({ signal }) => api.githubAccount(signal),
  staleTime: 300_000,
});

export const githubProjectRepoPreviewQuery = (name: string) => queryOptions({
  queryKey: workspaceKey("githubProjectRepoPreview", name),
  queryFn: ({ signal }) => api.githubProjectRepoPreview(name, signal),
  staleTime: 30_000,
});

export const repoAccessQuery = (owner: string, repo: string) => queryOptions({
  queryKey: workspaceKey("repoAccess", owner, repo),
  queryFn: ({ signal }) => api.repoAccess(owner, repo, signal),
  staleTime: 30_000,
});

export const resolvePaperQuery = (id: string) => queryOptions({
  queryKey: workspaceKey("resolvePaper", id),
  queryFn: ({ signal }) => api.resolvePaper(id, signal),
  staleTime: 3_600_000,
});

export const getProjectStarterPromptsQuery = (projectId: string, harness: api.HarnessId, model: string | null, locale: string) => queryOptions({
  queryKey: workspaceKey("getProjectStarterPrompts", projectId, harness, model, locale),
  queryFn: ({ signal }) => api.getProjectStarterPrompts(projectId, harness, model, locale, signal),
  staleTime: 300_000,
  refetchOnWindowFocus: false,
  retryOnMount: false,
});

export const listExperimentsQuery = (projectId: string) => queryOptions({
  queryKey: workspaceKey("listExperiments", projectId),
  queryFn: ({ signal, client, queryKey }) => readLiveSnapshot(client, queryKey, () => api.listExperiments(projectId, signal), mergeLiveList),
  staleTime: 30_000,
});

export const listRunsQuery = (projectId: string) => queryOptions({
  queryKey: workspaceKey("listRuns", projectId),
  queryFn: ({ signal, client, queryKey }) => readLiveSnapshot(client, queryKey, () => api.listRuns(projectId, signal), mergeLiveList),
  staleTime: 30_000,
});
