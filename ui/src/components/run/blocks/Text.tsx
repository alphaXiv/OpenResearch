import { fillSentence } from "../../../runMonitor/format";
import { bindingsOf, type Tone } from "../../../runMonitor/model";
import type { RunMonitor } from "../../../runMonitor/types";
import { Sentence } from "../Sentence";

/** The agent's own sentence, its numbers bound to the run's signals. A page block only. */
export function Text({ monitor, template, tone }: { monitor: RunMonitor; template: string; tone: Tone }) {
  return <p className="m-0 leading-relaxed"><Sentence text={fillSentence(template, bindingsOf(monitor), monitor.view.signals)} tone={tone} /></p>;
}
