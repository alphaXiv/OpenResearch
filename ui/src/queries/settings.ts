import type { WorkspaceEvents } from "../workspace";
import { upsertEngineHost } from "../engineHosts";
import type { WorkspaceInvalidation } from "./invalidation";
import type { Api } from "../api";
import type { WorkspaceClient } from "./client";
import { readLiveSnapshot } from "./live";
import { queryOptions, useMutation, useQuery } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import type * as api from "../api";

export function createSettingsQueries(client: WorkspaceClient, api: Api, invalidation: WorkspaceInvalidation, events: WorkspaceEvents) {
  const { workspaceKey, queryClient, isCurrentScope, setScopedQueryData } = client;
  const { invalidateFamilies } = invalidation;

  const getFileManagerQuery = (locale: string) => queryOptions({
      queryKey: workspaceKey("getFileManager", locale),
      queryFn: ({ signal }) => api.getFileManager(locale, signal),
      staleTime: 300_000,
  });

  const getHfSettingsQuery = () => queryOptions({
    queryKey: workspaceKey("getHfSettings"),
    queryFn: ({ signal }) => api.getHfSettings(signal),
    staleTime: 300_000,
  });

  const getTinkerSettingsQuery = () => queryOptions({
    queryKey: workspaceKey("getTinkerSettings"),
    queryFn: ({ signal }) => api.getTinkerSettings(signal),
    staleTime: 300_000,
  });

  const getK8sSettingsQuery = () => queryOptions({
    queryKey: workspaceKey("getK8sSettings"),
    queryFn: ({ signal }) => api.getK8sSettings(signal),
    staleTime: 30_000,
  });

  const getModalSettingsQuery = () => queryOptions({
    queryKey: workspaceKey("getModalSettings"),
    queryFn: ({ signal }) => api.getModalSettings(signal),
    staleTime: 300_000,
  });

  const getVercelSettingsQuery = () => queryOptions({
    queryKey: workspaceKey("getVercelSettings"),
    queryFn: ({ signal }) => api.getVercelSettings(signal),
    staleTime: 30_000,
  });

  const getSlurmSettingsQuery = () => queryOptions({
    queryKey: workspaceKey("getSlurmSettings"),
    queryFn: ({ signal }) => api.getSlurmSettings(signal),
    staleTime: 300_000,
  });

  const getRaySettingsQuery = () => queryOptions({
    queryKey: workspaceKey("getRaySettings"),
    queryFn: ({ signal }) => api.getRaySettings(signal),
    staleTime: 300_000,
  });

  const getRunEngineQuery = () => queryOptions({
    queryKey: workspaceKey("getRunEngine"),
    queryFn: ({ signal }) => api.getRunEngine(signal),
    staleTime: 30_000,
  });

  const getComputeSettingsQuery = (projectId?: string) => queryOptions({
    queryKey: workspaceKey("getComputeSettings", projectId ?? null),
    queryFn: ({ signal }) => api.getComputeSettings(projectId, signal),
    staleTime: 30_000,
  });

  const getPublicNetworkQuery = () => queryOptions({
    queryKey: workspaceKey("getPublicNetwork"),
    queryFn: ({ signal }) => api.getPublicNetwork(signal),
    staleTime: 300_000,
  });

  const getLocalMachineQuery = () => queryOptions({
    queryKey: workspaceKey("getLocalMachine"),
    queryFn: ({ signal }) => api.getLocalMachine(signal),
    staleTime: 300_000,
  });

  const getOpenResearchSettingsQuery = () => queryOptions({
    queryKey: workspaceKey("getOpenResearchSettings"),
    queryFn: ({ signal }) => api.getOpenResearchSettings(signal),
    staleTime: 30_000,
  });

  const useOpenResearchAccount = () => useQuery({
    queryKey: workspaceKey("getOpenResearchAccount"),
    queryFn: ({ signal }) => api.getOpenResearchAccount(signal),
  }, queryClient);

  /** Fails with `AlphaXivNotConnectedError` while the account has no alphaXiv connection. */
  const useAlphaXivFolders = () => {
    const account = useOpenResearchAccount();
    return useQuery({
      queryKey: workspaceKey("alphaXivFolders", account.data?.user?.id ?? null),
      queryFn: ({ signal }) => api.getAlphaXivFolders(signal),
      enabled: !!account.data?.loggedIn,
    }, queryClient);
  };

  /** Fails with `AlphaXivNotConnectedError` while the account has no alphaXiv connection. */
  const useAlphaXivMe = () => {
    const account = useOpenResearchAccount();
    return useQuery({
      queryKey: workspaceKey("alphaXivMe", account.data?.user?.id ?? null),
      queryFn: ({ signal }) => api.getAlphaXivMe(signal),
      enabled: !!account.data?.user,
      retry: false,
    }, queryClient);
  };

  const useSignOutOpenResearch = () => useMutation({
    mutationFn: async () => {
      const key = workspaceKey("getOpenResearchAccount");
      await api.logoutOpenResearch();
      queryClient.removeQueries({ queryKey: workspaceKey("alphaXivFolders") });
      queryClient.removeQueries({ queryKey: workspaceKey("alphaXivMe") });
      queryClient.removeQueries({ queryKey: workspaceKey("alphaXivPapers") });
      await queryClient.cancelQueries({ queryKey: key });
      if (!isCurrentScope(key)) return;
      queryClient.setQueryData(key, { loggedIn: false, apiUrl: null, user: null });
      invalidateFamilies(["getOpenResearchSettings", "getComputeSettings"]);
    },
  }, queryClient);

  /** Stays pending until the settings reflect the new key, so the step never offers to add it twice. */
  const useAddOpenResearchSshKey = () => useMutation({
    mutationFn: async () => {
      await api.addOpenResearchSshKey();
      await queryClient.invalidateQueries({ queryKey: workspaceKey("getOpenResearchSettings") });
    },
  }, queryClient);

  const getEnvVarsQuery = () => queryOptions({
    queryKey: workspaceKey("getEnvVars"),
    queryFn: ({ signal }) => api.getEnvVars(signal),
    staleTime: 300_000,
  });

  /** `live` polls while a run is in flight, so its Visualize button appears soon after it logs. */
  const getTrackioRunsQuery = (projectId: string, live: boolean) => queryOptions({
    queryKey: workspaceKey("getTrackioRuns", projectId),
    queryFn: ({ signal }) => api.getTrackioRuns(projectId, signal),
    staleTime: 15_000,
    refetchInterval: live ? 15_000 : false,
  });

  /** Also refetches once a run ends: Trackio flushes on `finish()`, after the last poll. */
  function useTrackioRuns(projectId: string, live: boolean) {
    const query = useQuery(getTrackioRunsQuery(projectId, live));
    const wasLive = useRef(live);
    const { refetch } = query;
    useEffect(() => {
      if (wasLive.current && !live) void refetch({ cancelRefetch: false });
      wasLive.current = live;
    }, [live, refetch]);
    return query;
  }

  const getDataDirQuery = () => queryOptions({
    queryKey: workspaceKey("getDataDir"),
    queryFn: ({ signal }) => api.getDataDir(signal),
    staleTime: 300_000,
  });

  const getSshSettingsQuery = () => queryOptions({
    queryKey: workspaceKey("getSshSettings"),
    queryFn: ({ signal }) => api.getSshSettings(signal),
    staleTime: 30_000,
  });

  const getEngineHostsQuery = () => queryOptions({
    queryKey: workspaceKey("listEngineHosts"),
    queryFn: ({ signal }) => api.listEngineHosts(signal),
    staleTime: 30_000,
  });

  function setEngineHost(host: api.EngineHost) {
    setScopedQueryData(getEngineHostsQuery().queryKey, (hosts) => hosts && upsertEngineHost(hosts, host));
  }

  /** The run engine's host connections, kept current by `host.updated` events. */
  function useEngineHosts() {
    useEffect(() => events.onEngineHost(setEngineHost), []);
    return useQuery(getEngineHostsQuery()).data;
  }

  const getSshConfigQuery = () => queryOptions({
    queryKey: workspaceKey("getSshConfig"),
    queryFn: ({ signal }) => api.getSshConfig(signal),
    staleTime: 300_000,
  });

  const getSshMasterStatusQuery = (host: string) => queryOptions({
    queryKey: workspaceKey("getSshMasterStatus", host),
    queryFn: ({ signal }) => api.getSshMasterStatus(host, signal),
    staleTime: 5_000,
  });

  const getRuntimeQuery = () => queryOptions({
    queryKey: ["gateway", "runtime"] as const,
    queryFn: ({ signal }) => api.getRuntime(signal),
    staleTime: 2_000,
  });

  const listRemoteSessionsQuery = () => queryOptions({
    queryKey: workspaceKey("listRemoteSessions"),
    queryFn: ({ signal }) => api.listRemoteSessions(signal),
    staleTime: 30_000,
  });

  const getUpdateStatusQuery = () => queryOptions({
    queryKey: workspaceKey("getUpdateStatus"),
    queryFn: ({ signal, client, queryKey }) => readLiveSnapshot(client, queryKey, () => api.getUpdateStatus(signal)),
    staleTime: 30_000,
  });

  const getBuildNotesQuery = () => queryOptions({
    queryKey: workspaceKey("getBuildNotes"),
    queryFn: ({ signal }) => api.getBuildNotes(signal),
    staleTime: 300_000,
  });

  const getLitSourcesQuery = () => queryOptions({
    queryKey: workspaceKey("getLitSources"),
    queryFn: ({ signal }) => api.getLitSources(signal),
    staleTime: 300_000,
  });

  const getProjectDefaultsQuery = () => queryOptions({
    queryKey: workspaceKey("getProjectDefaults"),
    queryFn: ({ signal }) => api.getProjectDefaults(signal),
    staleTime: 300_000,
  });

  const getProjectGitStatusQuery = (projectId: string) => queryOptions({
    queryKey: workspaceKey("getProjectGitStatus", projectId),
    queryFn: ({ signal }) => api.getProjectGitStatus(projectId, signal),
    staleTime: 30_000,
  });

  const getTelemetryQuery = () => queryOptions({
    queryKey: workspaceKey("getTelemetry"),
    queryFn: ({ signal }) => api.getTelemetry(signal),
    staleTime: 300_000,
  });

  const getHarnessSetupCommandsQuery = () => queryOptions({
    queryKey: workspaceKey("getHarnessSetupCommands"),
    queryFn: ({ signal }) => api.getHarnessSetupCommands(signal),
    staleTime: Infinity,
  });

  const getHarnessesQuery = () => queryOptions({
    queryKey: workspaceKey("getHarnesses"),
    queryFn: ({ signal }) => api.getHarnesses(false, false, signal),
    staleTime: 300_000,
    // Poll while anything is unresolved: `harness.catalog` is edge-triggered, so
    // a fill that lands before the EventSource connects would otherwise strand a
    // provisional payload until the 5-minute staleTime expires.
    refetchInterval: (query) => query.state.data?.some((h) => h.accountLoading || h.catalogPending) ? 1_000 : false,
  });

  const getSkillsQuery = (harness?: string, projectId?: string) => queryOptions({
    queryKey: workspaceKey("getSkills", harness ?? null, projectId ?? null),
    queryFn: ({ signal }) => api.getSkills(signal, harness, projectId),
    select: (data) => data.skills,
    refetchInterval: 30_000,
    staleTime: 300_000,
  });

  const getSkillContentQuery = (name: string, projectId?: string, harness?: string | null) => queryOptions({
    queryKey: workspaceKey("getSkillContent", name, projectId ?? null, harness ?? null),
    queryFn: ({ signal }) => api.getSkillContent(name, projectId, signal, harness),
    staleTime: 300_000,
  });

  const listLatexTemplatesQuery = () => queryOptions({
    queryKey: workspaceKey("listLatexTemplates"),
    queryFn: ({ signal }) => api.listLatexTemplates(signal),
    refetchInterval: 30_000,
    staleTime: 300_000,
  });

  const listUserSkillsQuery = () => queryOptions({
    queryKey: workspaceKey("listUserSkills"),
    queryFn: ({ signal }) => api.listUserSkills(signal),
    refetchInterval: 30_000,
    staleTime: 300_000,
  });

  async function refreshHarnesses(refresh = false, retryRejected = false) {
    if (!refresh && !retryRejected) return queryClient.fetchQuery(getHarnessesQuery());
    return fetchHarnesses(refresh, retryRejected);
  }

  /** Re-check one harness without waiting on the others' probes. */
  const refreshHarness = (harness: api.HarnessId) => fetchHarnesses(true, false, harness);

  /** The server's cached answer, skipping any in-flight poll that may predate a publish. */
  const readHarnesses = () => fetchHarnesses(false, false);

  async function fetchHarnesses(refresh: boolean, retryRejected: boolean, harness?: api.HarnessId) {
    const options = getHarnessesQuery();
    await queryClient.cancelQueries(options);
    const data = await api.getHarnesses(refresh, retryRejected, undefined, harness);
    if (isCurrentScope(options.queryKey)) queryClient.setQueryData(options.queryKey, data);
    return data;
  }

  const getLocalModelsQuery = () => queryOptions({
    queryKey: workspaceKey("getLocalModels"),
    queryFn: ({ signal }) => api.getLocalModels(signal),
    staleTime: 30_000,
  });

  const reportLocaleQuery = (locale: string) => queryOptions({
    queryKey: workspaceKey("reportLocale", locale),
    queryFn: ({ signal }) => api.reportLocale(locale, signal),
    staleTime: 0,
    retry: 3,
    retryDelay: 1_000,
    refetchOnWindowFocus: false,
  });

  return {
    getFileManagerQuery,
    getHfSettingsQuery,
    getTinkerSettingsQuery,
    getK8sSettingsQuery,
    getModalSettingsQuery,
    getVercelSettingsQuery,
    getSlurmSettingsQuery,
    getRaySettingsQuery,
    getRunEngineQuery,
    getComputeSettingsQuery,
    getLocalMachineQuery,
    getPublicNetworkQuery,
    getOpenResearchSettingsQuery,
    useOpenResearchAccount,
    useAlphaXivFolders,
    useAlphaXivMe,
    useSignOutOpenResearch,
    useAddOpenResearchSshKey,
    getEnvVarsQuery,
    getTrackioRunsQuery,
    useTrackioRuns,
    getDataDirQuery,
    getSshSettingsQuery,
    setEngineHost,
    useEngineHosts,
    getSshConfigQuery,
    getSshMasterStatusQuery,
    getRuntimeQuery,
    listRemoteSessionsQuery,
    getUpdateStatusQuery,
    getBuildNotesQuery,
    getLitSourcesQuery,
    getProjectDefaultsQuery,
    getProjectGitStatusQuery,
    getTelemetryQuery,
    getHarnessSetupCommandsQuery,
    getHarnessesQuery,
    getSkillsQuery,
    getSkillContentQuery,
    listLatexTemplatesQuery,
    listUserSkillsQuery,
    refreshHarnesses,
    refreshHarness,
    readHarnesses,
    getLocalModelsQuery,
    reportLocaleQuery,
  };
}
