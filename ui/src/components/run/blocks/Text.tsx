import { fillSentence } from "../../../runMonitor/format";
import type { Tone } from "../../../runMonitor/model";
import type { RunMonitor } from "../../../runMonitor/types";
import { Sentence } from "../Sentence";

export function Text({ monitor, template, tone }: { monitor: RunMonitor; template: string; tone: Tone }) {
  return <p className="m-0 wrap-anywhere leading-relaxed"><Sentence text={fillSentence(template, monitor.latest, monitor.view.signals)} tone={tone} /></p>;
}
