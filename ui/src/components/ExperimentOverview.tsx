import { useWorkspace } from "../workspace";
import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import {
  ArrowUpRight,
  CalendarDays,
  ChartLine,
  Clock3,
  FolderTree,
  GitCommitHorizontal,
  GitCompareArrows,
  Terminal,
} from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import {
  timeAgo,
  type Experiment,
  type Project,
  type Run,
} from "../api";
import { autoDir, fmtNumber, ltr } from "../i18n";
import { capitalized } from "../runMonitor/words";
import { noticeRun, runActive, runDisplay, runDuration, runNeverStarted, runResult } from "../runDisplay";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";
import { BackendBadge } from "./BackendLogos";
import { BranchPill } from "./BranchPill";
import type { OpenFile } from "./FileReference";
import { ChatImageScope, Md } from "./Md";
import { RunContactNotice } from "./RunContactNotice";
import { useRunClock } from "./RunTiming";
import { RunDetails } from "./run/Details";
import { RunPage, WatchSection } from "./run/RunPage";
import { historySentence } from "../runMonitor/model";
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

export function ExperimentOverview({
  experiment,
  lineageNode,
  onOpenExperiment,
  onOpenFile,
  parentExperiment,
  project,
  runs,
  selectedRunId = null,
  onOpenRun,
  onOpenLogs,
  onOpenTrackio,
  onOpenCode,
  onOpenDiff,
}: {
  experiment: Experiment;
  lineageNode: HistoryNode | null;
  onOpenExperiment: (id: string, intent: TabOpenIntent) => void;
  onOpenFile: (path: string, sessionId: string | null | undefined, line: number | undefined, exp: string | undefined, ref: string | undefined, intent: TabOpenIntent) => void;
  parentExperiment: Experiment | null;
  project: Project;
  runs: Run[];
  selectedRunId?: string | null;
  onOpenRun: (runId: string, intent: TabOpenIntent) => void;
  onOpenLogs: (runId: string, intent: TabOpenIntent) => void;
  onOpenTrackio: (runId: string, intent: TabOpenIntent) => void;
  onOpenCode: (intent: TabOpenIntent) => void;
  onOpenDiff: (intent: TabOpenIntent) => void;
}) {
  const { useTrackioRuns, listChatSessionsQuery } = useWorkspace().queries;
  const { trackioRunUrl } = useWorkspace().api;

  const run = selectedRunId ? runs.find((run) => run.id === selectedRunId) ?? null : runs[0] ?? null;
  const runHeading = !run ? m.experiment_runs()
    : run === runs[0] ? m.experiment_latest_run()
    : capitalized(m.run_number({ number: ltr(fmtNumber(runs.length - runs.indexOf(run))) }));
  const display = run ? runDisplay(run) : null;
  const waiting = display?.waiting;
  const attentionRun = selectedRunId ? run : noticeRun(runs);
  const result = run && run.status !== "failed" ? runResult(run) : null;
  const hasLiveRun = runs.some((run) => runActive(run.status));
  const now = useRunClock(hasLiveRun);
  const overview = useRef<HTMLDivElement>(null);
  const trackio = useTrackioRuns(experiment.projectId, hasLiveRun).data;
  const sessions = useQuery(listChatSessionsQuery(experiment.projectId)).data;
  // The experiment outlives its chat; the file route 404s a deleted session.
  const chatDeleted = sessions?.every((session) => session.id !== experiment.chatSessionId);
  const chatSessionId = chatDeleted ? null : experiment.chatSessionId;
  const openFile: OpenFile = (path, line, exp, ref, intent) => onOpenFile(path, chatSessionId, line, exp, ref, intent);
  const hasTrackio = run !== null && trackioRunUrl(trackio, run.id) !== null;

  useEffect(() => {
    if (overview.current) overview.current.scrollTop = 0;
  }, [selectedRunId]);

  return (
    <ChatImageScope projectId={experiment.projectId} sessionId={chatSessionId}>
    <div ref={overview} className="experiment-overview absolute inset-0 overflow-y-auto bg-background [&_h1]:m-0 [&_h1]:text-text [&_h1]:text-xl [&_h1]:leading-tight">
      <div className="experiment-overview-inner w-full max-w-230 my-0 mx-auto pt-6.5 px-7 pb-10 [@media((max-width:_720px))]:pt-5 [@media((max-width:_720px))]:px-4.5 [@media((max-width:_720px))]:pb-8">
        <header className="experiment-overview-head flex items-start justify-between gap-6">
          <div className="experiment-overview-heading min-w-0">
            <h1>{experiment.title || experiment.slug}</h1>
            <div className="experiment-overview-slug mt-1 text-muted text-sm">{experiment.slug}</div>
            {waiting && <div className="mt-1 truncate text-subtext text-sm" title={waiting}>{autoDir(waiting)}</div>}
          </div>
          {display ? <StatusBadge {...display} waiting={undefined} /> : <StatusBadge status="idle" />}
        </header>

        <div className="experiment-overview-actions flex gap-2 mt-4.5 [@media((max-width:_720px))]:flex-wrap">
          {run && (
            <Button
              {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
                onOpenLogs(run.id, intent),
              )}
            >
              <Terminal size={16} />
              {m.experiment_overview_logs()}
            </Button>
          )}
          {run && hasTrackio && (
            <Tooltip interactive content={m.a11y_trackio_charts()} className="rounded-md">
              <Button
                aria-description={m.a11y_trackio_charts()}
                {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
                  onOpenTrackio(run.id, intent),
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
            <Md text={experiment.description} onOpenFile={openFile} />
          </section>
        )}

        <section className={EXPERIMENT_OVERVIEW_SECTION_CLASS_NAME}>
          <h2>{runHeading}</h2>
          {run && (
            <>
              <div className="experiment-overview-meta flex items-center flex-wrap gap-y-2.5 gap-x-4.5 text-text text-sm [&_svg]:text-muted [&_.backend-badge]:text-text [&_.status-badge]:text-text [&_>_span]:inline-flex [&_>_span]:items-center [&_>_span]:gap-1 [&_code]:text-text [&_code]:text-xs">
                <StatusBadge {...runDisplay(run)} className="max-w-full" />
                <BackendBadge backend={run.backend} />
                {run.startedAt != null && (
                  <span title={m.experiment_overview_started()}>
                    <CalendarDays size={14} />
                    {fmtDate(run.startedAt)}
                  </span>
                )}
                {!runNeverStarted(run) && (
                  <span title={m.experiment_overview_duration()}>
                    <Clock3 size={14} />
                    {runDuration(run, now)}
                  </span>
                )}
                {run.commitSha && (
                  <span title={m.experiment_overview_commit()}>
                    <GitCommitHorizontal size={14} />
                    <code>{run.commitSha.slice(0, 7)}</code>
                  </span>
                )}
                {run.exitCode !== null &&
                  run.exitCode !== undefined &&
                  run.exitCode !== 0 && (
                    <span>{m.experiment_overview_exit()} {run.exitCode}</span>
                  )}
              </div>
              {run.command && (
                <code className={EXPERIMENT_OVERVIEW_COMMAND_CLASS_NAME}>$ {run.command}</code>
              )}
              <RunPage key={run.id} run={run} name={experiment.slug} chatSessionId={experiment.chatSessionId} now={now} />
              {attentionRun && !((run.monitor || run.status === "failed") && attentionRun.id === run.id) && <RunContactNotice run={attentionRun} className="mt-4" />}
              {result && (
                <div
                  className={`experiment-overview-result mt-4 [&.failed]:text-accent-red ${run.status === "failed" ? "failed" : ""}`}
                >
                  <Md text={result} onOpenFile={openFile} />
                </div>
              )}
            </>
          )}
        </section>

        {runs.length > 0 && (
          <section className={`${EXPERIMENT_OVERVIEW_SECTION_CLASS_NAME} @container`}>
            <h2>{m.experiment_overview_run_history()}</h2>
            <div className="experiment-run-history divide-y divide-border-variant [&_button]:w-full [&_button]:grid [&_button]:grid-cols-[minmax(56px,_0.5fr)_minmax(100px,_0.9fr)_minmax(0,_2.2fr)_minmax(64px,_0.6fr)_60px_16px] [&_button]:items-center [&_button]:gap-3.5 [&_button]:py-3 [&_button]:px-0.5 [&_button]:text-text [&_button]:text-start [&_button]:text-sm [&_button:hover]:bg-surface @max-lg:[&_button]:grid-cols-[48px_minmax(0,_1fr)_40px_16px] @max-lg:[&_button]:gap-y-1">
              {runs.map((attempt, index) => (
                <button
                  key={attempt.id}
                  aria-current={attempt.id === run?.id ? true : undefined}
                  className="focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:-outline-offset-2"
                  {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
                    onOpenRun(attempt.id, intent),
                  )}
                >
                  <span className="experiment-run-number text-xs font-medium">{m.experiment_overview_run()} {runs.length - index}</span>
                  <StatusBadge {...runDisplay(attempt)} className="min-w-0" />
                  <span className="truncate text-ink-subtext @max-lg:order-last @max-lg:col-start-2 @max-lg:col-span-3 @max-lg:line-clamp-2 @max-lg:whitespace-normal @max-lg:empty:hidden">{historySentence(attempt, now)}</span>
                  <span className="@max-lg:hidden">{timeAgo(attempt.createdAt)}</span>
                  <span>{runDuration(attempt, now)}</span>
                  <ArrowUpRight size={14} aria-hidden="true" />
                </button>
              ))}
            </div>
          </section>
        )}

        {run?.monitor && <WatchSection key={run.id} run={run} chatSessionId={experiment.chatSessionId} now={now} className={EXPERIMENT_OVERVIEW_SECTION_CLASS_NAME} />}

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
          {experiment.runCommand !== run?.command && (
            <code className={EXPERIMENT_OVERVIEW_COMMAND_CLASS_NAME}>$ {experiment.runCommand}</code>
          )}
        </section>

        {run?.monitor && <RunDetails key={`details:${run.id}`} run={run} className="mt-5.5 border-t border-t-border-variant pt-4.5" />}
      </div>
    </div>
    </ChatImageScope>
  );
}
