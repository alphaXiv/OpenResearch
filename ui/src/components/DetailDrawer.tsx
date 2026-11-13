import { useMutate } from "@clo/react-mutation";
import { useWorkspace } from "../workspace";
import { mutCancelRun } from "../queries/projects";
import { m } from "../paraglide/messages.js";
import { ChartLine, ChevronDown, CircleStop, ExternalLink, ScrollText } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import {
  timeAgo,
  type Experiment,
  type Project,
  type Run,
} from "../api";
import { ExperimentOverview } from "./ExperimentOverview";
import type { CodeView } from "./CodeTab";
import { LogTerminal } from "./LogTerminal";
import { RunContactNotice } from "./RunContactNotice";
import { StatusBadge } from "./StatusBadge";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";
import { useResolvedTheme } from "../theme";
import { Button, ButtonLink, MenuItem } from "./ui";
import { buildHistoryNodes } from "./experimentLineageModel";
import { runActive, runDisplay, runStoppable } from "../runDisplay";

export type ExperimentView = "overview" | "terminal" | "trackio";

/** An experiment's detail view, rendered as end-pane tab content. Mount it
 *  keyed by `${experiment.id}:${view}` so per-view state resets on switch. */
export function DetailDrawer({
  experiment,
  experiments,
  project,
  view,
  runs,
  selectedRunId,
  onSelectRun,
  parentExperiment,
  onOpenView,
  onOpenCode,
  onOpenExperiment,
}: {
  experiment: Experiment;
  experiments: Experiment[];
  /** Owning project — supplies owner/repo for the GitHub branch link. */
  project: Project;
  view: ExperimentView;
  runs: Run[];
  selectedRunId: string | null;
  onSelectRun: (id: string | null) => void;
  parentExperiment: Experiment | null;
  onOpenView: (view: ExperimentView, runId: string | undefined, intent: TabOpenIntent) => void;
  onOpenCode: (view: CodeView, intent: TabOpenIntent) => void;
  onOpenExperiment: (id: string, intent: TabOpenIntent) => void;
}) {
  const expRuns = runs
    .filter((r) => r.experimentId === experiment.id)
    .sort((a, b) => b.createdAt - a.createdAt);

  if (view === "overview") {
    const node = buildHistoryNodes(experiments, runs).find((node) => node.id === experiment.id);
    return (
      <ExperimentOverview
        lineageNode={node ?? null}
        onOpenExperiment={onOpenExperiment}
        experiment={experiment}
        parentExperiment={parentExperiment}
        project={project}
        runs={expRuns}
        onOpenLogs={(runId, intent) => onOpenView("terminal", runId, intent)}
        onOpenTrackio={(runId, intent) => onOpenView("trackio", runId, intent)}
        onOpenCode={(intent) => onOpenCode("files", intent)}
        onOpenDiff={(intent) => onOpenCode("changes", intent)}
     />
    );
  }

  return (
    <TerminalView
      experiment={experiment}
      expRuns={expRuns}
      view={view}
      selectedRunId={selectedRunId}
      onSelectRun={onSelectRun}
      onOpenView={onOpenView}
   />
  );
}

/**
 * A run's terminal output, or its Trackio charts, filling the whole pane. The
 * bar above carries the stop button, the run's status and a history switcher —
 * mirror of openresearch.sh's ExperimentFullView TerminalView.
 */
function TerminalView({
  experiment,
  expRuns,
  view,
  selectedRunId,
  onSelectRun,
  onOpenView,
}: {
  experiment: Experiment;
  expRuns: Run[];
  view: ExperimentView;
  selectedRunId: string | null;
  onSelectRun: (id: string | null) => void;
  onOpenView: (view: ExperimentView, runId: string | undefined, intent: TabOpenIntent) => void;
}) {
  const { useTrackioRuns } = useWorkspace().queries;
  const workspace = useWorkspace();
  const { trackioRunUrl } = workspace.api;
  const cancel = useMutate(mutCancelRun);
  const [historyOpen, setHistoryOpen] = useState(false);
  const historyRef = useRef<HTMLDivElement>(null);

  const selectedRun = selectedRunId
    ? expRuns.find((run) => run.id === selectedRunId) ?? null
    : expRuns[0] ?? null;
  const live = selectedRun ? runActive(selectedRun.status) : false;
  const theme = useResolvedTheme();
  const trackioQuery = useTrackioRuns(experiment.projectId, live);
  const trackio = trackioQuery.data;
  const trackioUrl = selectedRun ? trackioRunUrl(trackio, selectedRun.id) : null;
  const trackioEmbedUrl = view === "trackio" && selectedRun
    ? trackioRunUrl(trackio, selectedRun.id, { theme })
    : null;
  const display = selectedRun && runDisplay(selectedRun);
  const error = cancel.error === undefined ? null : cancel.error instanceof Error ? cancel.error.message : String(cancel.error);
  // expRuns is newest-first, so the oldest run is #1. Number a run by its
  // position from the end of the list.
  const runNumber = (id: string) => {
    const idx = expRuns.findIndex((r) => r.id === id);
    return idx === -1 ? expRuns.length : expRuns.length - idx;
  };

  // Close the history dropdown on outside click.
  useEffect(() => {
    if (!historyOpen) return;
    const onDown = (e: MouseEvent) => {
      if (!historyRef.current?.contains(e.target as Node)) setHistoryOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [historyOpen]);

  function stop() {
    if (selectedRun) cancel.run(workspace, selectedRun);
  }

  return (
    <div className="term-view absolute inset-0 flex flex-col bg-background z-20">
      <div className="term-bar flex items-center gap-2 h-10 py-0 px-2.5 border-b border-b-border shrink-0 [&_.error]:text-sm [&_.error]:text-accent-red [&_.btn]:inline-flex [&_.btn]:items-center [&_.btn]:gap-1">
        <div className="term-title min-w-0 text-sm font-semibold text-text overflow-hidden text-ellipsis whitespace-nowrap" title={experiment.title || experiment.slug}>
          {experiment.title || experiment.slug}
        </div>
        <span className="flex-1" />
        {error && (
          <span className="error" role="alert">
            {error}
          </span>
        )}
        {view === "trackio" ? (
          <>
            {trackioUrl && (
              <ButtonLink
                size="small"
                variant="ghost"
                data-tip={m.a11y_open_in_trackio()}
                aria-label={m.a11y_open_in_trackio()}
                href={trackioUrl}
                target="_blank"
                rel="noopener noreferrer"
              >
                <ExternalLink size={14} />
              </ButtonLink>
            )}
            <Button
              size="small"
              variant="ghost"
              {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => onOpenView("terminal", selectedRun?.id, intent))}
            >
              <ScrollText size={14} />
              {m.experiment_overview_logs()}
            </Button>
          </>
        ) : trackioUrl && (
          <Button
            size="small"
            variant="ghost"
            data-tip={m.a11y_trackio_charts()} aria-description={m.a11y_trackio_charts()}
            {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => onOpenView("trackio", selectedRun?.id, intent))}
          >
            <ChartLine size={14} />
            {m.experiment_overview_visualize()}
          </Button>
        )}
        {selectedRun && runStoppable(selectedRun) && (
          <Button size="small" variant="ghost" onClick={stop}>
            <CircleStop size={14} />
            {m.common_stop()}
          </Button>
        )}
        {expRuns.length > 0 && selectedRun && display && (
          <div className="run-history relative shrink-0" ref={historyRef}>
            <Button
              className="h-6 px-2 font-normal"
              data-tip={historyOpen ? undefined : m.detail_drawer_switch_run()} aria-description={m.detail_drawer_switch_run()}
              aria-expanded={historyOpen}
              onClick={() => setHistoryOpen((v) => !v)}
            >
              <span>{m.detail_drawer_run()} {runNumber(selectedRun.id)}</span>
              <StatusBadge className="text-sm font-normal" {...display} />
              <ChevronDown size={14} className="run-picker-chev text-muted shrink-0" />
            </Button>
            {historyOpen && (
              <div className="history-menu absolute top-[calc(100%_+_6px)] end-0 min-w-57.5 max-h-80 overflow-y-auto bg-background border border-border rounded-lg shadow-menu p-1 z-50">
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
                    <StatusBadge {...runDisplay(r)} />
                    <span className="ms-auto text-xs text-muted">{timeAgo(r.createdAt)}</span>
                  </MenuItem>
                ))}
              </div>
            )}
          </div>
        )}
      </div>

      {selectedRun && <RunContactNotice run={selectedRun} className="shrink-0 border-b border-border-variant px-3 py-2" />}

      <div className={`term-fill flex-1 min-h-0 ${view === "trackio" ? "bg-background" : "bg-terminal"}`}>
        {trackioEmbedUrl ? (
          <iframe
            key={trackioEmbedUrl}
            className="block w-full h-full border-0"
            title={m.a11y_trackio_charts()}
            src={trackioEmbedUrl}
            referrerPolicy="no-referrer"
            // Cross-origin, so allow-same-origin keeps the dashboard's own origin, never orx's.
            sandbox="allow-scripts allow-same-origin allow-popups allow-forms allow-downloads"
          />
        ) : view === "trackio" ? (
          <div className="term-empty h-full flex items-center justify-center p-6 text-center text-sm text-muted">
            {trackioQuery.isPending ? m.common_loading() : trackioQuery.error?.message ?? m.detail_drawer_trackio_no_data()}
          </div>
        ) : selectedRun ? (
          // Key by run id so switching runs in the history dropdown remounts
          // the terminal with the selected run's output.
          <LogTerminal key={selectedRun.id} run={selectedRun} />
        ) : (
          <div className="term-empty h-full flex items-center justify-center p-6 text-center text-sm text-muted">{selectedRunId ? m.model_picker_unavailable() : m.detail_drawer_no_runs_yet_ask_the_agent_to_launch()}</div>
        )}
      </div>
    </div>
  );
}
