import { useQuery } from "@tanstack/react-query";
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
export function ViewSource({ monitor, projectId, live, canSayEmpty, heading }: { monitor: RunMonitor; projectId: string; live: boolean; canSayEmpty: boolean; heading: string }) {
  const { listExperimentsQuery } = useWorkspace().queries;
  const experiments = useQuery(listExperimentsQuery(projectId)).data;
  const { viewFrom, source } = monitor;
  const owner = experiments?.find((experiment) => experiment.id === viewFrom.experimentId)?.slug ?? viewFrom.experimentId;
  const sourceText = source === null
    ? canSayEmpty ? (live ? m.run_view_source_none() : m.run_page_no_signals_ended()) : null
    : Object.hasOwn(SOURCE, source) ? SOURCE[source]() : null;
  return (
    <div className="@container">
      <h3 className={heading}>{m.run_view_how()}</h3>
      <div className="grid gap-3 @2xl:grid-cols-2">
        <div className="min-w-0">
          <p className="m-0 mb-1.5 text-xs text-ink-subtext">{viewFrom.isDefault ? m.run_view_default() : m.run_view_stored({ experiment: ltr(owner) })}</p>
          {viewFrom.markup && <pre dir="ltr" className={PRE}>{viewFrom.markup}</pre>}
        </div>
        <div className="min-w-0">
          {sourceText && <p className="m-0 mb-1.5 text-xs text-ink-subtext">{sourceText}</p>}
          {source === "pattern" && monitor.pattern && <pre dir="ltr" className={PRE}>{monitor.pattern}</pre>}
          <p className="m-0 mb-1.5 mt-3 text-xs text-ink-subtext">{m.run_view_as_text()}</p>
          <pre dir="ltr" className={PRE}>{monitor.statusText}</pre>
        </div>
      </div>
    </div>
  );
}
