import { useMutate } from "@clo/react-mutation";
import { ChevronDown, ChevronUp, X } from "lucide-react";
import { Fragment, useLayoutEffect, useState } from "react";
import { autoDir } from "../../i18n";
import { m } from "../../paraglide/messages.js";
import { mutDismissShownRun } from "../../queries/projects";
import { TONE_INK, type RunModel } from "../../runMonitor/model";
import type { SummaryPart } from "../../runMonitor/summary";
import { useWorkspace } from "../../workspace";
import { IconButton, Tooltip } from "../ui";
import { cn } from "../ui/cn";
import { Mark } from "./Mark";
import { Rail } from "./Rail";
import { Sentence } from "./Sentence";
import { useBoxSize } from "./useBoxSize";

/** One line for several runs: their marks, what they share, who needs attention or leads, and every bar stacked under it. */
export function RunSummary({ models, context, parts, firstDone, open, onToggle }: {
  models: RunModel[];
  context: { whole: string; short: string } | null;
  parts: SummaryPart[];
  firstDone: string | null;
  open: boolean;
  onToggle: () => void;
}) {
  const workspace = useWorkspace();
  const dismiss = useMutate(mutDismissShownRun);
  const toggle = open ? m.run_show_less() : m.run_show_each();
  // Runs launched together stay one summary until each is answered, or until it is dismissed whole.
  const dismissable = models.every((model) => model.canDismiss);

  // The shared words name every run when they fit beside the summary at its natural width.
  // Measured after every render, and each resize renders again.
  const [row] = useBoxSize<HTMLDivElement>();
  const [fits, setFits] = useState(true);
  useLayoutEffect(() => {
    const box = row.current;
    if (!box || !context) return;
    const [shared, whole, sentence] = ["shared", "whole", "sentence"].map((part) => box.querySelector<HTMLElement>(`[data-part="${part}"]`)!);
    const words = document.createRange();
    words.selectNodeContents(sentence);
    const natural = Math.ceil(words.getBoundingClientRect().width);
    setFits(whole.offsetWidth + natural <= shared.clientWidth + sentence.clientWidth);
  });
  return (
    <div className="relative">
      <div ref={row} className="flex min-h-9 min-w-0 items-center gap-x-2 py-1 ps-3 pe-1.5">
        <span className="flex max-w-1/4 shrink-0 items-center gap-1 overflow-hidden">
          {models.map((model) => <Mark key={model.run.id} tone={model.tone} live={model.live} label={model.status} />)}
        </span>
        {context && <>
          <span data-part="shared" className="min-w-0 shrink-[3] truncate text-ink-subtext">{autoDir(fits ? context.whole : context.short)}</span>
          <span data-part="whole" aria-hidden="true" className="invisible absolute whitespace-nowrap">{autoDir(context.whole)}</span>
        </>}
        <button
          data-part="sentence"
          className="min-h-6 min-w-6 flex-[1_1_auto] truncate rounded-xs text-start focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:outline-offset-2"
          aria-expanded={open}
          onClick={onToggle}
        >
          {parts.map((part, index) => (
            <Fragment key={index}>
              {index > 0 && " · "}
              {part.who && <span className={cn("font-medium", TONE_INK[part.tone])}>{part.who} </span>}
              <Sentence text={autoDir(part.text)} tone={part.tone} />
            </Fragment>
          ))}
        </button>
        {firstDone && <span className="shrink-0 whitespace-nowrap tabular-nums text-ink-subtext @max-[30rem]:hidden">{firstDone}</span>}
        {dismissable && (
          <Tooltip interactive content={m.run_dismiss_all()}>
            <IconButton size="small" aria-label={m.run_dismiss_all()} onClick={() => {
              for (const model of models) dismiss.run(workspace, model.sessionId, model.run.id);
            }}>
              <X size={14} />
            </IconButton>
          </Tooltip>
        )}
        <Tooltip interactive content={toggle}>
          <IconButton size="small" aria-label={toggle} aria-expanded={open} data-run-toggle="*" onClick={onToggle}>
            {open ? <ChevronDown size={14} /> : <ChevronUp size={14} />}
          </IconButton>
        </Tooltip>
      </div>
      {!open && (
        <div className="flex flex-col gap-px">
          {models.filter((model) => model.live).map((model) => <div key={model.run.id} className="relative h-[3px]"><Rail model={model} kind="edge" /></div>)}
        </div>
      )}
    </div>
  );
}
