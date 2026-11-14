import { ChevronDown, ChevronUp } from "lucide-react";
import { Fragment } from "react";
import { autoDir } from "../../i18n";
import { m } from "../../paraglide/messages.js";
import { TONE_INK, type RunModel } from "../../runMonitor/model";
import type { SummaryPart } from "../../runMonitor/summary";
import { IconButton, Tooltip } from "../ui";
import { cn } from "../ui/cn";
import { Mark } from "./Mark";
import { Rail } from "./Rail";
import { Sentence } from "./Sentence";

/** One line for several runs: their marks, what they share, who needs attention or leads, and every bar stacked under it. */
export function RunSummary({ models, context, parts, firstDone, open, onToggle }: {
  models: RunModel[];
  context: string | null;
  parts: SummaryPart[];
  firstDone: string | null;
  open: boolean;
  onToggle: () => void;
}) {
  const toggle = open ? m.run_show_less() : m.run_show_each();
  return (
    <div className="relative">
      <div className="flex min-h-9 min-w-0 items-center gap-x-2 py-1 ps-3 pe-1.5 @max-[26rem]:flex-wrap">
        <span className="flex shrink-0 items-center gap-1">
          {models.map((model) => <Mark key={model.run.id} tone={model.tone} live={model.live} label={model.status} />)}
        </span>
        {context && <span className="min-w-0 max-w-[40%] shrink truncate text-ink-subtext @max-[26rem]:max-w-none @max-[26rem]:flex-1">{autoDir(context)}</span>}
        <button
          className="min-w-0 flex-1 truncate rounded-xs text-start focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:outline-offset-2 @max-[26rem]:order-last @max-[26rem]:basis-full @max-[26rem]:pb-2"
          aria-expanded={open}
          onClick={onToggle}
        >
          {parts.map((part, index) => (
            <Fragment key={index}>
              {index > 0 && " · "}
              {part.who && <span className={cn("font-medium", TONE_INK[part.tone])}>{part.who} </span>}
              <Sentence text={part.text} tone={part.tone} />
            </Fragment>
          ))}
        </button>
        {firstDone && <span className="shrink-0 whitespace-nowrap tabular-nums text-ink-subtext @max-[30rem]:hidden">{firstDone}</span>}
        <Tooltip interactive content={toggle}>
          <IconButton size="small" aria-label={toggle} aria-expanded={open} data-run-toggle="*" onClick={onToggle}>
            {open ? <ChevronDown size={14} /> : <ChevronUp size={14} />}
          </IconButton>
        </Tooltip>
      </div>
      {!open && (
        <div className="flex flex-col gap-px">
          {models.map((model) => <div key={model.run.id} className="relative h-[3px]"><Rail model={{ ...model, ticks: [] }} kind="edge" /></div>)}
        </div>
      )}
    </div>
  );
}
