import type { Experiment, Run } from "./api";

export function activeWorkspaceRuns(experiments: Experiment[], runs: Run[], sessionId: string | null) {
  if (!sessionId) return [];
  return runs.flatMap((run) => {
    if (run.status !== "starting" && run.status !== "running") return [];
    const experiment = experiments.find((item) => item.id === run.experimentId && item.chatSessionId === sessionId);
    return experiment ? [{ experiment, run }] : [];
  });
}
