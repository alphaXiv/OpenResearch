import { Check, Minus } from "lucide-react";
import { autoDir } from "../../../i18n";
import { durationText } from "../../../runMonitor/words";
import { m } from "../../../paraglide/messages.js";
import { TONE_BG, type RunModel } from "../../../runMonitor/model";
import type { RunMonitor } from "../../../runMonitor/types";
import { cn } from "../../ui/cn";

/** A run's stages, with how long each took or is expected to; a declared plan's skipped and unplanned stages say so. */
export function Phases({ model, monitor }: { model: RunModel; monitor: RunMonitor }) {
  const current = monitor.progress?.phase ?? 0;
  return (
    <ol className="m-0 flex list-none flex-wrap gap-x-4 gap-y-1 p-0 text-xs">
      {monitor.phases.map((phase, index) => {
        const skipped = phase.skipped === true;
        const past = !skipped && (phase.endedAt !== null || index < current || model.tone === "done");
        const now = !past && !skipped && index === current;
        const took = phase.startedAt !== null && phase.endedAt !== null ? durationText(phase.endedAt - phase.startedAt) : "";
        const expected = model.live && !past && !now && phase.priorMs !== null ? m.run_eta_about({ duration: durationText(phase.priorMs) }) : "";
        const note = [skipped ? m.run_stage_skipped() : past ? took : expected, phase.planned === false ? m.run_stage_unplanned() : ""].filter(Boolean).join(" · ");
        return (
          <li key={`${phase.name}-${index}`} className={cn("inline-flex items-center gap-1.5", now ? "font-medium text-text" : "text-ink-subtext")}>
            {past ? <Check size={12} aria-hidden="true" />
              : skipped ? <Minus size={12} aria-hidden="true" />
              : <i aria-hidden="true" className={cn("mx-[2.5px] size-[7px] rounded-full", now ? cn(TONE_BG[model.tone], model.live && "motion-safe:animate-[or-pulse_1.2s_ease-in-out_infinite]") : "border-[1.5px] border-current")} />}
            {autoDir(phase.name)}
            {note && <span className="font-normal tabular-nums text-ink-subtext">{note}</span>}
          </li>
        );
      })}
    </ol>
  );
}
