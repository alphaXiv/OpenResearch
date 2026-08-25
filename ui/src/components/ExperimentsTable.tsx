import { CircleStop, FolderTree, GitBranch, Terminal } from "lucide-react";
import { useState } from "react";
import { runDisplayStatus, timeAgo, type Experiment, type Run } from "../api";
import { StatusBadge } from "./StatusBadge";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";
import { isTinkerRun, TinkerCancelDialog } from "./TinkerCancelDialog";

const EXPERIMENT_TABLE_ACTION_CLASS_NAME = [
  "experiment-table-action inline-flex items-center gap-1.5 py-1.5 px-2.5",
  "border border-border rounded-md bg-background text-text",
  "text-sm font-medium leading-none",
  "[&:hover:not(:disabled)]:bg-surface",
  "[&:hover:not(:disabled)]:border-border-strong [&:disabled]:text-muted",
  "[&:disabled]:cursor-default [&:disabled]:opacity-50",
  "[&.danger]:border-[color-mix(in_oklab,_var(--accent-red)_42%,_var(--border))]",
  "[&.danger]:bg-[color-mix(in_oklab,_var(--accent-red)_6%,_var(--base))]",
  "[&.danger]:text-accent-red [&.danger:hover:not(:disabled)]:border-accent-red",
  "[&.danger:hover:not(:disabled)]:bg-[color-mix(in_oklab,_var(--accent-red)_10%,_var(--base))]",
  "[@container((max-width:_560px))]:[&.danger]:ml-auto",
].join(" ");

export function ExperimentsTable({
  runs,
  experiments,
  emptyHint,
  onOpen,
  onOpenLogs,
  onOpenCode,
  onCancel,
}: {
  runs: Run[];
  experiments: Experiment[];
  emptyHint?: string;
  onOpen: (experiment: Experiment, intent: TabOpenIntent) => void;
  onOpenLogs: (experimentId: string, runId: string, intent: TabOpenIntent) => void;
  onOpenCode: (experimentId: string, intent: TabOpenIntent) => void;
  onCancel: (runId: string) => Promise<void>;
}) {
  const [pendingCancellation, setPendingCancellation] = useState<ReadonlySet<string>>(new Set());
  const [cancelError, setCancelError] = useState<string | null>(null);
  const [tinkerRun, setTinkerRun] = useState<Run | null>(null);
  const runsByExperiment = new Map<string, Run[]>();
  for (const run of runs) {
    const experimentRuns = runsByExperiment.get(run.experimentId);
    if (experimentRuns) experimentRuns.push(run);
    else runsByExperiment.set(run.experimentId, [run]);
  }
  for (const experimentRuns of runsByExperiment.values()) {
    experimentRuns.sort((a, b) => b.createdAt - a.createdAt);
  }

  const sortedExperiments = [...experiments].sort((a, b) => {
    const aActivity = runsByExperiment.get(a.id)?.[0]?.createdAt ?? a.createdAt;
    const bActivity = runsByExperiment.get(b.id)?.[0]?.createdAt ?? b.createdAt;
    return bActivity - aActivity;
  });

  if (sortedExperiments.length === 0) {
    return (
      <div className="empty-state absolute inset-0 flex flex-col items-center justify-center gap-2.5 p-6 text-center text-subtext [&_p]:max-w-[46ch] [&_p]:m-0 [&_p]:leading-normal [&_p]:text-balance [&_p.empty-state-title]:text-2xl [&_p.empty-state-title]:font-normal [&_p.empty-state-title]:text-text [&_p.empty-state-hint]:text-lg [&_p.empty-state-hint]:text-subtext experiments-empty-state [&_p]:text-2xl">
        <p>{emptyHint ?? "No experiments yet."}</p>
      </div>
    );
  }

  async function cancel(runId: string) {
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
      throw cause;
    }
  }

  function requestCancel(run: Run) {
    setCancelError(null);
    if (isTinkerRun(run)) {
      setTinkerRun(run);
      return;
    }
    void cancel(run.id).catch((cause) =>
      setCancelError(cause instanceof Error ? cause.message : String(cause)),
    );
  }

  return (
    <div className="experiments-table-wrap absolute inset-0 overflow-auto bg-background @container">
      {cancelError && (
        <div className="experiments-table-error py-2 px-3 text-accent-red text-sm border-b border-b-border" role="alert">
          Stop failed: {cancelError}
        </div>
      )}
      <div className="experiments-table w-full text-md bg-background" role="list" aria-label="Experiments">
        {sortedExperiments.map((experiment) => {
          const experimentRuns = runsByExperiment.get(experiment.id) ?? [];
          const latestRun = experimentRuns[0] ?? null;
          const liveRun = experimentRuns.find(
            (run) => run.status === "running" || run.status === "starting",
          );
          const logsRun = liveRun ?? latestRun;
          const cancelling = Boolean(
            liveRun && (liveRun.cancelRequested || pendingCancellation.has(liveRun.id)),
          );
          const status = liveRun
            ? cancelling
              ? "cancelling"
              : runDisplayStatus(liveRun)
            : latestRun
              ? runDisplayStatus(latestRun)
              : "idle";

          return (
            <div
              key={experiment.id}
              className="experiment-table-group grid grid-cols-[minmax(0,_1fr)_auto] [grid-template-areas:'name_meta'_'actions_actions'] gap-x-8 items-center py-4 px-5 gap-y-[7px] border-b border-b-[color-mix(in_oklab,_var(--text)_7%,_transparent)] bg-background cursor-pointer [&:hover]:bg-canvas [&:last-child]:border-b-0 [@container((max-width:_560px))]:grid-cols-[minmax(0,_1fr)_auto] [@container((max-width:_560px))]:gap-x-3.5 [@container((max-width:_560px))]:gap-y-[9px] [@container((max-width:_400px))]:grid-cols-[minmax(0,_1fr)] [@container((max-width:_400px))]:[grid-template-areas:'name'_'meta'_'actions']"
              role="listitem"
              onClick={() => onOpen(experiment, "preview")}
              onDoubleClick={() => onOpen(experiment, "keepOpen")}
              onAuxClick={(event) => {
                if (event.button !== 1) return;
                event.preventDefault();
                onOpen(experiment, "keepOpen");
              }}
            >
              <div className="experiment-table-name [grid-area:name] self-start min-w-0">
                <button
                  type="button"
                  className="experiment-table-title block w-full overflow-hidden text-text font-semibold text-left text-ellipsis whitespace-nowrap"
                  {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
                    onOpen(experiment, intent),
                  { stopPropagation: true })}
                >
                  {experiment.title || experiment.slug}
                </button>
                <span className="experiment-table-subtitle flex items-center min-w-0 gap-1.5 mt-1 overflow-hidden text-subtext text-sm [&_>_svg]:shrink-0 [&_code]:min-w-0 [&_code]:overflow-hidden [&_code]:text-ellipsis [&_code]:whitespace-nowrap" title={experiment.branchName}>
                  <GitBranch size={14} aria-hidden="true" />
                  <code>{experiment.branchName}</code>
                </span>
              </div>
              <div className="experiment-table-meta [grid-area:meta] self-start flex items-center justify-end gap-4.5 whitespace-nowrap [@container((max-width:_560px))]:flex-col [@container((max-width:_560px))]:items-end [@container((max-width:_560px))]:gap-1.5 [@container((max-width:_400px))]:!flex-row [@container((max-width:_400px))]:!items-center [@container((max-width:_400px))]:flex-wrap [@container((max-width:_400px))]:justify-start [@container((max-width:_400px))]:gap-3">
                <div className="experiment-table-status flex items-center min-w-0">
                  <StatusBadge status={status} />
                </div>
                <div className="experiment-run-summary flex items-center min-w-0 gap-2 text-subtext text-xs font-medium">
                  <span>
                    {experimentRuns.length} {experimentRuns.length === 1 ? "run" : "runs"}
                  </span>
                </div>
                <div className="experiment-table-latest flex items-center gap-1.5 min-w-0 text-subtext text-xs font-medium whitespace-nowrap">
                  <span>{latestRun ? timeAgo(latestRun.createdAt) : "Not run yet"}</span>
                </div>
              </div>
              <div
                className="experiment-table-actions [grid-area:actions] flex flex-wrap items-center justify-start gap-2 mt-3"
                role="group"
                aria-label={`Actions for ${experiment.title || experiment.slug}`}
                onClick={(event) => event.stopPropagation()}
                onDoubleClick={(event) => event.stopPropagation()}
                onAuxClick={(event) => event.stopPropagation()}
              >
                <button
                  className={EXPERIMENT_TABLE_ACTION_CLASS_NAME}
                  disabled={!logsRun}
                  title={logsRun ? "Open logs" : "No runs yet"}
                  {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => {
                    if (logsRun) onOpenLogs(experiment.id, logsRun.id, intent);
                  }, { stopPropagation: true })}
                >
                  <Terminal size={15} />
                  Logs
                </button>
                <button
                  className={EXPERIMENT_TABLE_ACTION_CLASS_NAME}
                  title={`Browse code on ${experiment.branchName}`}
                  {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
                    onOpenCode(experiment.id, intent),
                  { stopPropagation: true })}
                >
                  <FolderTree size={15} />
                  Code
                </button>
                {liveRun && (
                  <button
                    className="experiment-table-action inline-flex items-center gap-1.5 py-1.5 px-2.5 border border-border rounded-md bg-background text-text text-sm font-medium leading-none [&:hover:not(:disabled)]:bg-surface [&:hover:not(:disabled)]:border-border-strong [&:disabled]:text-muted [&:disabled]:cursor-default [&:disabled]:opacity-50 [&.danger]:border-[color-mix(in_oklab,_var(--accent-red)_42%,_var(--border))] [&.danger]:bg-[color-mix(in_oklab,_var(--accent-red)_6%,_var(--base))] [&.danger]:text-accent-red [&.danger:hover:not(:disabled)]:border-accent-red [&.danger:hover:not(:disabled)]:bg-[color-mix(in_oklab,_var(--accent-red)_10%,_var(--base))] [@container((max-width:_560px))]:[&.danger]:ml-auto danger"
                    disabled={cancelling}
                    title={cancelling ? "Stop requested" : "Stop run"}
                    onClick={() => requestCancel(liveRun)}
                  >
                    <CircleStop size={15} />
                    {cancelling ? "Stopping…" : "Stop"}
                  </button>
                )}
              </div>
            </div>
          );
        })}
      </div>
      {tinkerRun && (
        <TinkerCancelDialog run={tinkerRun} onCancel={cancel} onClose={() => setTinkerRun(null)} />
      )}
    </div>
  );
}
