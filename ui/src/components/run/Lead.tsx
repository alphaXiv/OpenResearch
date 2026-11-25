import { TONE_INK, type RunModel } from "../../runMonitor/model";
import { cn } from "../ui/cn";

/** The state a run ended in, or is in, before its sentence. */
export function Lead({ model, stop = false }: { model: RunModel; stop?: boolean }) {
  if (!model.lead) return null;
  const sizing = stop ? null : model.tone === "failed" || model.notice ? "shrink-0" : "min-w-0 shrink truncate";
  return <span className={cn("font-medium", TONE_INK[model.tone], sizing)}>{model.lead}{stop ? ". " : "\u00a0"}</span>;
}
