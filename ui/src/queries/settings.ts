import type { WorkspaceEvents } from "../workspace";
import { upsertEngineHost } from "../engineHosts";
import type { Api } from "../api";
import type { WorkspaceClient } from "./client";
import { readLiveSnapshot } from "./live";
import { mutations } from "./mutations";
import type { Workspace } from "../workspace";
import { queryOptions, useQuery } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import type * as api from "../api";

export function createSettingsQueries(client: WorkspaceClient, api: Api, events: WorkspaceEvents) {
  const { workspaceKey, queryClient, isCurrentScope, setScopedQueryData } = client;

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

  const getOpenResearchAccountQuery = () => queryOptions({
    queryKey: workspaceKey("getOpenResearchAccount"),
    queryFn: ({ signal }) => api.getOpenResearchAccount(signal),
  });

  const useOpenResearchAccount = () => useQuery(getOpenResearchAccountQuery(), queryClient);

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
    queryKey: workspaceKey("getRuntime"),
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

  const getPendingWorkQuery = () => queryOptions({
    queryKey: workspaceKey("getPendingWork"),
    queryFn: ({ signal }) => api.getPendingWork(signal),
    refetchInterval: 1_000,
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

  const listAcpHarnessesQuery = () => queryOptions({
    queryKey: workspaceKey("getAcpHarnesses"),
    queryFn: () => api.listAcpHarnesses(),
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

  const getOpenCodeProvidersQuery = () => queryOptions({
    queryKey: workspaceKey("getOpenCodeProviders"),
    queryFn: ({ signal }) => api.getOpenCodeProviders(signal),
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
    getComputeSettingsQuery,
    getLocalMachineQuery,
    getPublicNetworkQuery,
    getOpenResearchSettingsQuery,
    getOpenResearchAccountQuery,
    useOpenResearchAccount,
    useAlphaXivFolders,
    useAlphaXivMe,
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
    getPendingWorkQuery,
    getBuildNotesQuery,
    getLitSourcesQuery,
    getProjectDefaultsQuery,
    getProjectGitStatusQuery,
    getTelemetryQuery,
    getHarnessSetupCommandsQuery,
    getHarnessesQuery,
    getSkillsQuery,
    getSkillContentQuery,
    listAcpHarnessesQuery,
    listLatexTemplatesQuery,
    listUserSkillsQuery,
    refreshHarnesses,
    refreshHarness,
    readHarnesses,
    getLocalModelsQuery,
    getOpenCodeProvidersQuery,
    reportLocaleQuery,
  };
}

type UiStatePatch = Parameters<Api["updateUiState"]>[0];

/** A field written again before this reply lands keeps the newer value. */
export const mutUpdateUiState = mutations.define({
  id: "settings/ui-state",
  mutate: (ws: Workspace, changes: UiStatePatch) => ws.api.updateUiState(changes),
  optimistic({ helpers, args: [ws, changes], onRestore, onSuccess }) {
    const options = ws.queries.getUiStateQuery();
    const before = ws.queryClient.getQueryData(options.queryKey);
    if (!before) return;
    const fields = Object.keys(changes) as (keyof UiStatePatch)[];
    ws.setScopedQueryData(options.queryKey, (state) => state && { ...state, ...changes });
    void ws.queryClient.cancelQueries({ queryKey: options.queryKey, exact: true });
    helpers(ws.queryClient).refetchOnSettled(options);
    const shown = ws.queryClient.getQueryData(options.queryKey);
    onRestore(() => ws.setScopedQueryData(options.queryKey, (state) => state && {
      ...state, ...Object.fromEntries(fields.map((field) => [field, before[field]])),
    }));
    onSuccess((saved) => ws.setScopedQueryData(options.queryKey, (state) => {
      if (!state || fields.some((field) => state[field] !== shown?.[field])) return state;
      return { ...state, ...Object.fromEntries(fields.map((field) => [field, saved[field]])) };
    }));
  },
  key: ({ args: [ws] }) => [ws.identity, "ui-state"],
  describe: "save your preferences",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSaveK8sSettings = mutations.define({
  id: "settings/k8s",
  mutate: (ws: Workspace, settings: Parameters<Api["saveK8sSettings"]>[0]) => ws.api.saveK8sSettings(settings),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess((saved) => ws.setScopedQueryData(ws.queries.getK8sSettingsQuery().queryKey, saved));
  },
  describe: "save the Kubernetes settings",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSaveModalToken = mutations.define({
  id: "settings/modal",
  mutate: (ws: Workspace, tokenId: string, tokenSecret: string) => ws.api.saveModalToken(tokenId, tokenSecret),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess((saved) => ws.setScopedQueryData(ws.queries.getModalSettingsQuery().queryKey, saved));
  },
  describe: "save the Modal token",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSaveVercelSetup = mutations.define({
  id: "settings/vercel",
  mutate: (ws: Workspace, setup: Parameters<Api["saveVercelSetup"]>[0]) => ws.api.saveVercelSetup(setup),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess((saved) => ws.setScopedQueryData(ws.queries.getVercelSettingsQuery().queryKey, saved));
  },
  describe: "save the Vercel settings",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSaveSlurmSettings = mutations.define({
  id: "settings/slurm",
  mutate: (ws: Workspace, settings: Parameters<Api["saveSlurmSettings"]>[0]) => ws.api.saveSlurmSettings(settings),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess((saved) => ws.setScopedQueryData(ws.queries.getSlurmSettingsQuery().queryKey, saved));
  },
  describe: "save the Slurm settings",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSaveRaySettings = mutations.define({
  id: "settings/ray",
  mutate: (ws: Workspace, settings: Parameters<Api["saveRaySettings"]>[0]) => ws.api.saveRaySettings(settings),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess((saved) => ws.setScopedQueryData(ws.queries.getRaySettingsQuery().queryKey, saved));
  },
  describe: "save the Ray settings",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSaveTinkerKey = mutations.define({
  id: "settings/tinker",
  mutate: (ws: Workspace, key: string) => ws.api.saveTinkerKey(key),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess((saved) => ws.setScopedQueryData(ws.queries.getTinkerSettingsQuery().queryKey, saved));
  },
  describe: "save the Tinker key",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSaveHfToken = mutations.define({
  id: "settings/hf",
  mutate: (ws: Workspace, token: string) => ws.api.saveHfToken(token),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess((saved) => ws.setScopedQueryData(ws.queries.getHfSettingsQuery().queryKey, saved));
  },
  describe: "save the Hugging Face token",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSetComputeDefault = mutations.define({
  id: "settings/compute-default",
  mutate: (ws: Workspace, choice: Parameters<Api["setComputeDefault"]>[0]) => ws.api.setComputeDefault(choice),
  optimistic({ helpers, args: [ws, { backend, flavor = null, projectId }], onSuccess }) {
    const options = ws.queries.getComputeSettingsQuery(projectId);
    helpers(ws.queryClient).updateExisting(options, (settings) => ({
      ...settings,
      configuredDefaultBackend: backend,
      configuredDefaultFlavor: flavor,
      defaultFlavor: flavor,
    }));
    onSuccess((saved) => ws.setScopedQueryData(options.queryKey, saved));
  },
  key: ({ args: [ws, { projectId }] }) => [ws.identity, "compute-default", projectId ?? ""],
  describe: "change where new runs go",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutRetryEngineHost = mutations.define({
  id: "settings/engine-host-retry",
  mutate: (ws: Workspace, alias: string) => ws.api.retryEngineHost(alias),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess(ws.queries.setEngineHost);
  },
  describe: "retry the host",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSetEnvVar = mutations.define({
  id: "settings/env-set",
  mutate: (ws: Workspace, key: string, value: string) => ws.api.setEnvVar(key, value),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess((vars) => ws.setScopedQueryData(ws.queries.getEnvVarsQuery().queryKey, vars));
  },
  key: ({ args: [ws] }) => [ws.identity, "env"],
  describe: "save the variable",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutDeleteEnvVar = mutations.define({
  id: "settings/env-delete",
  mutate: (ws: Workspace, key: string) => ws.api.deleteEnvVar(key),
  optimistic({ helpers, args: [ws, key], onSuccess }) {
    const options = ws.queries.getEnvVarsQuery();
    helpers(ws.queryClient).arrayFilter(options, (entry) => entry.key !== key);
    onSuccess((vars) => ws.setScopedQueryData(options.queryKey, vars));
  },
  key: ({ args: [ws] }) => [ws.identity, "env"],
  describe: "delete the variable",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutApplyUpdate = mutations.define({
  id: "update/apply",
  mutate: (ws: Workspace) => ws.api.applyUpdate(),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess((status) => ws.setScopedQueryData(ws.queries.getUpdateStatusQuery().queryKey, status));
  },
  key: ({ args: [ws] }) => [ws.identity, "update"],
  describe: "check for updates",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSetAutoUpdate = mutations.define({
  id: "update/auto",
  mutate: (ws: Workspace, enabled: boolean) => ws.api.setAutoUpdate(enabled),
  optimistic({ helpers, args: [ws, enabled], onSuccess }) {
    const options = ws.queries.getUpdateStatusQuery();
    helpers(ws.queryClient).objSet(options, ["autoUpdate"], enabled);
    onSuccess((status) => ws.setScopedQueryData(options.queryKey, status));
  },
  key: ({ args: [ws] }) => [ws.identity, "update"],
  describe: "change automatic updates",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSetReleaseChannel = mutations.define({
  id: "update/channel",
  mutate: (ws: Workspace, channel: api.ReleaseChannel) => ws.api.setReleaseChannel(channel),
  optimistic({ helpers, args: [ws, channel], onSuccess }) {
    const options = ws.queries.getUpdateStatusQuery();
    helpers(ws.queryClient).objSet(options, ["releaseChannel"], channel);
    onSuccess((status) => ws.setScopedQueryData(options.queryKey, status));
  },
  key: ({ args: [ws] }) => [ws.identity, "update"],
  describe: "change the update channel",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutRestartApp = mutations.define({
  id: "update/restart",
  mutate: (ws: Workspace, stopAgents: boolean) => ws.api.restartApp(stopAgents),
  optimistic: () => {},
  describe: "restart OpenResearch",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutInstallCli = mutations.define({
  id: "update/install-cli",
  mutate: (ws: Workspace, force: boolean) => ws.api.installCli(force),
  optimistic: () => {},
  describe: "install the command-line tool",
  describeResult: null,
});

export const mutSetTelemetry = mutations.define({
  id: "settings/telemetry",
  mutate: (ws: Workspace, enabled: boolean) => ws.api.setTelemetry(enabled),
  optimistic({ helpers, args: [ws, enabled], onSuccess }) {
    const options = ws.queries.getTelemetryQuery();
    helpers(ws.queryClient).updateExisting(options, (settings) => ({ ...settings, enabled, preferenceEnabled: enabled }));
    onSuccess((saved) => ws.setScopedQueryData(options.queryKey, saved));
  },
  key: ({ args: [ws] }) => [ws.identity, "telemetry"],
  describe: "change usage analytics",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSetGithubForNewProjects = mutations.define({
  id: "settings/projects-github",
  mutate: (ws: Workspace, enabled: boolean) => ws.api.setProjectDefaults(enabled, true),
  optimistic({ helpers, args: [ws, enabled], onSuccess }) {
    const options = ws.queries.getProjectDefaultsQuery();
    helpers(ws.queryClient).updateExisting(options, (settings) => ({ ...settings, githubForNewProjects: enabled, githubDefaultPromptSeen: true }));
    onSuccess((saved) => ws.setScopedQueryData(options.queryKey, saved));
  },
  key: ({ args: [ws] }) => [ws.identity, "project-defaults"],
  describe: "change GitHub syncing for new projects",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSetLitSources = mutations.define({
  id: "settings/lit-sources",
  mutate: (ws: Workspace, sources: api.LitSourcesSettings) => ws.api.setLitSources(sources),
  optimistic({ helpers, args: [ws, sources], onSuccess }) {
    const options = ws.queries.getLitSourcesQuery();
    helpers(ws.queryClient).set(options, sources);
    onSuccess((saved) => ws.setScopedQueryData(options.queryKey, saved));
  },
  key: ({ args: [ws] }) => [ws.identity, "lit-sources"],
  describe: "change literature sources",
  describeResult: null,
  refetchOnSuccess: false,
});

function showGitResult(ws: Workspace, projectId: string, result: { project: api.Project; git: api.ProjectGitStatus }) {
  ws.setScopedQueryData(ws.queries.getProjectGitStatusQuery(projectId).queryKey, result.git);
  ws.setScopedQueryData(ws.queries.listProjectsQuery().queryKey, (projects) =>
    projects?.map((project) => project.id === result.project.id ? result.project : project));
}

export const mutInitializeProjectGit = mutations.define({
  id: "project/git-init",
  mutate: (ws: Workspace, projectId: string) => ws.api.initializeProjectGit(projectId),
  optimistic({ args: [ws, projectId], onSuccess }) {
    onSuccess((git) => ws.setScopedQueryData(ws.queries.getProjectGitStatusQuery(projectId).queryKey, git));
  },
  describe: "initialize Git",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutEnableProjectGithub = mutations.define({
  id: "project/github-enable",
  mutate: (ws: Workspace, projectId: string, organization?: string) => ws.api.enableProjectGithub(projectId, organization),
  optimistic({ args: [ws, projectId], onSuccess }) {
    onSuccess((result) => showGitResult(ws, projectId, result));
  },
  key: ({ args: [ws, projectId] }) => [ws.identity, "github", projectId],
  describe: "turn on GitHub syncing",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutDisableProjectGithub = mutations.define({
  id: "project/github-disable",
  mutate: (ws: Workspace, projectId: string) => ws.api.disableProjectGithub(projectId),
  optimistic({ args: [ws, projectId], onSuccess }) {
    onSuccess((result) => showGitResult(ws, projectId, result));
  },
  key: ({ args: [ws, projectId] }) => [ws.identity, "github", projectId],
  describe: "turn off GitHub syncing",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSaveSshDefault = mutations.define({
  id: "settings/ssh-default",
  mutate: (ws: Workspace, host: string | null) => ws.api.saveSshDefault(host),
  optimistic({ helpers, args: [ws, host] }) {
    helpers(ws.queryClient).objSet(ws.queries.getSshSettingsQuery(), ["defaultHost"], host);
  },
  key: ({ args: [ws] }) => [ws.identity, "ssh-default"],
  describe: "change the default SSH host",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSaveSshHost = mutations.define({
  id: "settings/ssh-host",
  mutate: (ws: Workspace, host: string, container: string | null) => ws.api.saveSshHost({ host, container }),
  optimistic({ helpers, args: [ws, host, container] }) {
    helpers(ws.queryClient).objArrayUpdate(ws.queries.getSshSettingsQuery(), ["hosts"], {
      filter: (entry) => entry.host === host,
      update: (entry) => ({ ...entry, container }),
    });
  },
  key: ({ args: [ws, host] }) => [ws.identity, "ssh-host", host],
  describe: "save the host's settings",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSaveSshConfig = mutations.define({
  id: "settings/ssh-config",
  mutate: (ws: Workspace, content: string, previousContent: string) => ws.api.saveSshConfig(content, previousContent),
  optimistic({ args: [ws, content], onSuccess }) {
    onSuccess(() => ws.setScopedQueryData(ws.queries.getSshConfigQuery().queryKey, (file) => file && { ...file, content }));
  },
  describe: "save the SSH config",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSaveAcpHarness = mutations.define({
  id: "settings/acp-save",
  mutate: (ws: Workspace, launch: Omit<api.AcpDefinition, "id">, id?: api.AcpDefinition["id"]) => ws.api.saveAcpHarness(launch, id),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess((saved) => ws.setScopedQueryData(ws.queries.listAcpHarnessesQuery().queryKey, (definitions) =>
      definitions && [...definitions.filter((definition) => definition.id !== saved.id), saved]));
  },
  describe: "save the agent",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutRemoveAcpHarness = mutations.define({
  id: "settings/acp-remove",
  mutate: (ws: Workspace, id: api.AcpDefinition["id"]) => ws.api.removeAcpHarness(id),
  optimistic({ helpers, args: [ws, id] }) {
    helpers(ws.queryClient).arrayFilter(ws.queries.listAcpHarnessesQuery(), (definition) => definition.id !== id);
    helpers(ws.queryClient).arrayFilter(ws.queries.getHarnessesQuery(), (harness) => harness.id !== id);
  },
  describe: "remove the agent",
  describeResult: null,
});

export const mutRemoveLocalModel = mutations.define({
  id: "settings/local-model-remove",
  mutate: (ws: Workspace, id: string) => ws.api.removeLocalModel(id),
  optimistic({ helpers, args: [ws, id] }) {
    helpers(ws.queryClient).arrayRemove(ws.queries.getLocalModelsQuery(), (connection) => connection.id === id);
  },
  describe: "disconnect the model",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutUploadUserSkill = mutations.define({
  id: "skills/upload",
  mutate: (ws: Workspace, file: Parameters<Api["uploadUserSkill"]>[0]) => ws.api.uploadUserSkill(file),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess((skill) => showUserSkill(ws, skill));
  },
  describe: "upload the skill",
  describeResult: null,
});

export const mutUpdateUserSkill = mutations.define({
  id: "skills/update",
  mutate: (ws: Workspace, name: string, content: string) => ws.api.updateUserSkill(name, content),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess((skill) => showUserSkill(ws, skill));
  },
  key: ({ args: [ws, name] }) => [ws.identity, "skill", name],
  describe: "save the skill",
  describeResult: null,
});

function showUserSkill(ws: Workspace, skill: api.UserSkill) {
  ws.setScopedQueryData(ws.queries.listUserSkillsQuery().queryKey, (list) =>
    list && { ...list, skills: [...list.skills.filter((item) => item.name !== skill.name), skill].sort((a, b) => a.name.localeCompare(b.name)) });
}

export const mutDeleteUserSkill = mutations.define({
  id: "skills/delete",
  mutate: (ws: Workspace, name: string) => ws.api.deleteUserSkill(name),
  optimistic({ helpers, args: [ws, name] }) {
    helpers(ws.queryClient).objArrayFilter(ws.queries.listUserSkillsQuery(), ["skills"], (skill) => skill.name !== name);
  },
  key: ({ args: [ws, name] }) => [ws.identity, "skill", name],
  describe: "delete the skill",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutUploadLatexTemplate = mutations.define({
  id: "templates/upload",
  mutate: (ws: Workspace, file: Parameters<Api["uploadLatexTemplate"]>[0]) => ws.api.uploadLatexTemplate(file),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess((template) => ws.setScopedQueryData(ws.queries.listLatexTemplatesQuery().queryKey, (templates) =>
      templates && [...templates.filter((item) => item.name !== template.name), template].sort((a, b) => a.name.localeCompare(b.name))));
  },
  describe: "upload the template",
  describeResult: null,
});

export const mutDeleteLatexTemplate = mutations.define({
  id: "templates/delete",
  mutate: (ws: Workspace, name: string) => ws.api.deleteLatexTemplate(name),
  optimistic({ helpers, args: [ws, name] }) {
    helpers(ws.queryClient).arrayRemove(ws.queries.listLatexTemplatesQuery(), (template) => template.name === name);
  },
  describe: "delete the template",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSignOutOpenResearch = mutations.define({
  id: "openresearch/sign-out",
  async mutate(ws: Workspace) {
    await ws.api.logoutOpenResearch();
  },
  optimistic({ helpers, args: [ws], onSuccess }) {
    helpers(ws.queryClient).set(ws.queries.getOpenResearchAccountQuery(), { loggedIn: false, apiUrl: null, user: null });
    onSuccess(() => {
      for (const family of ["alphaXivFolders", "alphaXivMe", "alphaXivPapers"]) ws.queryClient.removeQueries({ queryKey: [...ws.workspaceScope(), family] });
    });
  },
  describe: "sign out",
  describeResult: null,
  refetchOnSuccess: false,
});

/** Settles once the settings show the new key, so the step never offers to add it twice. */
export const mutAddOpenResearchSshKey = mutations.define({
  id: "openresearch/ssh-key",
  async mutate(ws: Workspace) {
    await ws.api.addOpenResearchSshKey();
    await ws.queryClient.invalidateQueries(ws.queries.getOpenResearchSettingsQuery());
  },
  optimistic: () => {},
  describe: "add the SSH key",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutCompleteOnboarding = mutations.define({
  id: "onboarding/complete",
  mutate: (ws: Workspace, selection: api.OnboardingSelection) => ws.api.completeOnboarding(selection),
  optimistic: () => {},
  describe: "finish setup",
  describeResult: null,
});

export const mutInstallGit = mutations.define({
  id: "onboarding/install-git",
  mutate: (ws: Workspace) => ws.api.installGit(),
  optimistic: () => {},
  describe: "install Git",
  describeResult: null,
});

export const mutCreateRemoteSession = mutations.define({
  id: "remote/create",
  mutate: (ws: Workspace, host: string, uiPreferences: Parameters<Api["createRemoteSession"]>[1]) => ws.api.createRemoteSession(host, uiPreferences),
  optimistic: () => {},
  describe: "connect to the machine",
  describeResult: null,
});

export const mutInstallRemote = mutations.define({
  id: "remote/install",
  mutate: (ws: Workspace, paths: api.RemoteInstallPaths) => ws.api.installRemoteSession(paths),
  optimistic: () => {},
  describe: "install OpenResearch on the machine",
  describeResult: null,
});

export const mutReconnectRemote = mutations.define({
  id: "remote/reconnect",
  mutate: (ws: Workspace) => ws.api.reconnectCurrentRemote(),
  optimistic: () => {},
  describe: "reconnect",
  describeResult: null,
});

export const mutDisconnectRemote = mutations.define({
  id: "remote/disconnect",
  mutate: (ws: Workspace) => ws.api.disconnectCurrentRemote(),
  optimistic: () => {},
  describe: "disconnect",
  describeResult: null,
});

export const mutStartRemoteHost = mutations.define({
  id: "remote/start-host",
  mutate: (ws: Workspace) => ws.api.startCurrentRemoteHost(),
  optimistic: () => {},
  describe: "start the remote host",
  describeResult: null,
});

export const mutStopRemoteHost = mutations.define({
  id: "remote/stop-host",
  async mutate(ws: Workspace, preview: api.RemoteStopPreview) {
    await ws.api.stopCurrentRemoteHost(preview);
  },
  optimistic: () => {},
  describe: "stop the remote host",
  describeResult: null,
});
