import { TONE_INK, type RunModel } from "../../runMonitor/model";
import { cn } from "../ui/cn";

/** The state a run ended in, or is in, before its sentence. */
export function Lead({ model, stop = false }: { model: RunModel; stop?: boolean }) {
  if (!model.lead) return null;
  return <span className={cn("font-medium", TONE_INK[model.tone])}>{model.lead}{stop ? ". " : " "}</span>;
}
