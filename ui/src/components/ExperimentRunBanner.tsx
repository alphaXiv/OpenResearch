import { useMutate } from "@clo/react-mutation";
import { useWorkspace } from "../workspace";
import { mutCancelRun } from "../queries/projects";
import { mutStopBackgroundTask } from "../queries/chat";
import { ChevronRight, CircleStop, FlaskConical, Hourglass, ScrollText } from "lucide-react";
import { useEffect, useId, useState } from "react";
import { fmtDuration, fmtNumber, type ChatActiveRun, type ChatSession } from "../api";
import { autoDir } from "../i18n";
import { m } from "../paraglide/messages.js";
import { runDisplay, runNotice, runStoppable } from "../runDisplay";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";
import { RunContactNotice } from "./RunContactNotice";
import { statusLabel } from "./StatusBadge";
import { Button } from "./ui";

type OpenTarget = (id: string, intent: TabOpenIntent) => void;
const BANNER_CLASS = "mb-2.5 rounded-lg border border-border bg-accent-green-subtle px-3 py-2 text-sm text-text";

function RunRow({ item, onOpenRun, onOpenExperiment }: { item: ChatActiveRun; onOpenRun?: OpenTarget; onOpenExperiment?: OpenTarget }) {
  const workspace = useWorkspace();
  const cancel = useMutate(mutCancelRun);

  const { run, experimentName } = item;
  const [now, setNow] = useState(Date.now);
  const display = runDisplay(run);
  const error = cancel.error === undefined ? null : cancel.error instanceof Error ? cancel.error.message : String(cancel.error);
  const label = display.stale || run.cancelling
    ? statusLabel(display.status, display.stopping)
    : run.status === "running" ? m.chat_experiment_running() : m.chat_experiment_starting();
  const noticeKind = runNotice(run)?.kind;
  const showNotice = noticeKind !== undefined && noticeKind !== "monitoring" && noticeKind !== "cancelling";

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  return (
    <div className="flex min-w-0 flex-wrap items-center gap-2">
      <FlaskConical size={16} className="shrink-0 text-accent-green" aria-hidden="true" />
      <span role="status" className="shrink-0 font-medium">{label}</span>
      <span aria-hidden="true">·</span>
      {onOpenExperiment ? (
        <button className="tool-target experiment-run-link min-w-0 truncate" {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => onOpenExperiment(run.experimentId, intent))}>
          {experimentName || run.experimentId}
        </button>
      ) : <span className="min-w-0 truncate">{experimentName || run.experimentId}</span>}
      <div className="ms-auto flex shrink-0 items-center gap-1">
        <span className="me-2 tabular-nums text-subtext">{fmtDuration(now - run.createdAt)}</span>
        {onOpenRun && <Button size="small" variant="ghost" {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => onOpenRun(run.id, intent))}><ScrollText size={14} />{m.experiment_overview_logs()}</Button>}
        {runStoppable(run) && (
          <Button size="small" variant="ghost" onClick={() => cancel.run(workspace, run)}>
            <CircleStop size={14} />{m.common_stop()}
          </Button>
        )}
      </div>
      {display.waiting && <span className="w-full min-w-0 truncate ps-6 text-subtext" title={display.waiting}>{autoDir(display.waiting)}</span>}
      {error && <span className="w-full text-accent-red" role="alert">{error}</span>}
      {showNotice && <RunContactNotice run={run} className="w-full" />}
    </div>
  );
}

export function ExperimentRunBanner({ runs, onOpenRun, onOpenExperiment }: { runs: ChatActiveRun[]; onOpenRun?: OpenTarget; onOpenExperiment?: OpenTarget }) {
  if (runs.length === 0) return null;
  if (runs.length === 1) return <div className={BANNER_CLASS}><RunRow item={runs[0]} onOpenRun={onOpenRun} onOpenExperiment={onOpenExperiment} /></div>;
  return (
    <details className={`${BANNER_CLASS} [&[open]_.experiment-chevron]:rotate-90`}>
      <summary className="flex cursor-pointer list-none items-center gap-2 rounded-sm focus-visible:outline-2 focus-visible:outline-focus [&::-webkit-details-marker]:hidden">
        <FlaskConical size={16} className="shrink-0 text-accent-green" aria-hidden="true" />
        <span className="font-medium">{m.chat_experiments_running({ count: runs.length })}</span>
        <ChevronRight size={14} className="experiment-chevron ms-auto shrink-0 text-subtext" aria-hidden="true" />
      </summary>
      <div className="mt-2 flex max-h-48 flex-col gap-2 overflow-y-auto">
        {runs.map((item) => <RunRow key={item.run.id} item={item} onOpenRun={onOpenRun} onOpenExperiment={onOpenExperiment} />)}
      </div>
    </details>
  );
}

type BackgroundTask = NonNullable<NonNullable<ChatSession["background"]>["tasks"]>[number];

function BackgroundTaskStop({ sessionId, taskId, nameId }: { sessionId: string; taskId: string; nameId?: string }) {
  const workspace = useWorkspace();
  const stop = useMutate(mutStopBackgroundTask);
  const labelId = useId();
  const error = stop.error === undefined ? null : stop.error instanceof Error ? stop.error.message : String(stop.error);
  return (
    <>
      <Button
        size="small"
        variant="ghost"
        aria-labelledby={nameId ? `${labelId} ${nameId}` : undefined}
        disabled={stop.isPending}
        onClick={() => stop.run(workspace, sessionId, taskId)}
      >
        <CircleStop size={14} aria-hidden="true" />
        <span id={labelId}>{m.common_stop()}</span>
      </Button>
      {error && <span className="w-full text-accent-red" role="alert">{error}</span>}
    </>
  );
}

const taskName = (task: BackgroundTask) => task.name || (task.chat ? m.chat_untitled() : "");

export function BackgroundTaskBanner({ sessionId, background }: { sessionId: string; background: ChatSession["background"] }) {
  const [now, setNow] = useState(Date.now);
  const nameId = useId();
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  if (!background) return null;
  const tasks = background.tasks ?? [];
  const elapsed = <span className="ms-auto shrink-0 tabular-nums text-subtext">{fmtDuration(now - background.since)}</span>;
  const icon = <Hourglass size={16} className="shrink-0 text-accent-blue" aria-hidden="true" />;
  const className = "mb-2.5 rounded-lg border border-border bg-accent-blue-subtle px-3 py-2 text-sm text-text";

  if (tasks.length > 1) {
    return (
      <details className={`${className} [&[open]_.background-chevron]:rotate-90`}>
        <summary className="flex min-h-6 cursor-pointer list-none items-center gap-2 rounded-sm focus-visible:outline-2 focus-visible:outline-focus [&::-webkit-details-marker]:hidden">
          {icon}
          <span role="status" className="font-medium">{m.chat_status_background_tasks({ count: fmtNumber(tasks.length) })}</span>
          {elapsed}
          <ChevronRight size={14} className="background-chevron shrink-0 text-subtext" aria-hidden="true" />
        </summary>
        <div className="mt-2 flex max-h-48 flex-col gap-1 overflow-y-auto">
          {tasks.map((task, index) => {
            const name = taskName(task);
            const id = `${nameId}-${index}`;
            return (
              <div key={task.id} className="flex min-w-0 flex-wrap items-center gap-2 ps-6">
                <span id={id} className="min-w-0 flex-1 truncate" title={name}>{autoDir(name)}</span>
                <BackgroundTaskStop sessionId={sessionId} taskId={task.id} nameId={name ? id : undefined} />
              </div>
            );
          })}
        </div>
      </details>
    );
  }

  const task = tasks[0];
  const name = task ? taskName(task) : "";
  return (
    <div className={`${className} flex min-w-0 flex-wrap items-center gap-2`}>
      {icon}
      <span role="status" className="shrink-0 font-medium">
        {background.count > 1 ? m.chat_status_background_tasks({ count: fmtNumber(background.count) }) : m.chat_status_background_task()}
      </span>
      {name && (
        <>
          <span aria-hidden="true">·</span>
          <span id={nameId} className="min-w-0 truncate text-subtext" title={name}>{autoDir(name)}</span>
        </>
      )}
      {elapsed}
      {task && <BackgroundTaskStop key={task.id} sessionId={sessionId} taskId={task.id} nameId={name ? nameId : undefined} />}
    </div>
  );
}
