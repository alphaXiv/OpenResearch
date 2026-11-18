import type { Experiment, Run } from "./api";
import { runActive } from "./runDisplay.ts";

export function activeWorkspaceRuns(experiments: Experiment[], runs: Run[], sessionId: string | null) {
  if (!sessionId) return [];
  return runs.flatMap((run) => {
    // An ended run with an open flag stays until a newer run of its experiment takes its place, as above the composer.
    const newest = !runs.some((other) => other.experimentId === run.experimentId && other.createdAt > run.createdAt);
    if (!runActive(run.status) && !(run.monitor?.flag && newest)) return [];
    const experiment = experiments.find((item) => item.id === run.experimentId && item.chatSessionId === sessionId);
    return experiment ? [{ experiment, run }] : [];
  });
}
