import type { RunModel } from "../../runMonitor/model";
import { Sentence } from "./Sentence";

/** Under the strip: how far, the time left and its range, what it has cost. */
export function Captions({ model }: { model: RunModel }) {
  return (
    <div className="flex flex-wrap justify-between gap-x-5 gap-y-0.5 text-xs tabular-nums text-ink-subtext">
      <span><Sentence text={model.progress} tone={model.tone} /></span>
      <span>
        {model.etaStrong ? <span className="font-medium text-text">{model.eta}</span> : model.eta}
        {model.eta && model.etaDetail ? " · " : ""}
        {model.etaDetail}
      </span>
      {model.cost && <span>{model.cost}</span>}
    </div>
  );
}
