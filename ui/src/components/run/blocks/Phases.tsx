import { Check, Minus } from "lucide-react";
import { autoDir } from "../../../i18n";
import { durationText } from "../../../runMonitor/words";
import { m } from "../../../paraglide/messages.js";
import { TONE_BG, type RunModel } from "../../../runMonitor/model";
import type { Phase } from "../../../runMonitor/types";
import { cn } from "../../ui/cn";

/** A run's stages, with how long each took or is expected to; a declared plan's skipped and unplanned stages say so. */
export function Phases({ model, phases, current, progress, setupNames, className }: { model: RunModel; phases: Phase[]; current: number | null; progress?: string; setupNames?: string[]; className?: string }) {
  return (
    <ol className={cn("m-0 flex list-none flex-wrap gap-x-4 gap-y-1 p-0 text-xs", className)}>
      {phases.map((phase, index) => {
        const skipped = phase.skipped === true;
        const now = !skipped && index === current;
        const past = !skipped && !now && (phase.endedAt !== null || current !== null && index < current || model.run.status === "done");
        const took = phase.startedAt !== null && phase.endedAt !== null ? durationText(phase.endedAt - phase.startedAt) : "";
        const expected = model.live && !past && !now && phase.priorMs !== null ? m.run_eta_about({ duration: durationText(phase.priorMs) }) : "";
        const note = [skipped ? m.run_stage_skipped() : now ? progress : past ? took : expected, phase.planned === false ? m.run_stage_unplanned() : ""].filter(Boolean).join(" · ");
        return (
          <li key={`${phase.name}-${index}`} aria-current={now ? "step" : undefined} className={cn("inline-flex items-center gap-1.5", now ? "font-medium text-text" : "text-ink-subtext")}>
            {past ? <Check size={12} role="img" aria-label={m.status_done()} />
              : skipped ? <Minus size={12} aria-hidden="true" />
              : <i aria-hidden="true" className={cn("mx-[2.5px] size-[7px] rounded-full", now ? cn(TONE_BG[model.tone], model.live && !model.bar.frozen && "motion-safe:animate-[or-pulse_1.2s_ease-in-out_infinite]") : "border-[1.5px] border-current")} />}
            <span className="min-w-0">{autoDir(phase.name)}
              {index === 0 && setupNames && setupNames.length > 0 && <span className="mt-0.5 block font-normal text-ink-subtext">{setupNames.map(autoDir).join(" · ")}</span>}
            </span>
            {note && <span className="ms-auto font-normal tabular-nums text-ink-subtext">{note}</span>}
          </li>
        );
      })}
    </ol>
  );
}
