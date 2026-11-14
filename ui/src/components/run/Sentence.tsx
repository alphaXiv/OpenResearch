import { Fragment } from "react";
import { TONE_INK, type Tone } from "../../runMonitor/model";

/** A sentence whose `**figure**`s draw bold in the run's tone. */
export function Sentence({ text, tone }: { text: string; tone: Tone }) {
  return (
    <span className="tabular-nums">
      {text.split(/(\*\*[^*]+\*\*)/).map((part, index) => part.startsWith("**") && part.endsWith("**") && part.length > 4
        ? <strong key={index} className={`font-semibold ${TONE_INK[tone]}`}>{part.slice(2, -2)}</strong>
        : <Fragment key={index}>{part}</Fragment>)}
    </span>
  );
}
