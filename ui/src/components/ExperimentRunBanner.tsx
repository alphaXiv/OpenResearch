import { ChevronRight, CircleStop, FlaskConical, ScrollText } from "lucide-react";
import { useEffect, useState } from "react";
import { cancelRun, fmtDuration, type ChatActiveRun } from "../api";
import { m } from "../paraglide/messages.js";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";
import { Button } from "./ui";

type OpenTarget = (id: string, intent: TabOpenIntent) => void;
const BANNER_CLASS = "mb-2.5 rounded-lg border border-border bg-accent-green-subtle px-3 py-2 text-sm text-text";

function RunRow({ item, onOpenRun, onOpenExperiment }: { item: ChatActiveRun; onOpenRun?: OpenTarget; onOpenExperiment?: OpenTarget }) {
  const { run, experimentName } = item;
  const [now, setNow] = useState(Date.now);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const cancelling = pending || run.cancelRequested;

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  async function stop() {
    setPending(true);
    setError(null);
    try {
      await cancelRun(run.id);
    } catch (err) {
      setPending(false);
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <div className="flex min-w-0 flex-wrap items-center gap-2">
      <FlaskConical size={16} className="shrink-0 text-accent-green" aria-hidden="true" />
      <span role="status" className="shrink-0 font-medium">{cancelling ? m.chat_experiment_cancelling() : run.status === "starting" ? m.chat_experiment_starting() : m.chat_experiment_running()}</span>
      <span aria-hidden="true">·</span>
      {onOpenExperiment ? (
        <button className="tool-target experiment-run-link min-w-0 truncate" {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => onOpenExperiment(run.experimentId, intent))}>
          {experimentName || run.experimentId}
        </button>
      ) : <span className="min-w-0 truncate">{experimentName || run.experimentId}</span>}
      <div className="ms-auto flex shrink-0 items-center gap-1">
        <span className="me-2 tabular-nums text-subtext">{fmtDuration(now - run.createdAt)}</span>
        {onOpenRun && <Button size="small" variant="ghost" {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => onOpenRun(run.id, intent))}><ScrollText size={14} />{m.experiment_overview_logs()}</Button>}
        <Button size="small" variant="ghost" disabled={cancelling} onClick={() => void stop()}><CircleStop size={14} />{cancelling ? m.common_cancelling() : m.common_stop()}</Button>
      </div>
      {error && <span className="w-full text-accent-red" role="alert">{error}</span>}
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
