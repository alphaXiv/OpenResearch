import { useQueries } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import type { Run } from "../../api";
import { monitorBehind } from "../../runMonitor/model";
import { useWorkspace } from "../../workspace";
import type { StripMonitor } from "./RunStrip";

/** How long a summary may run ahead of its monitor before the monitor is read again; its own event usually lands first. */
const CATCH_UP_MS = 2_000;

/**
 * Each run's whole live state, read while `reads` says so and again when the run's summary gets ahead of it. Null for
 * a run with no monitor to read, as against an older server. Across a generation the blocks wait for the reread
 * instead of drawing the old basis.
 */
export function useRunMonitors(runs: Run[], reads: (run: Run) => boolean): StripMonitor[] {
  const workspace = useWorkspace();
  const { getRunMonitorQuery } = workspace.queries;
  const queries = useQueries({ queries: runs.map((run) => ({ ...getRunMonitorQuery(run.id), enabled: !!run.monitor && reads(run) })) });

  const behind = runs.flatMap((run, index) => {
    const data = queries[index].data;
    if (!data || !reads(run) || !monitorBehind(run.monitor, data)) return [];
    const regenerated = run.monitor!.generation !== data.generation;
    // Keyed by the monitor's own revision, so later summaries never push its reread back.
    return [{ index, runId: run.id, regenerated, key: `${run.id}:${data.generation}:${data.rev}:${regenerated}` }];
  });

  const timers = useRef(new Map<string, number>());
  const behindKey = behind.map((entry) => entry.key).join(" ");
  useEffect(() => {
    for (const [key, timer] of timers.current) {
      if (behind.some((entry) => entry.key === key)) continue;
      window.clearTimeout(timer);
      timers.current.delete(key);
    }
    for (const { key, runId, regenerated } of behind) {
      if (timers.current.has(key)) continue;
      timers.current.set(key, window.setTimeout(() => {
        void workspace.queryClient.refetchQueries({ queryKey: getRunMonitorQuery(runId).queryKey, exact: true });
      }, regenerated ? 0 : CATCH_UP_MS));
    }
  }, [behindKey]);
  useEffect(() => () => timers.current.forEach((timer) => window.clearTimeout(timer)), []);

  return runs.map((run, index) => {
    if (!run.monitor) return null;
    const query = queries[index];
    const regenerated = behind.some((entry) => entry.index === index && entry.regenerated);
    const current = query.data && !regenerated ? query.data : null;
    if (query.isError) return { status: "error", retry: () => void query.refetch(), data: current };
    return current ? { status: "ready", data: current } : { status: "loading" };
  });
}

/** One run's whole live state, read for as long as it is shown. */
export const useRunMonitor = (run: Run): StripMonitor => useRunMonitors([run], () => true)[0];
