import { m } from "../../../paraglide/messages.js";
import { cn } from "../../ui/cn";
import type { Series } from "./series";

/** Which line is which, when a chart has more than one: stacked beside a strip's chart, in a row over a page's. */
export function Legend({ series, row = false }: { series: Series[]; row?: boolean }) {
  const parent = series.find((line) => line.ghost);
  const named = series.filter((line) => !line.ghost || line === parent);
  if (named.length < 2) return null;
  return (
    <span className={cn("flex text-xs text-ink-subtext", row ? "flex-wrap items-center gap-x-3.5 gap-y-1" : "flex-col items-start gap-y-0.5")} aria-hidden="true">
      {named.map((line) => (
        <span key={line.name} className="inline-flex items-center gap-1.5">
          <svg width="14" height="2" className="shrink-0"><line x1="0" x2="14" y1="1" y2="1" stroke={line.color} strokeWidth="2" strokeDasharray={line.ghost ? "4 3" : undefined} /></svg>
          {line.ghost ? m.run_chart_parent() : line.label}
        </span>
      ))}
    </span>
  );
}
