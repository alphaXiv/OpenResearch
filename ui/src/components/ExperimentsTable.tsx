import { WorkspaceEmptyState } from "./WorkspaceEmptyState";
import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { FlaskConical, CircleStop, FolderTree, Terminal } from "lucide-react";
import { useLayoutEffect, useMemo, useRef, useState } from "react";
import { fmtNumber, timeAgo, type Experiment, type Run } from "../api";
import { runActive, runDisplay } from "../runDisplay";
import { statusLabel, StatusBadge } from "./StatusBadge";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";
import { IconButton, Tooltip } from "./ui";
import { ArchiveMenu, type ArchiveActions } from "./ArchiveMenu";
import { Rail, type HistoryOrder, type Placed } from "./ExperimentLineage";
import { buildHistoryNodes, laneCount, layoutChronological, layoutLineage } from "./experimentLineageModel";

const NO_HIGHLIGHT = new Set<string>();

export function ExperimentsTable({
  runs,
  experiments,
  showLineage,
  order,
  archiveActions,
  emptyHint,
  onOpen,
  onOpenLogs,
  onOpenCode,
  onArchive,
  onDelete,
  onCancel,
}: {
  runs: Run[];
  experiments: Experiment[];
  showLineage: boolean;
  order: HistoryOrder;
  archiveActions: Map<string, ArchiveActions>;
  emptyHint?: string;
  onOpen: (experiment: Experiment, intent: TabOpenIntent) => void;
  onOpenLogs: (experimentId: string, runId: string, intent: TabOpenIntent) => void;
  onOpenCode: (experimentId: string, intent: TabOpenIntent) => void;
  onArchive: (id: string, direction: "ancestors" | "descendants" | "only", archived: boolean) => void;
  onDelete: (id: string) => void;
  onCancel: (runId: string) => Promise<void>;
}) {
  const [pendingCancellation, setPendingCancellation] = useState<ReadonlySet<string>>(new Set());
  const [cancelError, setCancelError] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [placed, setPlaced] = useState<Placed[]>([]);
  const rows = useMemo(() => {
    const group = { key: "table", title: "", nodes: buildHistoryNodes(experiments, runs) };
    return order === "lineage" ? layoutLineage(group) : layoutChronological(group, order === "newest");
  }, [experiments, runs, order]);
  const railWidth = 15 + 14 * laneCount(rows) + 8;
  const lastPlaced = placed.at(-1);

  useLayoutEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const elements = [...list.children].filter((child) => child instanceof HTMLDivElement);
    const measure = () => {
      setPlaced(rows.flatMap((row, index) => {
        const element = elements[index];
        if (!element) return [];
        const top = element.offsetTop;
        const height = element.offsetHeight;
        return [{ row, top, height, y: top + height / 2 }];
      }));
    };
    const observer = new ResizeObserver(measure);
    for (const element of elements) observer.observe(element);
    measure();
    return () => observer.disconnect();
  }, [rows]);
  const runsByExperiment = new Map<string, Run[]>();
  for (const run of runs) {
    const experimentRuns = runsByExperiment.get(run.experimentId);
    if (experimentRuns) experimentRuns.push(run);
    else runsByExperiment.set(run.experimentId, [run]);
  }
  for (const experimentRuns of runsByExperiment.values()) {
    experimentRuns.sort((a, b) => b.createdAt - a.createdAt);
  }

  if (rows.length === 0) {
    return (
      <WorkspaceEmptyState
        icon={FlaskConical}
        title={emptyHint ?? m.experiments_none_yet()}
        description={emptyHint ? undefined : m.experiments_empty_description()}
      />
    );
  }

  async function requestCancel(runId: string) {
    setCancelError(null);
    setPendingCancellation((current) => new Set(current).add(runId));
    try {
      await onCancel(runId);
    } catch (cause) {
      setPendingCancellation((current) => {
        const next = new Set(current);
        next.delete(runId);
        return next;
      });
      setCancelError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  return (
    <div className="experiments-table-wrap absolute inset-0 overflow-auto bg-background @container">
      {cancelError && (
        <div className="experiments-table-error py-2 px-3 text-accent-red text-sm border-b border-b-border" role="alert">
          {m.experiments_table_stop_failed()} {cancelError}
        </div>
      )}
      <div ref={listRef} data-keyboard-list className="experiments-table relative w-full text-sm bg-background" role="list" aria-label={m.experiments_table_experiments()}>
        {showLineage && <Rail placed={placed} height={lastPlaced ? lastPlaced.top + lastPlaced.height : 0} path={NO_HIGHLIGHT} selectedId={null} />}
        {rows.map((row) => {
          if (row.kind !== "experiment") return null;
          const experiment = row.node.experiment;
          const experimentRuns = runsByExperiment.get(experiment.id) ?? [];
          const latestRun = experimentRuns[0] ?? null;
          const liveRun = experimentRuns.find((run) => runActive(run.status));
          const logsRun = liveRun ?? latestRun;
          const cancelling = Boolean(liveRun && (liveRun.cancelling || pendingCancellation.has(liveRun.id)));
          const shownRun = liveRun && pendingCancellation.has(liveRun.id) ? { ...liveRun, cancelling: true } : logsRun;
          const display = shownRun ? runDisplay(shownRun) : { status: "idle" };

          return (
            <div
              key={experiment.id}
              style={{ paddingLeft: showLineage ? railWidth + 12 : 20 }}
              className="experiment-table-group grid min-h-11 grid-cols-[minmax(0,_1fr)_9.5rem_4rem_4rem_auto] [grid-template-areas:'name_status_attempts_latest_actions'] items-center gap-x-3 gap-y-1 py-2 pe-3 border-b border-b-divider-subtle bg-background cursor-pointer [&:hover]:bg-surface [&:last-child]:border-b-0 [@container(max-width:640px)]:grid-cols-[minmax(0,_1fr)_auto] [@container(max-width:640px)]:[grid-template-areas:'name_actions'_'meta_meta'] [@container(max-width:640px)]:gap-x-3"
              role="listitem"
              onClick={() => onOpen(experiment, "preview")}
              onDoubleClick={() => onOpen(experiment, "keepOpen")}
              onAuxClick={(event) => {
                if (event.button !== 1) return;
                event.preventDefault();
                onOpen(experiment, "keepOpen");
              }}
            >
              <div className="experiment-table-name [grid-area:name] min-w-0">
                <button
                  type="button"
                  data-keyboard-item
                  className="experiment-table-title line-clamp-2 w-full overflow-hidden text-text font-medium text-start"
                  {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
                    onOpen(experiment, intent),
                  { stopPropagation: true })}
                >
                  {experiment.title || experiment.slug}
                </button>
                {experiment.archived && <span className="text-muted text-xs">{m.tree_archived()}</span>}
              </div>
              {/* Columns when wide; one full-width line under the title when narrow, clear of the actions. */}
              <div className="experiment-table-meta contents [@container(max-width:640px)]:flex [@container(max-width:640px)]:[grid-area:meta] [@container(max-width:640px)]:min-w-0 [@container(max-width:640px)]:items-center [@container(max-width:640px)]:gap-x-3">
                <div className="experiment-table-status [grid-area:status] min-w-0">
                  <StatusBadge {...display} className="max-w-full" />
                </div>
                <div className="experiment-run-summary [grid-area:attempts] flex items-center gap-2 whitespace-nowrap text-sm text-subtext">
                  <span>{experimentRuns.length === 1 ? m.experiments_one_run() : m.experiments_run_count({ count: fmtNumber(experimentRuns.length) })}</span>
                </div>
                <div className="experiment-table-latest [grid-area:latest] min-w-0 truncate text-xs tabular-nums text-subtext">
                  {latestRun ? timeAgo(latestRun.createdAt) : "–"}
                </div>
              </div>
              <div
                className="experiment-table-actions [grid-area:actions] flex items-center justify-end gap-1"
                role="group"
                aria-label={m.a11y_actions_for({ name: experiment.title || experiment.slug })}
                onClick={(event) => event.stopPropagation()}
                onDoubleClick={(event) => event.stopPropagation()}
                onAuxClick={(event) => event.stopPropagation()}
              >
                <Tooltip interactive content={logsRun ? m.experiments_open_logs() : m.experiments_no_runs_yet()}>
                  <IconButton
                    size="small"
                    disabled={!logsRun}
                    aria-label={m.experiments_table_logs()}
                    {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => {
                      if (logsRun) onOpenLogs(experiment.id, logsRun.id, intent);
                    }, { stopPropagation: true })}
                  >
                    <Terminal size={16} />
                  </IconButton>
                </Tooltip>
                <Tooltip interactive content={m.experiments_table_code()}>
                  <IconButton
                    size="small"
                    aria-label={m.a11y_browse_code_on({ branch: ltr(experiment.branchName) })}
                    {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
                      onOpenCode(experiment.id, intent),
                    { stopPropagation: true })}
                  >
                    <FolderTree size={16} />
                  </IconButton>
                </Tooltip>
                {liveRun && (
                  <Tooltip interactive content={cancelling ? statusLabel(display.status) : m.experiments_stop_run()}>
                    <IconButton
                      size="small"
                      variant="stop"
                      disabled={cancelling}
                      aria-label={cancelling ? statusLabel(display.status) : m.common_stop()}
                      onClick={() => void requestCancel(liveRun.id)}
                    >
                      <CircleStop size={16} />
                    </IconButton>
                  </Tooltip>
                )}
                <Tooltip interactive content={m.tree_archive_manage()}>
                  <ArchiveMenu
                    compact
                    id={experiment.id}
                    name={experiment.title || experiment.slug}
                    actions={archiveActions.get(experiment.id)!}
                    onArchive={onArchive}
                    onDelete={onDelete}
                  />
                </Tooltip>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
