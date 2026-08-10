import { CircleStop, FolderTree, GitBranch, Terminal } from "lucide-react";
import { useState } from "react";
import { runDisplayStatus, timeAgo, type Experiment, type Run } from "../api";
import { StatusBadge } from "./StatusBadge";

const EXPERIMENT_TABLE_ACTION_CLASS_NAME = [
  "experiment-table-action [display:inline-flex] [align-items:center] [gap:6px] [padding:6px_10px]",
  "[border:1px_solid_var(--border)] [border-radius:var(--radius-md)] [background:var(--base)] [color:var(--text)]",
  "[font-size:var(--fs-sm)] [font-weight:var(--fw-medium)] [line-height:1]",
  "[&:hover:not(:disabled)]:[background:var(--surface)]",
  "[&:hover:not(:disabled)]:[border-color:var(--border-strong)] [&:disabled]:[color:var(--muted)]",
  "[&:disabled]:[cursor:default] [&:disabled]:[opacity:0.5]",
  "[&.danger]:[border-color:color-mix(in_oklab,_var(--accent-red)_42%,_var(--border))]",
  "[&.danger]:[background:color-mix(in_oklab,_var(--accent-red)_6%,_var(--base))]",
  "[&.danger]:[color:var(--accent-red)] [&.danger:hover:not(:disabled)]:[border-color:var(--accent-red)]",
  "[&.danger:hover:not(:disabled)]:[background:color-mix(in_oklab,_var(--accent-red)_10%,_var(--base))]",
  "[@container((max-width:_560px))]:[&.danger]:[margin-left:auto]",
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
  onOpen: (experiment: Experiment) => void;
  onOpenLogs: (experimentId: string, runId: string) => void;
  onOpenCode: (experimentId: string) => void;
  onCancel: (runId: string) => Promise<void>;
}) {
  const [pendingCancellation, setPendingCancellation] = useState<ReadonlySet<string>>(new Set());
  const [cancelError, setCancelError] = useState<string | null>(null);
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
      <div className="empty-state [position:absolute] [inset:0] [display:flex] [flex-direction:column] [align-items:center] [justify-content:center] [gap:10px] [padding:24px] [text-align:center] [color:var(--subtext)] [&_p]:[max-width:46ch] [&_p]:[margin:0] [&_p]:[line-height:1.5] [&_p]:[text-wrap:balance] [&_p.empty-state-title]:[font-size:var(--fs-2xl)] [&_p.empty-state-title]:[font-weight:var(--fw-regular)] [&_p.empty-state-title]:[color:var(--text)] [&_p.empty-state-hint]:[font-size:var(--fs-lg)] [&_p.empty-state-hint]:[color:var(--subtext)] experiments-empty-state [&_p]:[font-size:var(--fs-2xl)]">
        <p>{emptyHint ?? "No experiments yet."}</p>
      </div>
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
    <div className="experiments-table-wrap [position:absolute] [inset:0] [overflow:auto] [background:var(--base)] [container-type:inline-size]">
      {cancelError && (
        <div className="experiments-table-error [padding:8px_12px] [color:var(--accent-red)] [font-size:var(--fs-sm)] [border-bottom:1px_solid_var(--border)]" role="alert">
          Stop failed: {cancelError}
        </div>
      )}
      <div className="experiments-table [width:100%] [font-size:var(--fs-md)] [background:var(--base)]" role="list" aria-label="Experiments">
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
              className="experiment-table-group [display:grid] [grid-template-columns:minmax(0,_1fr)_auto] [grid-template-areas:'name_meta'_'actions_actions'] [column-gap:32px] [align-items:center] [padding:20px_24px] [row-gap:7px] [border-bottom:1px_solid_color-mix(in_oklab,_var(--text)_7%,_transparent)] [background:var(--base)] [cursor:pointer] [&:hover]:[background:var(--canvas)] [&:last-child]:[border-bottom:none] [@container((max-width:_560px))]:[grid-template-columns:minmax(0,_1fr)_auto] [@container((max-width:_560px))]:[column-gap:14px] [@container((max-width:_560px))]:[row-gap:9px] [@container((max-width:_360px))]:[grid-template-columns:minmax(0,_1fr)] [@container((max-width:_360px))]:[grid-template-areas:'name'_'meta'_'actions']"
              role="listitem"
              onClick={() => onOpen(experiment)}
            >
              <div className="experiment-table-name [grid-area:name] [align-self:start] [min-width:0]">
                <button
                  type="button"
                  className="experiment-table-title [display:block] [width:100%] [overflow:hidden] [color:var(--text)] [font-weight:var(--fw-semibold)] [text-align:left] [text-overflow:ellipsis] [white-space:nowrap]"
                  onClick={(event) => {
                    event.stopPropagation();
                    onOpen(experiment);
                  }}
                >
                  {experiment.title || experiment.slug}
                </button>
                <span className="experiment-table-subtitle [display:flex] [align-items:center] [min-width:0] [gap:6px] [margin-top:4px] [overflow:hidden] [color:var(--subtext)] [font-size:var(--fs-sm)] [&_>_svg]:[flex-shrink:0] [&_code]:[min-width:0] [&_code]:[overflow:hidden] [&_code]:[text-overflow:ellipsis] [&_code]:[white-space:nowrap]" title={experiment.branchName}>
                  <GitBranch size={14} aria-hidden="true" />
                  <code>{experiment.branchName}</code>
                </span>
              </div>
              <div className="experiment-table-meta [grid-area:meta] [align-self:start] [display:flex] [align-items:center] [justify-content:flex-end] [gap:18px] [white-space:nowrap] [@container((max-width:_560px))]:[flex-direction:column] [@container((max-width:_560px))]:[align-items:flex-end] [@container((max-width:_560px))]:[gap:6px] [@container((max-width:_360px))]:[flex-direction:row] [@container((max-width:_360px))]:[flex-wrap:wrap] [@container((max-width:_360px))]:[justify-content:flex-start] [@container((max-width:_360px))]:[gap:12px]">
                <div className="experiment-table-status [display:flex] [align-items:center] [min-width:0]">
                  <StatusBadge status={status} />
                </div>
                <div className="experiment-run-summary [display:flex] [align-items:center] [min-width:0] [gap:8px] [color:var(--subtext)] [font-size:var(--fs-xs)] [font-weight:var(--fw-medium)]">
                  <span>
                    {experimentRuns.length} {experimentRuns.length === 1 ? "run" : "runs"}
                  </span>
                </div>
                <div className="experiment-table-latest [display:flex] [align-items:center] [gap:6px] [min-width:0] [color:var(--subtext)] [font-size:var(--fs-xs)] [font-weight:var(--fw-medium)] [white-space:nowrap]">
                  <span>{latestRun ? timeAgo(latestRun.createdAt) : "Not run yet"}</span>
                </div>
              </div>
              <div
                className="experiment-table-actions [grid-area:actions] [display:flex] [flex-wrap:wrap] [align-items:center] [justify-content:flex-start] [gap:8px] [margin-top:12px]"
                role="group"
                aria-label={`Actions for ${experiment.title || experiment.slug}`}
                onClick={(event) => event.stopPropagation()}
              >
                <button
                  className={EXPERIMENT_TABLE_ACTION_CLASS_NAME}
                  disabled={!logsRun}
                  title={logsRun ? "Open logs" : "No runs yet"}
                  onClick={() => logsRun && onOpenLogs(experiment.id, logsRun.id)}
                >
                  <Terminal size={15} />
                  Logs
                </button>
                <button
                  className={EXPERIMENT_TABLE_ACTION_CLASS_NAME}
                  title={`Browse code on ${experiment.branchName}`}
                  onClick={() => onOpenCode(experiment.id)}
                >
                  <FolderTree size={15} />
                  Code
                </button>
                {liveRun && (
                  <button
                    className="experiment-table-action [display:inline-flex] [align-items:center] [gap:6px] [padding:6px_10px] [border:1px_solid_var(--border)] [border-radius:var(--radius-md)] [background:var(--base)] [color:var(--text)] [font-size:var(--fs-sm)] [font-weight:var(--fw-medium)] [line-height:1] [&:hover:not(:disabled)]:[background:var(--surface)] [&:hover:not(:disabled)]:[border-color:var(--border-strong)] [&:disabled]:[color:var(--muted)] [&:disabled]:[cursor:default] [&:disabled]:[opacity:0.5] [&.danger]:[border-color:color-mix(in_oklab,_var(--accent-red)_42%,_var(--border))] [&.danger]:[background:color-mix(in_oklab,_var(--accent-red)_6%,_var(--base))] [&.danger]:[color:var(--accent-red)] [&.danger:hover:not(:disabled)]:[border-color:var(--accent-red)] [&.danger:hover:not(:disabled)]:[background:color-mix(in_oklab,_var(--accent-red)_10%,_var(--base))] [@container((max-width:_560px))]:[&.danger]:[margin-left:auto] danger"
                    disabled={cancelling}
                    title={cancelling ? "Stop requested" : "Stop run"}
                    onClick={() => void requestCancel(liveRun.id)}
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
    </div>
  );
}
