import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import {
  CalendarDays,
  ChartLine,
  Clock3,
  FolderTree,
  GitCommitHorizontal,
  GitCompareArrows,
  Terminal,
} from "lucide-react";
import { useEffect, useState } from "react";
import {
  experimentMonitoringError,
  fmtDuration,
  runDisplayStatus,
  timeAgo,
  trackioRunUrl,
  type Experiment,
  type Project,
  type Run,
} from "../api";
import { useTrackioRuns } from "../queries/settings";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";
import { BackendBadge } from "./BackendLogos";
import { BranchPill } from "./BranchPill";
import { Md } from "./Md";
import { StatusBadge } from "./StatusBadge";
import { Button, Tooltip } from "./ui";
import { ExperimentLineage } from "./ExperimentLineage";
import type { HistoryNode } from "./experimentLineageModel";

const EXPERIMENT_OVERVIEW_SECTION_CLASS_NAME = [
  "experiment-overview-section mt-5.5 pt-4.5 border-t border-t-border-variant",
  "[&_h2]:mt-0 [&_h2]:mx-0 [&_h2]:mb-3.5 [&_h2]:text-text [&_h2]:text-sm",
  "[&_h2]:font-semibold",
].join(" ");

const EXPERIMENT_OVERVIEW_COMMAND_CLASS_NAME = [
  "experiment-overview-command block mt-3 text-text text-sm",
  "wrap-anywhere",
].join(" ");

function fmtDate(ms: number): string {
  return new Date(ms).toLocaleString(getLocale(), {
    month: "short",
    day: "numeric",
    year: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

function runDuration(run: Run, now: number): string {
  return fmtDuration((run.endedAt ?? now) - run.createdAt);
}

export function ExperimentOverview({
  experiment,
  lineageNode,
  onOpenExperiment,
  parentExperiment,
  project,
  runs,
  onOpenLogs,
  onOpenTrackio,
  onOpenCode,
  onOpenDiff,
}: {
  experiment: Experiment;
  lineageNode: HistoryNode | null;
  onOpenExperiment: (id: string, intent: TabOpenIntent) => void;
  parentExperiment: Experiment | null;
  project: Project;
  runs: Run[];
  onOpenLogs: (runId: string, intent: TabOpenIntent) => void;
  onOpenTrackio: (runId: string, intent: TabOpenIntent) => void;
  onOpenCode: (intent: TabOpenIntent) => void;
  onOpenDiff: (intent: TabOpenIntent) => void;
}) {
  const latestRun = runs[0] ?? null;
  const monitoringError = experimentMonitoringError(runs);
  const hasLiveRun = runs.some(
    (run) => run.status === "running" || run.status === "starting",
  );
  const [now, setNow] = useState(() => Date.now());
  const trackio = useTrackioRuns(experiment.projectId, hasLiveRun).data;
  const hasTrackio = latestRun !== null && trackioRunUrl(trackio, latestRun.id) !== null;

  useEffect(() => {
    if (!hasLiveRun) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [hasLiveRun]);

  return (
    <div className="experiment-overview absolute inset-0 overflow-y-auto bg-background [&_h1]:m-0 [&_h1]:text-text [&_h1]:text-xl [&_h1]:leading-tight">
      <div className="experiment-overview-inner w-full max-w-230 my-0 mx-auto pt-6.5 px-7 pb-10 [@media((max-width:_720px))]:pt-5 [@media((max-width:_720px))]:px-4.5 [@media((max-width:_720px))]:pb-8">
        <header className="experiment-overview-head flex items-start justify-between gap-6">
          <div className="experiment-overview-heading min-w-0">
            <h1>{experiment.title || experiment.slug}</h1>
            <div className="experiment-overview-slug mt-1 text-muted text-sm">{experiment.slug}</div>
          </div>
          <StatusBadge status={latestRun ? runDisplayStatus(latestRun) : "idle"} />
        </header>

        <div className="experiment-overview-actions flex gap-2 mt-4.5 [@media((max-width:_720px))]:flex-wrap">
          {latestRun && (
            <Button
              {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
                onOpenLogs(latestRun.id, intent),
              )}
            >
              <Terminal size={16} />
              {m.experiment_overview_logs()}
            </Button>
          )}
          {latestRun && hasTrackio && (
            <Tooltip interactive content={m.a11y_trackio_charts()} className="rounded-md">
              <Button
                aria-description={m.a11y_trackio_charts()}
                {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
                  onOpenTrackio(latestRun.id, intent),
                )}
              >
                <ChartLine size={16} />
                {m.experiment_overview_visualize()}
              </Button>
            </Tooltip>
          )}
          <Button
            {...tabOpenGestureHandlers<HTMLButtonElement>(onOpenCode)}
          >
            <FolderTree size={16} />
            {m.experiment_overview_code()}
          </Button>
          <Tooltip interactive content={m.experiment_overview_diff_from_parent()}>
            <Button
              disabled={!parentExperiment}
              {...tabOpenGestureHandlers<HTMLButtonElement>(onOpenDiff)}
            >
              <GitCompareArrows size={16} />
              {m.experiment_overview_diff()}
            </Button>
          </Tooltip>
        </div>

        {experiment.description && (
          <section className="experiment-overview-section mt-5.5 pt-4.5 border-t border-t-border-variant [&_h2]:mt-0 [&_h2]:mx-0 [&_h2]:mb-3.5 [&_h2]:text-text [&_h2]:text-sm [&_h2]:font-semibold overview-description [&_.md]:text-text [&_.md]:leading-relaxed">
            <h2>{m.experiment_overview_description()}</h2>
            <Md text={experiment.description} />
          </section>
        )}

        <section className={EXPERIMENT_OVERVIEW_SECTION_CLASS_NAME}>
          <h2>{latestRun ? m.experiment_latest_run() : m.experiment_runs()}</h2>
          {latestRun && (
            <>
              <div className="experiment-overview-meta flex items-center flex-wrap gap-y-2.5 gap-x-4.5 text-text text-sm [&_svg]:text-muted [&_.backend-badge]:text-text [&_.status-badge]:text-text [&_>_span]:inline-flex [&_>_span]:items-center [&_>_span]:gap-1 [&_code]:text-text [&_code]:text-xs">
                <StatusBadge status={runDisplayStatus(latestRun)} />
                <BackendBadge backend={latestRun.backend} />
                <span title={m.experiment_overview_started()}>
                  <CalendarDays size={14} />
                  {fmtDate(latestRun.createdAt)}
                </span>
                <span title={m.experiment_overview_duration()}>
                  <Clock3 size={14} />
                  {runDuration(latestRun, now)}
                </span>
                {latestRun.commitSha && (
                  <span title={m.experiment_overview_commit()}>
                    <GitCommitHorizontal size={14} />
                    <code>{latestRun.commitSha.slice(0, 7)}</code>
                  </span>
                )}
                {latestRun.exitCode !== null &&
                  latestRun.exitCode !== undefined &&
                  latestRun.exitCode !== 0 && (
                    <span>{m.experiment_overview_exit()} {latestRun.exitCode}</span>
                  )}
              </div>
              {latestRun.command && (
                <code className={EXPERIMENT_OVERVIEW_COMMAND_CLASS_NAME}>$ {latestRun.command}</code>
              )}
              {monitoringError && (
                <p className="experiment-overview-monitoring mt-4 text-accent-amber text-sm wrap-anywhere">
                  {monitoringError}
                </p>
              )}
              {latestRun.resultMarkdown && (
                <div
                  className={`experiment-overview-result mt-4 [&.failed]:text-accent-red ${latestRun.status === "failed" ? "failed" : ""}`}
                >
                  <Md text={latestRun.resultMarkdown} />
                </div>
              )}
            </>
          )}
        </section>

        {runs.length > 0 && (
          <section className={`${EXPERIMENT_OVERVIEW_SECTION_CLASS_NAME} @container`}>
            <h2>{m.experiment_overview_run_history()}</h2>
            <div className="experiment-run-history divide-y divide-border-variant [&_button]:w-full [&_button]:grid [&_button]:grid-cols-[minmax(72px,_0.7fr)_minmax(100px,_1fr)_minmax(70px,_0.7fr)_60px_16px] [&_button]:items-center [&_button]:gap-3.5 [&_button]:py-3 [&_button]:px-0.5 [&_button]:text-text [&_button]:text-start [&_button]:text-sm [&_button:hover]:bg-surface @max-lg:[&_button]:grid-cols-[48px_minmax(0,_1fr)_40px_16px] @max-lg:[&_button_>_:nth-child(3)]:hidden">
              {runs.map((run, index) => (
                <button
                  key={run.id}
                  {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
                    onOpenLogs(run.id, intent),
                  )}
                >
                  <span className="experiment-run-number text-xs font-medium">{m.experiment_overview_run()} {runs.length - index}</span>
                  <StatusBadge status={runDisplayStatus(run)} className="min-w-0" />
                  <span>{timeAgo(run.createdAt)}</span>
                  <span>{runDuration(run, now)}</span>
                  <Terminal size={14} />
                </button>
              ))}
            </div>
          </section>
        )}

        {lineageNode && (
          <div className="mt-5.5 border-t border-border-variant pt-4.5">
            <ExperimentLineage node={lineageNode} onSelect={onOpenExperiment} />
          </div>
        )}

        <section className={EXPERIMENT_OVERVIEW_SECTION_CLASS_NAME}>
          <h2>Git</h2>
          <div className="experiment-overview-meta flex items-center flex-wrap text-text text-sm [&_svg]:text-muted [&_.backend-badge]:text-text [&_.status-badge]:text-text [&_>_span]:inline-flex [&_>_span]:items-center [&_>_span]:gap-1 [&_code]:text-text [&_code]:text-xs experiment-overview-git-meta gap-y-2 gap-x-3.5 [&_.files-pill]:py-1 [&_.files-pill]:px-2 [&_.files-pill]:rounded-sm [&_.files-pill_code]:text-xs">
            <BranchPill
              owner={project.githubEnabled ? project.githubOwner : ""}
              repo={project.githubEnabled ? project.githubRepo : ""}
              branch={experiment.branchName}
           />
            {parentExperiment && (
              <span>
                {m.experiment_overview_from()} <code>{parentExperiment.slug}</code>
              </span>
            )}
            <span title={fmtDate(experiment.createdAt)}>
              {m.experiment_overview_created()} {timeAgo(experiment.createdAt)}
            </span>
          </div>
          {experiment.runCommand !== latestRun?.command && (
            <code className={EXPERIMENT_OVERVIEW_COMMAND_CLASS_NAME}>$ {experiment.runCommand}</code>
          )}
        </section>

      </div>
    </div>
  );
}
