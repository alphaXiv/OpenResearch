import type { QueryClient } from "@tanstack/react-query";
import type { DiffPayload, EngineHost, Experiment, LocalMachine, UiState } from "../src/api";
import { createWorkspace } from "../src/workspace";
import { now, readError, sessionId, specimenGroups, type Specimen } from "../tests/fixtures/runMonitor/gallery";

const prefix = "/gallery-fixture/";
export const groups = specimenGroups.map((group, index) => ({
  ...group,
  id: String(index),
  specimens: group.specimens.map((specimen, row) => ({ ...specimen, id: `${index}-${row}` })),
}));
const specimens = new Map(groups.flatMap((group) => group.specimens.map((specimen) => [specimen.id, specimen])));

function shownRuns(specimen: Specimen) {
  return {
    source: specimen.oldServer ? "active" as const : "shown" as const,
    runs: specimen.runs.map(({ item }) => specimen.oldServer ? { ...item, run: { ...item.run, monitor: null } } : item),
    failedSessions: specimen.failedList ? [sessionId] : [],
  };
}

function preferences(specimen: Specimen): UiState {
  return {
    workspace: null, onboardingCompleted: true, tourCompleted: true,
    preferredAgent: null, nestedDelegation: false,
    opencodeFreeModels: false, opencodeReviewModel: null,
    favoriteModels: [], hiddenModels: [], hiddenAgents: [],
    modelSectionOrder: [], collapsedModelSections: [],
    runBanner: specimen.level ?? "strip",
  };
}

function experiments(specimen: Specimen): Experiment[] {
  const item = specimen.runs[0]?.item;
  if (!item || !specimen.lineage) return [];
  const experiment = {
    id: item.run.experimentId,
    projectId: item.run.projectId,
    slug: item.experimentSlug,
    branchName: item.experimentSlug,
    runCommand: "python train.py",
    agentStatus: "idle",
    createdAt: item.run.createdAt,
    updatedAt: item.run.updatedAt,
    archived: false,
  };
  return [
    { ...experiment, parentExperimentId: "parent" },
    { ...experiment, id: "parent", slug: "resnet-baseline", branchName: "resnet-baseline" },
  ];
}

function hosts(specimen: Specimen): EngineHost[] {
  return specimen.retryHost ? [{
    alias: "lab-gpu", root: "/runs", class: "key", status: "backoff",
    connectRequired: false, sinceMs: now - 4 * 60_000, nextRetryMs: now + 2 * 60_000,
    cause: "The SSH connection timed out.",
  }] : [];
}

const local = {
  hostname: "workstation", os: "macos", arch: "aarch64", chip: "Apple M3 Max", osName: "macOS 15.2", model: "MacBook Pro",
  cpuCount: 16, memBytes: 64 * 1024 ** 3, gpus: [],
} satisfies LocalMachine;

const diff = {
  diff: "diff --git a/train.py b/train.py\n--- a/train.py\n+++ b/train.py\n@@ -1 +1,2 @@\n-lr = 0.1\n+lr = 0.05\n+epochs = 90\n",
  truncated: false, bytesRead: 99, byteLimit: 100_000,
} satisfies DiffPayload;

export function galleryWorkspace(specimen: Specimen & { id: string }, client: QueryClient) {
  const workspace = createWorkspace(`run-gallery-${specimen.id}`, prefix + specimen.id, client);
  const { queries } = workspace;
  client.setQueryData(queries.getUiStateQuery().queryKey, preferences(specimen));
  client.setQueryData(queries.getLocalMachineQuery().queryKey, local);
  const listed = shownRuns(specimen);
  client.setQueryData(queries.listChatShownRunsQuery().queryKey, listed);

  for (const { item, monitor } of specimen.runs) {
    const { run } = item;
    client.setQueryData(queries.listExperimentsQuery(run.projectId).queryKey, experiments(specimen));
    if (specimen.read !== "loading" && specimen.read !== "error") {
      client.setQueryData(queries.getRunMonitorQuery(run.id).queryKey, monitor);
    }
    if (specimen.lineage === "diff") client.setQueryData(queries.getRunDiffQuery(run.id).queryKey, diff);
    const flag = run.monitor?.flag;
    const choice = specimen.choice;
    if (flag && choice) client.setQueryData(queries.flagChoiceQuery(run.id, flag.id).queryKey, {
      sending: "sending" in choice ? choice.sending : null,
      requests: {},
      error: "error" in choice ? choice.error : null,
    });
  }
  if (specimen.read === "refresh-error") {
    const query = queries.getRunMonitorQuery(specimen.runs[0].item.run.id);
    void client.prefetchQuery({ ...query, staleTime: 0 });
  }
  return workspace;
}

const error = (message: string, status = 503) => Response.json({ error: message }, { status });

/** The standalone page's transport. Any request outside the fixture endpoints stops here. */
export const galleryFetch: typeof fetch = async (input, init) => {
  const url = new URL(typeof input === "string" || input instanceof URL ? input : input.url, location.href);
  const [id, path] = url.pathname.slice(prefix.length).split(/(?=\/api\/)/, 2);
  const specimen = url.origin === location.origin && url.pathname.startsWith(prefix) ? specimens.get(id) : undefined;
  if (!specimen) return error("This request is outside the run gallery.", 404);

  const method = init?.method ?? "GET";
  if (path === "/api/settings/ui-state") {
    const state = preferences(specimen);
    if (method === "POST") {
      const patch = JSON.parse(String(init?.body)) as Pick<UiState, "runBanner">;
      return Response.json({ ...state, runBanner: patch.runBanner });
    }
    return Response.json(state);
  }
  if (method !== "GET") return error("Gallery runs are fixtures. This action cannot change a run.", 409);
  if (path === "/api/chat/shown-runs") {
    const { runs, failedSessions } = shownRuns(specimen);
    return specimen.oldServer ? error("Shown runs are unavailable.", 404) : Response.json({ runs, failedSessions });
  }
  if (path === "/api/chat/active-runs") return Response.json(shownRuns(specimen).runs);
  if (path === "/api/compute/hosts") return Response.json(hosts(specimen));
  if (path === "/api/settings/local") return Response.json(local);
  if (path?.endsWith("/experiments")) return Response.json({ experiments: experiments(specimen) });

  const [, runId, kind] = /^\/api\/runs\/([^/]+)\/(monitor|diff)$/.exec(path ?? "") ?? [];
  const run = specimen.runs.find(({ item }) => item.run.id === runId);
  if (!run) return error("This endpoint has no gallery fixture.", 404);
  if (kind === "diff") return specimen.lineage === "error" ? error("Changes could not be read.") : Response.json(diff);
  if (specimen.read === "loading") {
    const signal = init?.signal;
    signal?.throwIfAborted();
    return new Promise<Response>((_, reject) => signal?.addEventListener("abort", () => reject(signal.reason), { once: true }));
  }
  return specimen.read === "error" || specimen.read === "refresh-error"
    ? error(readError)
    : Response.json(run.monitor);
};
