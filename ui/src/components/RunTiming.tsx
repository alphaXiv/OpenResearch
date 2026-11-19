import { Clock3 } from "lucide-react";
import { useEffect, useState } from "react";
import { timeAgo, type Run } from "../api";
import { m } from "../paraglide/messages.js";
import { runActive, runDuration } from "../runDisplay";

export function useRunClock(ticking: boolean): number {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!ticking) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [ticking]);
  return now;
}

export function RunTiming({ run, className = "inline-flex" }: { run: Run; className?: string }) {
  const now = useRunClock(run.endedAt == null && runActive(run.status));
  const label = m.experiment_overview_duration();

  return (
    <span className={`min-w-0 items-center gap-2 text-text tabular-nums ${className}`}>
      <span className="inline-flex shrink-0 items-center gap-1 whitespace-nowrap" title={label}>
        <Clock3 size={12} aria-hidden="true" />
        <span className="sr-only">{label} </span>
        {runDuration(run, now)}
      </span>
      <span className="truncate @max-lg:hidden">{timeAgo(run.createdAt)}</span>
    </span>
  );
}
