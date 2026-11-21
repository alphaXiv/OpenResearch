import { useQuery } from "@tanstack/react-query";
import { ChevronRight } from "lucide-react";
import { type Run } from "../../api";
import { autoDir } from "../../i18n";
import { m } from "../../paraglide/messages.js";
import { runActive } from "../../runDisplay";
import type { BriefLabel } from "../../runMonitor/types";
import { useWorkspace } from "../../workspace";
import { cn } from "../ui/cn";
import { Rules } from "./Rules";
import { ViewSource } from "./ViewSource";

const BRIEF: Record<BriefLabel, () => string> = {
  question: m.run_brief_question,
  healthy: m.run_brief_healthy,
  noise: m.run_brief_noise,
  worth_a_flag: m.run_brief_worth_a_flag,
  from_parent: m.run_brief_from_parent,
  decided: m.run_brief_decided,
};

const HEADING = "m-0 mb-2 text-xs font-medium text-ink-subtext";

export function RunDetails({ run, className }: { run: Run; className?: string }) {
  const { getRunMonitorQuery } = useWorkspace().queries;
  const { data: monitor, isError } = useQuery({ ...getRunMonitorQuery(run.id), enabled: false });
  if (!monitor) return null;
  const told = (monitor.brief ?? []).filter((line) => Object.hasOwn(BRIEF, line.label));
  return (
    <details className={cn("group text-sm", className)}>
      <summary className="inline-flex min-h-6 cursor-pointer list-none items-center gap-1 rounded-sm text-ink-subtext hover:text-text focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus [&::-webkit-details-marker]:hidden">
        <ChevronRight size={14} aria-hidden="true" className="shrink-0 motion-safe:transition-transform rtl:rotate-180 group-open:rotate-90 rtl:group-open:rotate-90" />
        {m.run_details()}
      </summary>
      <div className="mt-3.5 flex flex-col gap-6">
        <Rules rules={monitor.rules} outcome={run.outcome ?? null} heading={HEADING} />
        {told.length > 0 && (
          <div>
            <h3 className={HEADING}>{m.run_watch_told()}</h3>
            <dl className="m-0 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5">
              {told.map((line) => (
                <div key={line.label} className="contents">
                  <dt className="text-xs text-ink-subtext">{BRIEF[line.label]()}</dt>
                  <dd className="m-0 wrap-anywhere text-text">{autoDir(line.text)}</dd>
                </div>
              ))}
            </dl>
          </div>
        )}
        <ViewSource monitor={monitor} projectId={run.projectId} live={runActive(run.status)} canSayEmpty={!isError && monitor.availability.state === "live" && !monitor.availability.collectionIncomplete} heading={HEADING} />
      </div>
    </details>
  );
}
