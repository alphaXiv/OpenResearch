import { useMutate } from "@clo/react-mutation";
import { ChevronDown, ChevronUp, X } from "lucide-react";
import { Fragment, useLayoutEffect, useRef, useState } from "react";
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

/** Several runs together, with every bar stacked below when folded. */
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

  const [content, { width }] = useBoxSize<HTMLDivElement>();
  const whole = useRef<HTMLSpanElement>(null);
  const fullLabel = context?.whole;
  const [fits, setFits] = useState(true);
  useLayoutEffect(() => {
    if (whole.current && width > 0) setFits(whole.current.offsetWidth <= width);
  }, [fullLabel, width]);
  return (
    <div className="relative">
      <div className="flex min-h-9 min-w-0 items-start gap-x-2 py-1 ps-3 pe-1.5">
        <span className="flex h-7 max-w-1/4 shrink-0 items-center gap-1 overflow-hidden">
          {models.map((model) => <Mark key={model.run.id} tone={model.tone} live={model.live} label={model.status} />)}
        </span>
        <div
          ref={content}
          className={cn(
            "grid min-w-0 flex-1 items-baseline gap-x-1 py-0.5",
            firstDone && "@min-[30rem]:grid-cols-[fit-content(65%)_minmax(0,1fr)]",
          )}
        >
          <div className="flex min-w-0 flex-wrap items-baseline gap-x-2">
            {context && (
              <span data-part="shared" className="max-w-full wrap-anywhere text-ink-subtext">{autoDir(fits ? context.whole : context.short)}</span>
            )}
            <button
              className="min-h-6 min-w-0 max-w-full grow rounded-xs text-start wrap-anywhere focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:outline-offset-2"
              aria-expanded={open}
              onClick={onToggle}
            >
              <span data-part="sentence" className="block">
                {parts.map((part, index) => (
                  <Fragment key={index}>
                    {index > 0 && " · "}
                    {part.who && <span className={cn("font-medium", TONE_INK[part.tone])}>{part.who} </span>}
                    <Sentence text={autoDir(part.text)} tone={part.tone} />
                  </Fragment>
                ))}
              </span>
            </button>
          </div>
          {firstDone && (
            <span data-part="estimate" className="min-w-0 wrap-anywhere text-ink-subtext">
              <span aria-hidden="true" className="@max-[30rem]:hidden">· </span>
              <Sentence text={autoDir(firstDone)} tone="running" />
            </span>
          )}
        </div>
        {fullLabel && <span ref={whole} aria-hidden="true" className="invisible absolute start-0 top-0 w-max whitespace-nowrap">{autoDir(fullLabel)}</span>}
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
