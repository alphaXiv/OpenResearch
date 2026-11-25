import type { Run } from "../../api";
import { useId, type ReactNode } from "react";
import { autoDir } from "../../i18n";
import { m } from "../../paraglide/messages.js";
import type { RunModel } from "../../runMonitor/model";
import type { TabOpenIntent } from "../../tabPreview";
import { Tooltip } from "../ui";
import { cn } from "../ui/cn";
import { Controls } from "./Controls";
import { Delta } from "./Delta";
import { Mark } from "./Mark";
import { Rail } from "./Rail";
import { RunStatus } from "./RunStatus";

/** The frame a lone line or strip sits in; amber while it waits for the user. */
export const runFrame = (model: RunModel) => cn(
  "@container relative mb-2.5 overflow-hidden rounded-lg border text-sm text-text",
  model.needsChoice ? "border-accent-amber/45 bg-accent-amber-subtle" : "border-border bg-surface",
);

/** The run's one number once it has one, unless the sentence already says it; then only how it compares. */
function Headline({ model }: { model: RunModel }) {
  const headline = model.headline;
  if (!headline?.known || (model.tone !== "starting" && model.tone !== "running" && model.tone !== "done")) return null;
  const { delta } = headline;
  if (headline.inSentence) {
    if (!delta || delta.flat || headline.vsInSentence) return null;
    return <span className="shrink-0 whitespace-nowrap text-ink-subtext @max-[40rem]:hidden"><Delta delta={delta} said /> {m.run_than_parent()}</span>;
  }
  return (
    <span className="shrink-0 whitespace-nowrap text-ink-subtext @max-[40rem]:hidden">
      {autoDir(headline.label)} <span className="font-medium tabular-nums text-text">{headline.value}</span>
      {delta && <> <Delta delta={delta} said />{!delta.flat && <> {m.run_than_parent()}</>}</>}
    </span>
  );
}

/** How a run among several is named: what sets it apart from the others, and its experiment's whole title. */
export interface RunLabel {
  text: string;
  title: string;
}

/** The collapsed run: one sentence, how it compares, the time left. The bar is its underline. A `label` puts it among several. */
export function RunLine({ model, agent, label, actions, error, onToggle, onOpenRunPage, expanded = false, expandable = true, children }: { actions?: ReactNode; error?: string | null; expandable?: boolean; expanded?: boolean; children?: ReactNode; model: RunModel; agent: string | null; label?: RunLabel; onToggle: () => void; onOpenRunPage?: (run: Run, intent: TabOpenIntent) => void }) {
  const state = expanded && model.flag?.record ? null : model.flag?.state;
  const editing = model.tone === "revising";
  const name = autoDir(agent ?? m.chat_the_agent());
  const working = editing ? m.run_agent_editing({ agent: name })
    : state === "agent_looking" ? m.run_agent_looking({ agent: name })
    : state === "waking" ? m.chat_starting()
    : state === "raised" || state === "asking" ? m.chat_queued() : null;
  const whyId = useId();
  const { stale } = model;
  // The time slot explains itself: why the view stopped, or what the time means.
  const time = expanded && stale ? null
    : stale?.age ? { text: stale.age, why: stale.why, tip: `${stale.why} · ${stale.age}` }
    : stale ? { text: stale.why, why: "", tip: "" }
    : model.eta ? { text: model.eta, why: model.etaDetail, tip: `${model.eta} · ${model.etaDetail}` }
    : null;
  const { text, title } = label ?? { text: model.name, title: model.name };
  // Under the sentence, the time starts where the sentence does: past the mark and the gap.
  const under = cn("@max-[30rem]:order-last", model.tone === "flag" ? "@max-[30rem]:ms-[22px]" : "@max-[30rem]:ms-[15px]");
  const besideMark = model.tone === "flag" ? "ms-[22px]" : "ms-[15px]";
  return (
    <div data-run-frame={label ? undefined : ""} role="group" aria-label={title} className={label ? cn("relative", model.needsChoice && "bg-accent-amber-subtle") : runFrame(model)}>
      <div className="flex min-h-9 min-w-0 items-center gap-x-2 py-1 ps-3 pe-1.5 @max-[30rem]:flex-wrap @max-[30rem]:items-start">
        <span className="flex h-7 shrink-0 items-center"><Mark tone={model.tone} live={model.live} label={model.status} /></span>
        <RunStatus model={model} label={label ? text : undefined} descriptionId={time?.why ? whyId : undefined} onOpenRunPage={onOpenRunPage} expanded={expanded} />
        {working && <span className={cn("shrink-0 whitespace-nowrap @max-[40rem]:hidden", editing || state === "agent_looking" ? "tool-running-shimmer" : "text-ink-subtext")}>{working}</span>}
        {!expanded && <Headline model={model} />}
        {(time || actions) && <span aria-hidden="true" className="hidden basis-full @max-[30rem]:order-last @max-[30rem]:block" />}
        {actions && <div className={cn("shrink-0", under)}>{actions}</div>}
        {time && (time.why ? (
          <Tooltip content={time.tip} className={cn("shrink-0 rounded-xs", under)}>
            <span className="whitespace-nowrap tabular-nums text-ink-subtext">{time.text}</span>
            <span id={whyId} hidden>{time.why}</span>
          </Tooltip>
        ) : <span className={cn("whitespace-nowrap tabular-nums text-ink-subtext", under, stale ? "max-w-3/5 shrink-0 truncate" : "shrink-0")}>{time.text}</span>)}
        <Controls model={model} expandable={expandable} expanded={expanded} onToggle={onToggle} />
      </div>
      {error && <p role="alert" className={cn("m-0 ps-3 pe-3 pb-2 text-xs text-accent-red wrap-anywhere", besideMark)}>{error}</p>}
      {children}
      <Rail model={model} kind="edge" />
    </div>
  );
}
