import type { Api } from "../api";
import type { WorkspaceClient } from "./client";
import type { Workspace } from "../workspace";
import { readLiveSnapshot, mergeLiveList, markLiveUpdate } from "./live";
import { mutations } from "./mutations";
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

  const getProjectStarterPromptsQuery = (projectId: string, harness: api.HarnessId, model: string | null, locale: string, profile: string | null) => queryOptions({
    queryKey: workspaceKey("getProjectStarterPrompts", projectId, harness, model, locale, profile),
    queryFn: ({ signal }) => api.getProjectStarterPrompts(projectId, harness, model, locale, profile, signal),
    staleTime: 300_000,
    refetchOnWindowFocus: false,
    retryOnMount: false,
  });

  const getProjectLinkQuery = (projectId: string) => queryOptions({
    queryKey: workspaceKey("getProjectLink", projectId),
    queryFn: ({ signal }) => api.getProjectLink(projectId, signal),
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
    getProjectLinkQuery,
    listExperimentsQuery,
    listRunsQuery,
  };
}

function showProject(ws: Workspace, project: api.Project) {
  const { queryKey } = ws.queries.listProjectsQuery();
  markLiveUpdate(ws.queryClient, queryKey, project.id);
  ws.setScopedQueryData(queryKey, (rows) => rows && [...rows.filter((row) => row.id !== project.id), project]);
}

export const mutCreateProject = mutations.define({
  id: "projects/create",
  mutate: (ws: Workspace, project: api.NewProject) => ws.api.createProject(project),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess(({ project }) => showProject(ws, project));
  },
  describe: "create the project",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutImportSharedProject = mutations.define({
  id: "projects/import",
  mutate: (ws: Workspace, path: string, source: api.SharedSource, onProgress: (progress: api.OperationProgress) => void) =>
    ws.api.importSharedProject(path, source, onProgress),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess(({ project }) => showProject(ws, project));
  },
  describe: "open the shared project",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutUpdateProject = mutations.define({
  id: "projects/update",
  mutate: (ws: Workspace, projectId: string, changes: Parameters<Api["updateProject"]>[1]) => ws.api.updateProject(projectId, changes),
  optimistic({ helpers, args: [ws, projectId, changes], onSuccess }) {
    const options = ws.queries.listProjectsQuery();
    helpers(ws.queryClient).arrayUpdate(options, {
      filter: (row) => row.id === projectId,
      update: (row) => ({ ...row, ...changes }),
    });
    onSuccess((saved) => {
      markLiveUpdate(ws.queryClient, options.queryKey, saved.id);
      ws.setScopedQueryData(options.queryKey, (rows) => rows?.map((row) => row.id === saved.id ? saved : row));
    });
  },
  key: ({ args: [ws, projectId] }) => [ws.identity, projectId],
  describe: "update the project",
  describeResult: null,
  refetchOnSuccess: false,
});

/** Resolves to why the artifacts folder could not be removed, if it couldn't. */
export const mutDeleteProject = mutations.define({
  id: "projects/delete",
  mutate: (ws: Workspace, projectId: string, deleteArtifacts: boolean) => ws.api.deleteProject(projectId, deleteArtifacts),
  optimistic: () => {},
  key: ({ args: [ws, projectId] }) => [ws.identity, projectId],
  describe: "delete the project",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutPublishProject = mutations.define({
  id: "projects/publish",
  mutate: (ws: Workspace, projectId: string, choice: api.ShareChoice, onProgress: (progress: api.OperationProgress) => void) =>
    ws.api.publishProject(projectId, choice, onProgress),
  optimistic({ args: [ws, projectId], onSuccess }) {
    onSuccess(({ sharedProject }) =>
      ws.setScopedQueryData(ws.queries.getProjectLinkQuery(projectId).queryKey, { signedIn: true, sharedProject }));
  },
  key: ({ args: [ws, projectId] }) => [ws.identity, projectId],
  describe: "publish the project",
  describeResult: null,
  refetchOnSuccess: false,
});

type ExperimentRef = Pick<api.Experiment, "id" | "projectId">;

export const mutSetExperimentArchived = mutations.define({
  id: "projects/experiment-archived",
  mutate: (ws: Workspace, experiment: ExperimentRef, direction: Parameters<Api["setExperimentArchived"]>[1], archived: boolean) =>
    ws.api.setExperimentArchived(experiment.id, direction, archived),
  optimistic({ helpers, args: [ws, experiment, direction, archived], onSuccess }) {
    const options = ws.queries.listExperimentsQuery(experiment.projectId);
    if (direction === "only") {
      helpers(ws.queryClient).arrayUpdate(options, {
        filter: (row) => row.id === experiment.id,
        update: (row) => ({ ...row, archived }),
      });
    }
    onSuccess(({ ids }) => {
      for (const id of ids) markLiveUpdate(ws.queryClient, options.queryKey, id);
      ws.setScopedQueryData(options.queryKey, (rows) => rows?.map((row) => ids.includes(row.id) ? { ...row, archived } : row));
    });
  },
  key: ({ args: [ws, experiment] }) => [ws.identity, experiment.projectId],
  describe: "archive experiments",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutDeleteExperiment = mutations.define({
  id: "projects/experiment-delete",
  mutate: (ws: Workspace, experiment: ExperimentRef) => ws.api.deleteExperiment(experiment.id),
  optimistic({ helpers, args: [ws, experiment] }) {
    const cache = helpers(ws.queryClient);
    cache.arrayRemove(ws.queries.listExperimentsQuery(experiment.projectId), (row) => row.id === experiment.id);
    cache.arrayRemove(ws.queries.listRunsQuery(experiment.projectId), (run) => run.experimentId === experiment.id);
  },
  key: ({ args: [ws, experiment] }) => [ws.identity, experiment.projectId],
  describe: "delete the experiment",
  describeResult: null,
  refetchOnSuccess: true,
});

export const mutCancelRun = mutations.define({
  id: "projects/run-cancel",
  mutate: (ws: Workspace, run: Pick<api.Run, "id" | "projectId">) => ws.api.cancelRun(run.id),
  optimistic({ helpers, args: [ws, run] }) {
    const cache = helpers(ws.queryClient);
    cache.arrayUpdate(ws.queries.listRunsQuery(run.projectId), {
      filter: (row) => row.id === run.id,
      update: (row) => ({ ...row, cancelling: true }),
    });
    cache.arrayUpdate(ws.queries.listChatActiveRunsQuery(), {
      filter: (item) => item.run.id === run.id,
      update: (item) => ({ ...item, run: { ...item.run, cancelling: true } }),
    });
  },
  key: ({ args: [ws, run] }) => [ws.identity, run.id],
  describe: "stop the run",
  describeResult: null,
  refetchOnSuccess: true,
});

export const mutCleanupRun = mutations.define({
  id: "projects/run-cleanup",
  mutate: (ws: Workspace, runId: string) => ws.api.cleanupRun(runId),
  optimistic: () => {},
  key: ({ args: [ws, runId] }) => [ws.identity, runId],
  describe: "stop the run's leftover job",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutAbandonRun = mutations.define({
  id: "projects/run-abandon",
  mutate: (ws: Workspace, runId: string) => ws.api.abandonRun(runId),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess((saved) => {
      const options = ws.queries.listRunsQuery(saved.projectId);
      markLiveUpdate(ws.queryClient, options.queryKey, saved.id);
      ws.setScopedQueryData(options.queryKey, (rows) => rows?.map((row) => row.id === saved.id ? saved : row));
    });
  },
  key: ({ args: [ws, runId] }) => [ws.identity, runId],
  describe: "abandon the run",
  describeResult: null,
  refetchOnSuccess: false,
});
