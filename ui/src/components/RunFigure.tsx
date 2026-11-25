import { useQuery } from "@tanstack/react-query";
import { createContext, useContext, type ReactNode } from "react";
import type { Run } from "../api";
import { autoDir } from "../i18n";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";
import { timeAgo } from "../api";
import { m } from "../paraglide/messages.js";
import { runActive } from "../runDisplay";
import { runTone } from "../runMonitor/model";
import type { ChartBlock, RunMonitor, ViewBlock } from "../runMonitor/types";
import { useWorkspace } from "../workspace";
import { FileScopeContext } from "./FileReference";
import { Block, BlockBoundary, drawsAt } from "./run/blocks/Block";
import { Chart } from "./run/blocks/Chart";
import { Legend } from "./run/blocks/Legend";
import { chartOf, seriesOf } from "./run/blocks/series";
import { useRunMonitors } from "./run/useRunMonitor";
import { Button } from "./ui";
import { cn } from "./ui/cn";

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

/**
 * Set inside a chat's status, where figures share a bounded box with text: each figure is one short chart named by its
 * experiments, and each run tag reads as a link on the result it labels.
 */
export const InStatus = createContext(false);

type OpenRun = (runId: string, intent: TabOpenIntent) => void;

/** Finds the run an id or its prefix names in the project, with its experiment's name, once both lists load. */
export function useCitedRuns(projectId: string | undefined) {
  const { listRunsQuery, listExperimentsQuery } = useWorkspace().queries;
  const runs = useQuery({ ...listRunsQuery(projectId ?? ""), enabled: !!projectId }).data;
  const experiments = useQuery({ ...listExperimentsQuery(projectId ?? ""), enabled: !!projectId }).data;
  return (id: string) => {
    const matches = runs?.filter((run) => run.id.startsWith(id));
    const run = matches?.length === 1 ? matches[0] : undefined;
    const experiment = run && experiments?.find((one) => one.id === run.experimentId);
    return { run, name: experiment?.title?.trim() || experiment?.slug || null };
  };
}

/** Text that opens a run's experiment, as a run chip opens the run; plain text without `onOpenRun`. */
function RunLink({ runId, name, onOpenRun, className, description }: {
  runId: string;
  name: string;
  onOpenRun?: OpenRun;
  className?: string;
  description?: string;
}) {
  if (!onOpenRun) return <span>{autoDir(name)}</span>;
  return (
    <button
      type="button"
      className={cn("min-w-0 rounded-xs text-start hover:underline focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:outline-offset-2", className)}
      aria-description={description}
      {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => onOpenRun(runId, intent))}
    >
      {autoDir(name)}
    </button>
  );
}

/** A run tag in a chat's status: the result its label states, else its experiment's name, linked to the experiment once it resolves. */
export function StatusRunLink({ id, label, onOpenRun }: { id: string; label?: string; onOpenRun?: OpenRun }) {
  const scope = useContext(FileScopeContext);
  const { run, name } = useCitedRuns(scope?.projectId)(id);
  const resolved = run && name;
  // Inline, so a line never breaks between the result and the punctuation after it.
  return (
    <RunLink
      runId={id}
      name={label || name || m.tree_experiment()}
      onOpenRun={resolved ? onOpenRun : undefined}
      className="file-link inline"
      description={resolved ? m.a11y_open_experiment({ name: autoDir(resolved) }) : undefined}
    />
  );
}

/**
 * A run against its parent on one chart of the signal it leads with, each line named by its experiment; without a
 * parent, the run's own first chart under its experiment's name. Only the lead signal compares, so two lines stay two.
 */
function CompactChart({ run, monitor, onOpenRun }: { run: Run; monitor: RunMonitor; onOpenRun?: OpenRun }) {
  const cited = useCitedRuns(run.projectId);
  const lead = monitor.headline;
  const parent = monitor.parent;
  const compared = lead !== null && parent !== null && (parent.series[lead]?.length ?? 0) > 0;
  const first = monitor.view.blocks[0];
  const drawsLead = monitor.view.blocks.find((one): one is ChartBlock & ViewBlock => "chart" in one && one.from === undefined && one.chart.includes(lead ?? ""));
  const block: ChartBlock | null = compared ? { chart: [lead], ghost: "parent", yScale: drawsLead?.yScale, xRange: drawsLead?.xRange } : first && "chart" in first ? first : null;
  if (!block) return null;
  const own = cited(run.id).name ?? m.tree_experiment();
  const parentRun = parent && cited(parent.runId);
  const title = compared ? monitor.view.signals[lead]?.label ?? lead : own;
  const series = seriesOf(monitor, block);
  const live = runActive(run.status) && monitor.availability.state === "live";

  return (
    <section aria-label={title} className="min-w-0 rounded-md border border-border px-3 py-2.5">
      <div className="mb-1.5 flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
        {/* Important, to win over the Markdown heading styles around it. */}
        <h3 className="m-0! min-w-0 text-sm font-medium! text-text!">
          {compared ? autoDir(title) : <RunLink runId={run.id} name={own} onOpenRun={onOpenRun} />}
        </h3>
        {compared ? (
          <ul className="m-0 flex list-none flex-wrap items-center gap-x-3.5 gap-y-1 p-0 text-xs text-ink-subtext">
            {series.map((line) => (
              <li key={line.name} className="inline-flex min-w-0 items-center gap-1.5">
                <svg width="14" height="2" className="shrink-0" aria-hidden="true"><line x1="0" x2="14" y1="1" y2="1" stroke={line.color} strokeWidth="2" strokeDasharray={line.ghost ? "4 3" : undefined} /></svg>
                {line.ghost ? (
                  <RunLink runId={parent.runId} name={parentRun?.name ?? m.run_chart_parent()} onOpenRun={parentRun?.run && onOpenRun} />
                ) : <RunLink runId={run.id} name={own} onOpenRun={onOpenRun} />}
              </li>
            ))}
          </ul>
        ) : <Legend series={series} row />}
      </div>
      <BlockBoundary title={title} rev={monitor.rev}>
        <Chart monitor={monitor} block={block} title={title} size="page" live={live} height={120} />
      </BlockBoundary>
    </section>
  );
}

/** A message cites only a run in its own project; its first block shares the page's live monitor. */
export function RunFigure({ id, onOpenRun, children }: { id: string; onOpenRun?: OpenRun; children: ReactNode }) {
  const workspace = useWorkspace();
  const scope = useContext(FileScopeContext);
  const runs = useQuery({ ...workspace.queries.listRunsQuery(scope?.projectId ?? ""), enabled: !!scope });
  const matches = runs.data?.filter((run) => run.id === id || run.id.startsWith(id));
  const run = matches?.length === 1 ? matches[0] : undefined;
  const [monitor] = useRunMonitors(run ? [run] : [], () => true);
  const data = monitor && monitor.status !== "loading" ? monitor.data : null;
  const failed = runs.isError || monitor?.status === "error";
  const block = data?.view.blocks[0];
  const compact = useContext(InStatus);
  const retry = (
    <div role="status" className="mt-2 flex items-center gap-2 text-xs text-ink-subtext">
      {m.run_monitor_load_failed()}
      <Button size="small" variant="ghost" onClick={() => {
        if (runs.isError) void runs.refetch();
        else if (monitor?.status === "error") monitor.retry();
      }}>{m.app_retry()}</Button>
    </div>
  );

  if (compact) {
    return (
      <div className="my-2.5 min-w-0">
        {failed ? retry : run && data && <CompactChart run={run} monitor={data} onOpenRun={onOpenRun} />}
      </div>
    );
  }

  return (
    <div className="my-2.5 min-w-0">
      {children}
      {failed ? (
        retry
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
