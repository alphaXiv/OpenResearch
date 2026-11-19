import { useQuery } from "@tanstack/react-query";
import { useContext, type ReactNode } from "react";
import { timeAgo } from "../api";
import { m } from "../paraglide/messages.js";
import { runActive } from "../runDisplay";
import { runTone } from "../runMonitor/model";
import type { RunMonitor, ViewBlock } from "../runMonitor/types";
import { useWorkspace } from "../workspace";
import { FileScopeContext } from "./FileReference";
import { Block, drawsAt } from "./run/blocks/Block";
import { chartOf } from "./run/blocks/series";
import { useRunMonitors } from "./run/useRunMonitor";
import { Button } from "./ui";

function hasData(block: ViewBlock, monitor: RunMonitor): boolean {
  if ("chart" in block) return chartOf(monitor, block).series.some((series) => !series.ghost && series.points.some(([, y]) => y !== null));
  if ("stats" in block) return block.stats.some((name) => monitor.latest[name] != null || monitor.best?.[name] != null);
  if ("table" in block) return !!monitor.rows[block.table]?.length;
  if ("bars" in block) {
    const groups = monitor.groups[block.bars]?.[block.by];
    return Array.isArray(groups) && groups.length > 0;
  }
  if ("cells" in block) return Object.hasOwn(monitor.rows, block.cells) && !!monitor.rows[block.cells].length;
  if ("media" in block) return true;
  return "text" in block && !!block.text;
}

/** A message cites only a run in its own project; its first block shares the page's live monitor. */
export function RunFigure({ id, children }: { id: string; children: ReactNode }) {
  const workspace = useWorkspace();
  const scope = useContext(FileScopeContext);
  const runs = useQuery({ ...workspace.queries.listRunsQuery(scope?.projectId ?? ""), enabled: !!scope });
  const matches = runs.data?.filter((run) => run.id === id || run.id.startsWith(id));
  const run = matches?.length === 1 ? matches[0] : undefined;
  const [monitor] = useRunMonitors(run ? [run] : [], () => true);
  const data = monitor && monitor.status !== "loading" ? monitor.data : null;
  const failed = runs.isError || monitor?.status === "error";
  const block = data?.view.blocks[0];

  return (
    <div className="my-2.5 min-w-0">
      {children}
      {failed ? (
        <div role="status" className="mt-2 flex items-center gap-2 text-xs text-ink-subtext">
          {m.run_monitor_load_failed()}
          <Button size="small" variant="ghost" onClick={() => {
            if (runs.isError) void runs.refetch();
            else if (monitor?.status === "error") monitor.retry();
          }}>{m.app_retry()}</Button>
        </div>
      ) : run && data && block && drawsAt(block, "page") && hasData(block, data) ? (
        <div className="mt-2 min-w-0 rounded-md border border-border p-4">
          {data.availability.state !== "live" && data.availability.reason && (
            <p className="m-0 mb-2 text-xs text-ink-subtext">{data.availability.reason} · {timeAgo(data.asOf)}</p>
          )}
          <Block monitor={data} block={block} size="page" tone={runTone(run, data)} live={runActive(run.status) && data.availability.state === "live"} />
        </div>
      ) : null}
    </div>
  );
}
