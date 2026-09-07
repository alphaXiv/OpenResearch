import { readLiveSnapshot } from "./live";
import { queryOptions } from "@tanstack/react-query";
import * as api from "../api";
import { queryClient, isCurrentScope, workspaceKey } from "./client";

export const getHfSettingsQuery = () => queryOptions({
  queryKey: workspaceKey("getHfSettings"),
  queryFn: ({ signal }) => api.getHfSettings(signal),
  staleTime: 300_000,
});

export const getTinkerSettingsQuery = () => queryOptions({
  queryKey: workspaceKey("getTinkerSettings"),
  queryFn: ({ signal }) => api.getTinkerSettings(signal),
  staleTime: 300_000,
});

export const getK8sSettingsQuery = () => queryOptions({
  queryKey: workspaceKey("getK8sSettings"),
  queryFn: ({ signal }) => api.getK8sSettings(signal),
  staleTime: 30_000,
});

export const getModalSettingsQuery = () => queryOptions({
  queryKey: workspaceKey("getModalSettings"),
  queryFn: ({ signal }) => api.getModalSettings(signal),
  staleTime: 300_000,
});

export const getSlurmSettingsQuery = () => queryOptions({
  queryKey: workspaceKey("getSlurmSettings"),
  queryFn: ({ signal }) => api.getSlurmSettings(signal),
  staleTime: 300_000,
});

export const getRaySettingsQuery = () => queryOptions({
  queryKey: workspaceKey("getRaySettings"),
  queryFn: ({ signal }) => api.getRaySettings(signal),
  staleTime: 300_000,
});

export const getComputeSettingsQuery = (projectId?: string) => queryOptions({
  queryKey: workspaceKey("getComputeSettings", projectId ?? null),
  queryFn: ({ signal }) => api.getComputeSettings(projectId, signal),
  staleTime: 30_000,
});

export const getLocalMachineQuery = () => queryOptions({
  queryKey: workspaceKey("getLocalMachine"),
  queryFn: ({ signal }) => api.getLocalMachine(signal),
  staleTime: 300_000,
});

export const getOpenResearchSettingsQuery = () => queryOptions({
  queryKey: workspaceKey("getOpenResearchSettings"),
  queryFn: ({ signal }) => api.getOpenResearchSettings(signal),
  staleTime: 30_000,
});

export const getEnvVarsQuery = () => queryOptions({
  queryKey: workspaceKey("getEnvVars"),
  queryFn: ({ signal }) => api.getEnvVars(signal),
  staleTime: 300_000,
});

export const getDataDirQuery = () => queryOptions({
  queryKey: workspaceKey("getDataDir"),
  queryFn: ({ signal }) => api.getDataDir(signal),
  staleTime: 300_000,
});

export const getSshHostsQuery = () => queryOptions({
  queryKey: workspaceKey("getSshHosts"),
  queryFn: ({ signal }) => api.getSshHosts(signal),
  staleTime: 300_000,
});

export const getSshConfigQuery = () => queryOptions({
  queryKey: workspaceKey("getSshConfig"),
  queryFn: ({ signal }) => api.getSshConfig(signal),
  staleTime: 300_000,
});

export const getSshMasterStatusQuery = (host: string) => queryOptions({
  queryKey: workspaceKey("getSshMasterStatus", host),
  queryFn: ({ signal }) => api.getSshMasterStatus(host, signal),
  staleTime: 5_000,
});

export const getRuntimeQuery = () => queryOptions({
  queryKey: ["gateway", "runtime"] as const,
  queryFn: ({ signal }) => api.getRuntime(signal),
  staleTime: 2_000,
});

export const listRemoteSessionsQuery = () => queryOptions({
  queryKey: workspaceKey("listRemoteSessions"),
  queryFn: ({ signal }) => api.listRemoteSessions(signal),
  staleTime: 30_000,
});

export const getUpdateStatusQuery = () => queryOptions({
  queryKey: workspaceKey("getUpdateStatus"),
  queryFn: ({ signal, client, queryKey }) => readLiveSnapshot(client, queryKey, () => api.getUpdateStatus(signal)),
  staleTime: 30_000,
});

export const getProfileQuery = () => queryOptions({
  queryKey: workspaceKey("getProfile"),
  queryFn: ({ signal }) => api.getProfile(signal),
  staleTime: 300_000,
});

export const getLitSourcesQuery = () => queryOptions({
  queryKey: workspaceKey("getLitSources"),
  queryFn: ({ signal }) => api.getLitSources(signal),
  staleTime: 300_000,
});

export const getProjectDefaultsQuery = () => queryOptions({
  queryKey: workspaceKey("getProjectDefaults"),
  queryFn: ({ signal }) => api.getProjectDefaults(signal),
  staleTime: 300_000,
});

export const getProjectGitStatusQuery = (projectId: string) => queryOptions({
  queryKey: workspaceKey("getProjectGitStatus", projectId),
  queryFn: ({ signal }) => api.getProjectGitStatus(projectId, signal),
  staleTime: 30_000,
});

export const getTelemetryQuery = () => queryOptions({
  queryKey: workspaceKey("getTelemetry"),
  queryFn: ({ signal }) => api.getTelemetry(signal),
  staleTime: 300_000,
});

export const getHarnessesQuery = () => queryOptions({
  queryKey: workspaceKey("getHarnesses"),
  queryFn: ({ signal }) => api.getHarnesses(false, false, signal),
  staleTime: 300_000,
});

export const getSkillsQuery = () => queryOptions({
  queryKey: workspaceKey("getSkills"),
  queryFn: ({ signal }) => api.getSkills(signal),
  staleTime: 300_000,
});

export const getSkillContentQuery = (name: string, projectId?: string) => queryOptions({
  queryKey: workspaceKey("getSkillContent", name, projectId ?? null),
  queryFn: ({ signal }) => api.getSkillContent(name, projectId, signal),
  staleTime: 300_000,
});

export const listLatexTemplatesQuery = () => queryOptions({
  queryKey: workspaceKey("listLatexTemplates"),
  queryFn: ({ signal }) => api.listLatexTemplates(signal),
  staleTime: 300_000,
});

export const listUserSkillsQuery = () => queryOptions({
  queryKey: workspaceKey("listUserSkills"),
  queryFn: ({ signal }) => api.listUserSkills(signal),
  staleTime: 300_000,
});

export async function refreshHarnesses(refresh = false, retryRejected = false) {
  const options = getHarnessesQuery();
  if (!refresh && !retryRejected) return queryClient.fetchQuery(options);
  await queryClient.cancelQueries(options);
  const data = await api.getHarnesses(refresh, retryRejected);
  if (isCurrentScope(options.queryKey)) queryClient.setQueryData(options.queryKey, data);
  return data;
}
