import { m } from "../paraglide/messages.js";
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
import type { TabOpenIntent } from "../tabPreview";
import { Button, MenuItem } from "./ui";

export type ExperimentView = "overview" | "terminal";

/** An experiment's detail view, rendered as end-pane tab content. Mount it
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

  async function stop() {
    if (!selectedRun) return;
    setError(null);
    setPendingRunId(selectedRun.id);
    try {
      await cancelRun(selectedRun.id);
    } catch (err) {
      setPendingRunId(null);
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <div className="term-view absolute inset-0 flex flex-col bg-background z-20">
      <div className="term-bar flex items-center gap-2 h-10 py-0 px-2.5 border-b border-b-border shrink-0 [&_.error]:text-sm [&_.error]:text-accent-red [&_.btn]:inline-flex [&_.btn]:items-center [&_.btn]:gap-[5px]">
        <div className="term-title min-w-0 text-sm font-semibold text-text overflow-hidden text-ellipsis whitespace-nowrap" title={experiment.title || experiment.slug}>
          {experiment.title || experiment.slug}
        </div>
        <span className="flex-1" />
        {error && (
          <span className="error" role="alert">
            {error}
          </span>
        )}
        {live && (
          <Button size="small" variant="ghost" disabled={cancelling} onClick={() => void stop()}>
            <CircleStop size={13} />
            {cancelling ? m.common_cancelling() : m.common_stop()}
          </Button>
        )}
        {expRuns.length > 0 && selectedRun && (
          <div className="run-history relative shrink-0" ref={historyRef}>
            <Button
              title={m.detail_drawer_switch_run()}
              aria-expanded={historyOpen}
              onClick={() => setHistoryOpen((v) => !v)}
            >
              <span>{m.detail_drawer_run()} {runNumber(selectedRun.id)}</span>
              <StatusBadge
                status={cancelling ? "cancelling" : runDisplayStatus(selectedRun)}
             />
              <ChevronDown size={14} className="run-picker-chev text-muted shrink-0" />
            </Button>
            {historyOpen && (
              <div className="history-menu absolute top-[calc(100%_+_6px)] end-0 min-w-57.5 max-h-80 overflow-y-auto bg-background border border-border rounded-lg shadow-menu p-[5px] z-50">
                {expRuns.map((r) => (
                  <MenuItem
                    key={r.id}
                    className="justify-start"
                    active={r.id === selectedRun?.id}
                    onClick={() => {
                      onSelectRun(r.id);
                      setHistoryOpen(false);
                    }}
                  >
                    <span className="font-medium">{m.detail_drawer_run()} {runNumber(r.id)}</span>
                    <StatusBadge status={runDisplayStatus(r)} />
                    <span className="ms-auto text-xs text-muted">{timeAgo(r.createdAt)}</span>
                  </MenuItem>
                ))}
              </div>
            )}
          </div>
        )}
      </div>

      <div className="term-fill flex-1 min-h-0 bg-terminal pt-1 pe-0 pb-1 ps-1.5">
        {selectedRun ? (
          // Key by run id so switching runs in the history dropdown remounts
          // the terminal with the selected run's output.
          <LogTerminal key={selectedRun.id} runId={selectedRun.id} />
        ) : (
          <div className="term-empty h-full flex items-center justify-center p-6 text-center text-sm text-muted">{m.detail_drawer_no_runs_yet_ask_the_agent_to_launch()}</div>
        )}
      </div>
    </div>
  );
}
