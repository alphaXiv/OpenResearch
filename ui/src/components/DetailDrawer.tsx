import { ChevronDown, CircleStop } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import {
  cancelRun,
  runDisplayStatus,
  timeAgo,
  type Experiment,
  type Project,
  type Run,
} from "../api";
import { ExperimentOverview } from "./ExperimentOverview";
import type { CodeView } from "./CodeTab";
import { LogTerminal } from "./LogTerminal";
import { StatusBadge } from "./StatusBadge";
import { SMALL_BUTTON_CLASS_NAME } from "../styleClasses";
import type { TabOpenIntent } from "../tabPreview";
import { isTinkerRun, TinkerCancelDialog, tinkerConsoleUrl } from "./TinkerCancelDialog";

export type ExperimentView = "overview" | "terminal";

/** An experiment's detail view, rendered as right-pane tab content. Mount it
 *  keyed by `${experiment.id}:${view}` so per-view state resets on switch. */
export function DetailDrawer({
  experiment,
  project,
  view,
  runs,
  selectedRunId,
  onSelectRun,
  parentExperiment,
  onOpenView,
  onOpenCode,
}: {
  experiment: Experiment;
  /** Owning project — supplies owner/repo for the GitHub branch link. */
  project: Project;
  view: ExperimentView;
  runs: Run[];
  selectedRunId: string | null;
  onSelectRun: (id: string | null) => void;
  parentExperiment: Experiment | null;
  onOpenView: (view: ExperimentView, runId: string | undefined, intent: TabOpenIntent) => void;
  onOpenCode: (view: CodeView, intent: TabOpenIntent) => void;
}) {
  const expRuns = runs
    .filter((r) => r.experimentId === experiment.id)
    .sort((a, b) => b.createdAt - a.createdAt);

  if (view === "overview") {
    return (
      <ExperimentOverview
        experiment={experiment}
        parentExperiment={parentExperiment}
        project={project}
        runs={expRuns}
        onOpenLogs={(runId, intent) => onOpenView("terminal", runId, intent)}
        onOpenCode={(intent) => onOpenCode("files", intent)}
      />
    );
  }

  return (
    <TerminalView
      experiment={experiment}
      expRuns={expRuns}
      selectedRunId={selectedRunId}
      onSelectRun={onSelectRun}
    />
  );
}

/**
 * A run's terminal output filling the whole pane. The bar above carries the
 * stop button, the run's status and a history switcher — mirror of
 * openresearch.sh's ExperimentFullView TerminalView.
 */
function TerminalView({
  experiment,
  expRuns,
  selectedRunId,
  onSelectRun,
}: {
  experiment: Experiment;
  expRuns: Run[];
  selectedRunId: string | null;
  onSelectRun: (id: string | null) => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const [pendingRunId, setPendingRunId] = useState<string | null>(null);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [tinkerRun, setTinkerRun] = useState<Run | null>(null);
  const historyRef = useRef<HTMLDivElement>(null);

  const selectedRun =
    (selectedRunId && expRuns.find((r) => r.id === selectedRunId)) || expRuns[0] || null;
  const live = selectedRun?.status === "running" || selectedRun?.status === "starting";
  const cancelling = Boolean(
    selectedRun && live && (selectedRun.cancelRequested || pendingRunId === selectedRun.id),
  );
  // expRuns is newest-first, so the oldest run is #1. Number a run by its
  // position from the end of the list.
  const runNumber = (id: string) => {
    const idx = expRuns.findIndex((r) => r.id === id);
    return idx === -1 ? expRuns.length : expRuns.length - idx;
  };

  // When a new run starts while the tab is open, follow it live.
  const seenRunIds = useRef<Set<string> | null>(null);
  useEffect(() => {
    if (seenRunIds.current === null) {
      seenRunIds.current = new Set(expRuns.map((r) => r.id));
      return;
    }
    const fresh = expRuns.find((r) => !seenRunIds.current!.has(r.id));
    for (const r of expRuns) seenRunIds.current.add(r.id);
    if (fresh) onSelectRun(fresh.id);
  }, [expRuns, onSelectRun]);

  // Close the history dropdown on outside click.
  useEffect(() => {
    if (!historyOpen) return;
    const onDown = (e: MouseEvent) => {
      if (!historyRef.current?.contains(e.target as Node)) setHistoryOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [historyOpen]);

  async function stop(runId: string) {
    setError(null);
    setPendingRunId(runId);
    try {
      await cancelRun(runId);
    } catch (err) {
      setPendingRunId(null);
      throw err;
    }
  }

  function requestStop() {
    if (!selectedRun) return;
    setError(null);
    if (isTinkerRun(selectedRun)) {
      setTinkerRun(selectedRun);
      return;
    }
    void stop(selectedRun.id).catch((err) =>
      setError(err instanceof Error ? err.message : String(err)),
    );
  }

  return (
    <div className="term-view absolute inset-0 flex flex-col bg-background z-20">
      <div className="term-bar flex items-center gap-2 h-10 py-0 px-2.5 border-b border-b-border shrink-0 [&_.error]:text-sm [&_.error]:text-accent-red [&_.btn]:inline-flex [&_.btn]:items-center [&_.btn]:gap-[5px]">
        <div className="term-title min-w-0 text-md font-semibold text-text overflow-hidden text-ellipsis whitespace-nowrap" title={experiment.title || experiment.slug}>
          {experiment.title || experiment.slug}
        </div>
        <span style={{ flex: 1 }} />
        {error && (
          <span className="error" role="alert">
            {error}
          </span>
        )}
        {live && (
          <button className={`${SMALL_BUTTON_CLASS_NAME} ghost`} disabled={cancelling} onClick={requestStop}>
            <CircleStop size={13} />
            {cancelling ? "Cancelling…" : "Stop"}
          </button>
        )}
        {expRuns.length > 0 && selectedRun && (
          <div className="run-history relative shrink-0" ref={historyRef}>
            <button
              className="run-picker inline-flex items-center gap-2 pt-1 pr-1.5 pb-1 pl-2.5 border border-border rounded-md bg-background text-text [&:hover]:bg-surface [&_.run-label]:text-sm [&_.run-label]:font-semibold"
              title="Switch run"
              onClick={() => setHistoryOpen((v) => !v)}
            >
              <span className="run-label">Run {runNumber(selectedRun.id)}</span>
              <StatusBadge
                status={cancelling ? "cancelling" : runDisplayStatus(selectedRun)}
              />
              <ChevronDown size={14} className="run-picker-chev text-muted shrink-0" />
            </button>
            {historyOpen && (
              <div className="history-menu absolute top-[calc(100%_+_6px)] right-0 min-w-57.5 max-h-80 overflow-y-auto bg-background border border-border rounded-lg shadow-[0_12px_32px_rgba(0,_0,_0,_0.18)] p-[5px] z-50">
                {expRuns.map((r) => (
                  <button
                    key={r.id}
                    className={`history-item flex items-center gap-2 w-full text-left py-1.5 px-2 text-sm rounded-sm [&:hover]:bg-surface [&.active]:bg-surface [&_.run-label]:font-semibold [&_.when]:ml-auto [&_.when]:text-xs [&_.when]:text-muted ${r.id === selectedRun?.id ? "active" : ""}`}
                    onClick={() => {
                      onSelectRun(r.id);
                      setHistoryOpen(false);
                    }}
                  >
                    <span className="run-label">Run {runNumber(r.id)}</span>
                    <StatusBadge status={runDisplayStatus(r)} />
                    <span className="when">{timeAgo(r.createdAt)}</span>
                  </button>
                ))}
              </div>
            )}
          </div>
        )}
      </div>

      {selectedRun?.status === "cancelled" && isTinkerRun(selectedRun) && (
        <div className="border-b border-border bg-accent-amber-subtle px-3 py-2 text-sm text-accent-amber">
          The local controller stopped. Accepted Tinker operations may still be running.{" "}
          <a href={tinkerConsoleUrl(selectedRun)} target="_blank" rel="noreferrer">Open Tinker</a>
        </div>
      )}

      <div className="term-fill flex-1 min-h-0 bg-[var(--term-bg)] pt-1 pr-0 pb-1 pl-1.5">
        {selectedRun ? (
          // Key by run id so switching runs in the history dropdown remounts
          // the terminal with the selected run's output.
          <LogTerminal key={selectedRun.id} runId={selectedRun.id} />
        ) : (
          <div className="term-empty h-full flex items-center justify-center p-6 text-center text-md text-muted">No runs yet — ask the agent to launch one.</div>
        )}
      </div>
      {tinkerRun && (
        <TinkerCancelDialog run={tinkerRun} onCancel={stop} onClose={() => setTinkerRun(null)} />
      )}
    </div>
  );
}
