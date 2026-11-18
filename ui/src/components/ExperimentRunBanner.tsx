import { useMutate } from "@clo/react-mutation";
import { useWorkspace } from "../workspace";
import { mutCancelRun } from "../queries/projects";
import { ChevronRight, CircleStop, FlaskConical, Hourglass, ScrollText } from "lucide-react";
import { useEffect, useState } from "react";
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

/** Tasks the agent left running when its turn ended; it picks the chat back up when they finish. */
export function BackgroundTaskBanner({ background }: { background: ChatSession["background"] }) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  if (!background) return null;
  return (
    <div className="mb-2.5 flex min-w-0 items-center gap-2 rounded-lg border border-border bg-accent-blue-subtle px-3 py-2 text-sm text-text">
      <Hourglass size={16} className="shrink-0 text-accent-blue" aria-hidden="true" />
      <span role="status" className="shrink-0 font-medium">
        {background.count > 1 ? m.chat_status_background_tasks({ count: fmtNumber(background.count) }) : m.chat_status_background_task()}
      </span>
      <span aria-hidden="true">·</span>
      <span className="min-w-0 truncate text-subtext">{m.chat_background_resumes()}</span>
      <span className="ms-auto shrink-0 tabular-nums text-subtext">{fmtDuration(now - background.since)}</span>
    </div>
  );
}
