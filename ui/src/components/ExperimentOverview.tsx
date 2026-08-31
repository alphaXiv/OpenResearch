import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import {
  CalendarDays,
  Clock3,
  FolderTree,
  GitCommitHorizontal,
  Terminal,
} from "lucide-react";
import { useEffect, useState } from "react";
import {
  fmtDuration,
  runDisplayStatus,
  timeAgo,
  type Experiment,
  type Project,
  type Run,
} from "../api";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";
import { BackendBadge } from "./BackendLogos";
import { BranchPill } from "./BranchPill";
import { Md } from "./Md";
import { StatusBadge } from "./StatusBadge";
import { Button } from "./ui";

const EXPERIMENT_OVERVIEW_SECTION_CLASS_NAME = [
  "experiment-overview-section mt-5.5 pt-4.5 border-t border-t-border-variant",
  "[&_h2]:mt-0 [&_h2]:mx-0 [&_h2]:mb-3.5 [&_h2]:text-text [&_h2]:text-sm",
  "[&_h2]:font-semibold",
].join(" ");

const EXPERIMENT_OVERVIEW_COMMAND_CLASS_NAME = [
  "experiment-overview-command block mt-[13px] text-text text-sm",
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
  parentExperiment,
  project,
  runs,
  onOpenLogs,
  onOpenCode,
}: {
  experiment: Experiment;
  parentExperiment: Experiment | null;
  project: Project;
  runs: Run[];
  onOpenLogs: (runId: string, intent: TabOpenIntent) => void;
  onOpenCode: (intent: TabOpenIntent) => void;
}) {
  const latestRun = runs[0] ?? null;
  const hasLiveRun = runs.some(
    (run) => run.status === "running" || run.status === "starting",
  );
  const [now, setNow] = useState(() => Date.now());

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
            <div className="experiment-overview-slug mt-[5px] text-muted text-sm">{experiment.slug}</div>
          </div>
          <StatusBadge status={latestRun ? runDisplayStatus(latestRun) : "idle"} />
        </header>

        <div className="experiment-overview-actions flex gap-[7px] mt-4.5 [@media((max-width:_720px))]:flex-wrap">
          {latestRun && (
            <Button
              {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
                onOpenLogs(latestRun.id, intent),
              )}
            >
              <Terminal size={15} />
              {m.experiment_overview_logs()}
            </Button>
          )}
          <Button
            {...tabOpenGestureHandlers<HTMLButtonElement>(onOpenCode)}
          >
            <FolderTree size={15} />
            {m.experiment_overview_code()}
          </Button>
        </div>

        {experiment.description && (
          <section className="experiment-overview-section mt-5.5 pt-4.5 border-t border-t-border-variant [&_h2]:mt-0 [&_h2]:mx-0 [&_h2]:mb-3.5 [&_h2]:text-text [&_h2]:text-sm [&_h2]:font-semibold overview-description [&_.md]:text-text [&_.md]:leading-[1.65]">
            <h2>{m.experiment_overview_description()}</h2>
            <Md text={experiment.description} />
          </section>
        )}

        <section className={EXPERIMENT_OVERVIEW_SECTION_CLASS_NAME}>
          <h2>{latestRun ? m.experiment_latest_run() : m.experiment_runs()}</h2>
          {latestRun && (
            <>
              <div className="experiment-overview-meta flex items-center flex-wrap gap-y-2.5 gap-x-4.5 text-text text-sm [&_svg]:text-muted [&_.backend-badge]:text-text [&_.status-badge]:text-text [&_>_span]:inline-flex [&_>_span]:items-center [&_>_span]:gap-[5px] [&_code]:text-text [&_code]:text-xs">
                <StatusBadge status={runDisplayStatus(latestRun)} />
                <BackendBadge backend={latestRun.backend} />
                <span title={m.experiment_overview_started()}>
                  <CalendarDays size={13} />
                  {fmtDate(latestRun.createdAt)}
                </span>
                <span title={m.experiment_overview_duration()}>
                  <Clock3 size={13} />
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

        <section className={EXPERIMENT_OVERVIEW_SECTION_CLASS_NAME}>
          <h2>Git</h2>
          <div className="experiment-overview-meta flex items-center flex-wrap text-text text-sm [&_svg]:text-muted [&_.backend-badge]:text-text [&_.status-badge]:text-text [&_>_span]:inline-flex [&_>_span]:items-center [&_>_span]:gap-[5px] [&_code]:text-text [&_code]:text-xs experiment-overview-git-meta gap-y-[9px] gap-x-3.5 [&_.files-pill]:py-[5px] [&_.files-pill]:px-2 [&_.files-pill]:rounded-sm [&_.files-pill_code]:text-xs">
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

        {runs.length > 0 && (
          <section className={EXPERIMENT_OVERVIEW_SECTION_CLASS_NAME}>
            <h2>{m.experiment_overview_run_history()}</h2>
            <div className="experiment-run-history border-t border-t-border-variant [&_button]:w-full [&_button]:grid [&_button]:grid-cols-[minmax(72px,_0.7fr)_minmax(100px,_1fr)_minmax(70px,_0.7fr)_60px_16px] [&_button]:items-center [&_button]:gap-3.5 [&_button]:py-[11px] [&_button]:px-0.5 [&_button]:border-b [&_button]:border-b-border-variant [&_button]:text-text [&_button]:text-start [&_button]:text-sm [&_button:hover]:bg-surface [@media((max-width:_720px))]:[&_button]:grid-cols-[65px_1fr_60px_16px] [@media((max-width:_720px))]:[&_button_>_:nth-child(3)]:hidden">
              {runs.map((run, index) => (
                <button
                  key={run.id}
                  {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
                    onOpenLogs(run.id, intent),
                  )}
                >
                  <span className="experiment-run-number text-xs font-medium">{m.experiment_overview_run()} {runs.length - index}</span>
                  <StatusBadge status={runDisplayStatus(run)} />
                  <span>{timeAgo(run.createdAt)}</span>
                  <span>{runDuration(run, now)}</span>
                  <Terminal size={13} />
                </button>
              ))}
            </div>
          </section>
        )}
      </div>
    </div>
  );
}
