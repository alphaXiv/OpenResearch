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
  onOpenView: (view: ExperimentView, runId?: string) => void;
  onOpenCode: (view: CodeView) => void;
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
        onOpenLogs={(runId) => onOpenView("terminal", runId)}
        onOpenCode={() => onOpenCode("files")}
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
    <div className="term-view [position:absolute] [inset:0] [display:flex] [flex-direction:column] [background:var(--base)] [z-index:20]">
      <div className="term-bar [display:flex] [align-items:center] [gap:8px] [height:40px] [padding:0_10px] [border-bottom:1px_solid_var(--border)] [flex-shrink:0] [&_.error]:[font-size:var(--fs-sm)] [&_.error]:[color:var(--accent-red)] [&_.btn]:[display:inline-flex] [&_.btn]:[align-items:center] [&_.btn]:[gap:5px]">
        <div className="term-title [min-width:0] [font-size:var(--fs-md)] [font-weight:var(--fw-semibold)] [color:var(--text)] [overflow:hidden] [text-overflow:ellipsis] [white-space:nowrap]" title={experiment.title || experiment.slug}>
          {experiment.title || experiment.slug}
        </div>
        <span style={{ flex: 1 }} />
        {error && (
          <span className="error" role="alert">
            {error}
          </span>
        )}
        {live && (
          <button className={`${SMALL_BUTTON_CLASS_NAME} ghost`} disabled={cancelling} onClick={() => void stop()}>
            <CircleStop size={13} />
            {cancelling ? "Cancelling…" : "Stop"}
          </button>
        )}
        {expRuns.length > 0 && selectedRun && (
          <div className="run-history [position:relative] [flex-shrink:0]" ref={historyRef}>
            <button
              className="run-picker [display:inline-flex] [align-items:center] [gap:8px] [padding:4px_6px_4px_10px] [border:1px_solid_var(--border)] [border-radius:var(--radius-md)] [background:var(--base)] [color:var(--text)] [&:hover]:[background:var(--surface)] [&_.run-label]:[font-size:var(--fs-sm)] [&_.run-label]:[font-weight:var(--fw-semibold)]"
              title="Switch run"
              onClick={() => setHistoryOpen((v) => !v)}
            >
              <span className="run-label">Run {runNumber(selectedRun.id)}</span>
              <StatusBadge
                status={cancelling ? "cancelling" : runDisplayStatus(selectedRun)}
              />
              <ChevronDown size={14} className="run-picker-chev [color:var(--muted)] [flex-shrink:0]" />
            </button>
            {historyOpen && (
              <div className="history-menu [position:absolute] [top:calc(100%_+_6px)] [right:0] [min-width:230px] [max-height:320px] [overflow-y:auto] [background:var(--base)] [border:1px_solid_var(--border)] [border-radius:var(--radius-lg)] [box-shadow:0_12px_32px_rgba(0,_0,_0,_0.18)] [padding:5px] [z-index:50]">
                {expRuns.map((r) => (
                  <button
                    key={r.id}
                    className={`history-item [display:flex] [align-items:center] [gap:8px] [width:100%] [text-align:left] [padding:6px_8px] [font-size:var(--fs-sm)] [border-radius:var(--radius-sm)] [&:hover]:[background:var(--surface)] [&.active]:[background:var(--surface)] [&_.run-label]:[font-weight:var(--fw-semibold)] [&_.when]:[margin-left:auto] [&_.when]:[font-size:var(--fs-xs)] [&_.when]:[color:var(--muted)] ${r.id === selectedRun?.id ? "active" : ""}`}
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

      <div className="term-fill [flex:1] [min-height:0] [background:var(--term-bg)] [padding:4px_0_4px_6px]">
        {selectedRun ? (
          // Key by run id so switching runs in the history dropdown remounts
          // the terminal with the selected run's output.
          <LogTerminal key={selectedRun.id} runId={selectedRun.id} />
        ) : (
          <div className="term-empty [height:100%] [display:flex] [align-items:center] [justify-content:center] [padding:24px] [text-align:center] [font-size:var(--fs-md)] [color:var(--muted)]">No runs yet — ask the agent to launch one.</div>
        )}
      </div>
    </div>
  );
}
