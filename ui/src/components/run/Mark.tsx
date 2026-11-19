import { TriangleAlert } from "lucide-react";
import { TONE_BG, type Tone } from "../../runMonitor/model";
import { cn } from "../ui/cn";

/** A dot in the run's tone, pulsing while it runs; a flag is the amber warning. */
export function Mark({ tone, live, label }: { tone: Tone; live: boolean; label: string }) {
  if (tone === "flag") return <TriangleAlert size={14} className="shrink-0 text-mark-amber" role="img" aria-label={label} />;
  const pulse = (live && tone !== "lost") || tone === "revising";
  return (
    <span
      role="img"
      aria-label={label}
      className={cn("inline-block size-[7px] shrink-0 rounded-full", TONE_BG[tone], pulse && "motion-safe:animate-[or-pulse_1.2s_ease-in-out_infinite]")}
    />
  );
}
