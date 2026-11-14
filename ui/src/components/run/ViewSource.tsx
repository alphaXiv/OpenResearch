import { useQuery } from "@tanstack/react-query";
import { ChevronRight } from "lucide-react";
import { ltr } from "../../i18n";
import { m } from "../../paraglide/messages.js";
import type { RunMonitor } from "../../runMonitor/types";
import { useWorkspace } from "../../workspace";

const PRE = "m-0 whitespace-pre-wrap rounded-md border border-border-variant bg-surface px-3 py-2.5 font-mono text-xs leading-relaxed text-text [overflow-wrap:anywhere]";

const SOURCE: Record<string, () => string> = {
  declared: m.run_view_source_declared,
  pattern: m.run_view_source_pattern,
  detected: m.run_view_source_detected,
  trackio: m.run_view_source_trackio,
};

/** How the run's view is built: the view itself, where its signals come from, and what an agent reads instead. */
export function ViewSource({ monitor, projectId, live }: { monitor: RunMonitor; projectId: string; live: boolean }) {
  const { listExperimentsQuery } = useWorkspace().queries;
  const experiments = useQuery(listExperimentsQuery(projectId)).data;
  const { viewFrom, source } = monitor;
  const owner = experiments?.find((experiment) => experiment.id === viewFrom.experimentId)?.slug ?? viewFrom.experimentId;
  return (
    <details className="group @container mt-3.5 text-sm">
      <summary className="inline-flex cursor-pointer list-none items-center gap-1 rounded-sm text-ink-subtext hover:text-text focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus [&::-webkit-details-marker]:hidden">
        <ChevronRight size={14} aria-hidden="true" className="shrink-0 motion-safe:transition-transform rtl:rotate-180 group-open:rotate-90 rtl:group-open:rotate-90" />
        {m.run_view_how()}
      </summary>
      <div className="mt-2.5 grid gap-3 @2xl:grid-cols-2">
        <div className="min-w-0">
          <p className="m-0 mb-1.5 text-xs text-ink-subtext">{viewFrom.isDefault ? m.run_view_default() : m.run_view_stored({ experiment: ltr(owner) })}</p>
          {viewFrom.markup && <pre dir="ltr" className={PRE}>{viewFrom.markup}</pre>}
        </div>
        <div className="min-w-0">
          {/* A source from a newer server goes unnamed. */}
          {(source === null || Object.hasOwn(SOURCE, source)) && <p className="m-0 mb-1.5 text-xs text-ink-subtext">{source ? SOURCE[source]() : live ? m.run_view_source_none() : m.run_page_no_signals_ended()}</p>}
          {source === "pattern" && monitor.pattern && <pre dir="ltr" className={PRE}>{monitor.pattern}</pre>}
          {monitor.newestLine && (
            <>
              <p className="m-0 mb-1.5 mt-3 text-xs text-ink-subtext">{m.run_view_newest_line()}</p>
              <pre dir="ltr" className={PRE}>{monitor.newestLine}</pre>
            </>
          )}
          <p className="m-0 mb-1.5 mt-3 text-xs text-ink-subtext">{m.run_view_as_text()}</p>
          <pre dir="ltr" className={PRE}>{monitor.statusText}</pre>
        </div>
      </div>
    </details>
  );
}
