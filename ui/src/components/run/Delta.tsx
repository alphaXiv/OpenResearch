import { m } from "../../paraglide/messages.js";
import type { Delta as DeltaValue } from "../../runMonitor/model";
import { cn } from "../ui/cn";

/**
 * Better or worse than the parent at the same point; the arrow is the way the number moved. `said` writes better or
 * worse out where there is room, instead of leaving it to the colour.
 */
export function Delta({ delta, said = false, className }: { delta: DeltaValue; said?: boolean; className?: string }) {
  const words = delta.good ? m.run_vs_better({ delta: delta.magnitude }) : m.run_vs_worse({ delta: delta.magnitude });
  if (delta.flat) return <span className={cn("text-ink-subtext", className)}>{m.run_level_with_parent()}</span>;
  return (
    <span className={cn("inline-flex items-center gap-0.5 tabular-nums", delta.good ? "text-ink-green" : "text-ink-red", className)}>
      <svg width="8" height="8" viewBox="0 0 8 8" aria-hidden="true"><path d={delta.down ? "M0 1.5h8L4 7.5z" : "M0 6.5h8L4 .5z"} fill="currentColor" /></svg>
      {said ? <span className="ms-0.5">{words}</span> : (
        <>
          <span aria-hidden="true">{delta.magnitude}</span>
          <span className="sr-only">{words}</span>
        </>
      )}
    </span>
  );
}
