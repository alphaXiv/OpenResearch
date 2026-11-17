import type { Run } from "../../api";
import { useId, type ReactNode } from "react";
import { autoDir } from "../../i18n";
import { m } from "../../paraglide/messages.js";
import type { RunModel } from "../../runMonitor/model";
import type { TabOpenIntent } from "../../tabPreview";
import { useWorkspace } from "../../workspace";
import { Button, Tooltip } from "../ui";
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

/** The run's one number, unless the sentence already says it; then only how it compares. */
function Headline({ model }: { model: RunModel }) {
  const headline = model.headline;
  if (!headline?.known || (model.tone !== "running" && model.tone !== "done")) return null;
  const { delta } = headline;
  if (headline.inSentence) {
    if (!delta || delta.flat || headline.vsInSentence) return null;
    return <span className="shrink-0 whitespace-nowrap text-ink-subtext @max-[40rem]:hidden"><Delta delta={delta} said /> {m.run_than_parent()}</span>;
  }
  return (
    <span className="shrink-0 whitespace-nowrap text-ink-subtext @max-[40rem]:hidden">
      {headline.label} <span className="font-medium tabular-nums text-text">{headline.value}</span>
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
export function RunLine({ model, agent, label, onToggle, onOpenRunPage, expanded = false, expandable = true, children }: { expandable?: boolean; expanded?: boolean; children?: ReactNode; model: RunModel; agent: string; label?: RunLabel; onToggle: () => void; onOpenRunPage?: (run: Run, intent: TabOpenIntent) => void }) {
  const { requestHostConnect } = useWorkspace().events;
  const host = model.connectHost;
  const working = model.tone === "revising" ? m.run_agent_editing({ agent: autoDir(agent) }) : model.looking ? m.run_agent_looking({ agent: autoDir(agent) }) : null;
  const whyId = useId();
  const { stale } = model;
  // The time slot explains itself: why the view stopped, or what the time means.
  const time = expanded && stale ? null : stale ? { text: stale.age, why: stale.why, tip: `${stale.why} · ${stale.age}` }
    : model.eta ? { text: model.eta, why: model.etaDetail, tip: `${model.eta} · ${model.etaDetail}` }
    : null;
  const { text, title } = label ?? { text: model.name, title: model.name };
  return (
    <div role="group" aria-label={title} className={label ? cn("relative", model.needsChoice && "bg-accent-amber-subtle") : runFrame(model)}>
      <div className="flex min-h-9 min-w-0 items-center gap-x-2 py-1 ps-3 pe-1.5">
        <Mark tone={model.tone} live={model.live} label={model.status} />
        <RunStatus model={model} label={label ? text : undefined} descriptionId={time?.why ? whyId : undefined} onOpenRunPage={onOpenRunPage} />
        {working && <span className="tool-running-shimmer shrink-0 whitespace-nowrap @max-[40rem]:hidden">{working}</span>}
        {!expanded && <Headline model={model} />}
        {host && <Button size="small" onClick={() => requestHostConnect(host)}>{m.settings_connect()}</Button>}
        {time && (time.why ? (
          <Tooltip content={time.tip} className="shrink-0 rounded-xs">
            <span className="whitespace-nowrap tabular-nums text-ink-subtext">{time.text}</span>
            <span id={whyId} hidden>{time.why}</span>
          </Tooltip>
        ) : <span className="shrink-0 whitespace-nowrap tabular-nums text-ink-subtext">{time.text}</span>)}
        <Controls model={model} expandable={expandable} expanded={expanded} onToggle={onToggle} />
      </div>
      {children}
      <Rail model={model} kind="edge" />
    </div>
  );
}
