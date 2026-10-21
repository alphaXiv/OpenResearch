import { ChevronRight, FlaskConical } from "lucide-react";
import type { ChatActiveRun } from "../api";
import { m } from "../paraglide/messages.js";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";

type OpenRun = (id: string, intent: TabOpenIntent) => void;
const BANNER_CLASS = "mb-2.5 rounded-md border border-border border-s-3 border-s-accent-green bg-accent-green-subtle px-3 py-2.5 text-sm text-text shadow-plan";

function ExperimentRunLink({ item, onOpenRun, className = "" }: { item: ChatActiveRun; onOpenRun?: OpenRun; className?: string }) {
  const { run, experimentName } = item;
  return onOpenRun ? (
    <button className={`tool-target experiment-run-link ${className}`} {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => onOpenRun(run.id, intent))}>
      {experimentName || run.experimentId}
    </button>
  ) : <span className={className}>{experimentName || run.experimentId}</span>;
}

export function ExperimentRunBanner({ runs, onOpenRun }: { runs: ChatActiveRun[]; onOpenRun?: OpenRun }) {
  const first = runs[0];
  if (!first) return null;
  if (runs.length === 1) {
    return (
      <div role="status" className={`${BANNER_CLASS} flex items-center gap-2`}>
        <FlaskConical size={16} className="shrink-0 text-accent-green" aria-hidden="true" />
        <span className="shrink-0 font-medium">{first.run.cancelRequested ? m.chat_experiment_cancelling() : first.run.status === "starting" ? m.chat_experiment_starting() : m.chat_experiment_running()}</span>
        <span aria-hidden="true">·</span>
        <ExperimentRunLink item={first} onOpenRun={onOpenRun} className="min-w-0 truncate" />
      </div>
    );
  }
  return (
    <details className={`${BANNER_CLASS} [&[open]_.experiment-chevron]:rotate-90`}>
      <summary className="flex cursor-pointer list-none items-center gap-2 rounded-sm focus-visible:outline-2 focus-visible:outline-text [&::-webkit-details-marker]:hidden">
        <FlaskConical size={16} className="shrink-0 text-accent-green" aria-hidden="true" />
        <span className="font-medium">{m.chat_experiments_running({ count: runs.length })}</span>
        <ChevronRight size={14} className="experiment-chevron ms-auto shrink-0 text-subtext" aria-hidden="true" />
      </summary>
      <div className="mt-2 max-h-48 overflow-y-auto ps-6 leading-relaxed">
        {runs.map((item, index) => (
          <span key={item.run.id}>
            {index > 0 && <span className="text-subtext"> · </span>}
            <ExperimentRunLink item={item} onOpenRun={onOpenRun} />
          </span>
        ))}
      </div>
    </details>
  );
}
