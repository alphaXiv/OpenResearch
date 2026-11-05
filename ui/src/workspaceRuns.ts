import type { Experiment, Run } from "./api";
import { runActive } from "./runDisplay.ts";

export function activeWorkspaceRuns(experiments: Experiment[], runs: Run[], sessionId: string | null) {
  if (!sessionId) return [];
  return runs.flatMap((run) => {
    if (!runActive(run.status)) return [];
    const experiment = experiments.find((item) => item.id === run.experimentId && item.chatSessionId === sessionId);
    return experiment ? [{ experiment, run }] : [];
  });
}
