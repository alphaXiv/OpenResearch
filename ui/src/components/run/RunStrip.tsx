import { RefreshCw } from "lucide-react";
import { timeAgo } from "../../api";
import { autoDir, fmtNumber, ltr } from "../../i18n";
import { m } from "../../paraglide/messages.js";
import type { LogSeek } from "../../logWindow";
import { namedStages, type RunModel } from "../../runMonitor/model";
import type { RunMonitor } from "../../runMonitor/types";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../../tabPreview";
import { RunContactNotice } from "../RunContactNotice";
import { Button, Tooltip } from "../ui";
import { cn } from "../ui/cn";
import { Block, drawsAt } from "./blocks/Block";
import { Legend } from "./blocks/Legend";
import { Phases } from "./blocks/Phases";
import { seriesOf } from "./blocks/series";
import { Captions } from "./Captions";
import { Controls } from "./Controls";
import { Delta } from "./Delta";
import { Issue } from "./Issue";
import { Lead } from "./Lead";
import { Mark } from "./Mark";
import { Rail } from "./Rail";
import { runFrame, type RunLabel } from "./RunLine";
import { Sentence } from "./Sentence";

type OpenTarget = (id: string, intent: TabOpenIntent) => void;
type OpenRun = (runId: string, intent: TabOpenIntent, seek?: LogSeek) => void;

/** The run's state as the strip reads it: absent against an older server, which can't serve it. A failed refresh keeps what it last had. */
export type StripMonitor = { status: "loading" } | { status: "error"; retry: () => void; data: RunMonitor | null } | { status: "ready"; data: RunMonitor } | null;

/** The headline and the first block of the view that applies now. */
function Hero({ model, monitor }: { model: RunModel; monitor: RunMonitor }) {
  const phase = monitor.progress ? monitor.phases[monitor.progress.phase]?.name : undefined;
  const block = monitor.view.blocks.find((candidate) => (!candidate.phase || candidate.phase === phase) && drawsAt(candidate, "strip"));
  const chart = block && "chart" in block ? block : null;
  const series = chart ? seriesOf(monitor, chart) : [];
  const headline = model.headline;
  if (!block && !headline?.known) return null;
  return (
    <div className="mt-3 flex h-16 items-stretch gap-5">
      {headline?.known && (
        <div className="shrink-0">
          <div className="text-xs text-ink-subtext">{headline.label}</div>
          <div className="text-xl font-semibold leading-7 tabular-nums">{headline.value}</div>
          <div className="h-4 text-xs">{headline.delta && <><Delta delta={headline.delta} said />{!headline.delta.flat && <span className="text-ink-subtext"> {m.run_than_parent()}</span>}</>}</div>
        </div>
      )}
      <div className="min-w-0 flex-1 overflow-hidden">
        {block && <Block monitor={monitor} block={block} size="strip" projectId={model.run.projectId} tone={model.tone} live={model.live} />}
      </div>
      {series.length > 1 && <div className="hidden w-24 shrink-0 flex-col justify-center @2xl:flex"><Legend series={series} /></div>}
    </div>
  );
}

/** The expanded run. It grows upward from the line: the underline stays where it was. A `label` puts it among several. */
export function RunStrip({ model, monitor, runNo, runCount, agent, label, onToggle, onOpenExperiment, onOpenRun }: {
  model: RunModel;
  monitor: StripMonitor;
  runNo?: number;
  runCount?: number;
  agent: string;
  label?: RunLabel;
  onToggle: () => void;
  onOpenExperiment?: OpenTarget;
  onOpenRun?: OpenRun;
}) {
  const data = monitor && monitor.status !== "loading" ? monitor.data : null;
  const flag = model.live ? model.flag?.record ?? null : null;
  const age = model.live && model.sentence.at !== null ? model.sentence.at : null;
  const failed = monitor?.status === "error";
  const { text, title } = label ?? { text: model.name, title: model.name };
  const name = autoDir(text);
  const titled = title === text ? undefined : autoDir(title);

  return (
    <div role="group" aria-label={title} className={label ? cn("relative", model.needsChoice && "bg-accent-amber-subtle") : runFrame(model)}>
      <div className="flex min-w-0 items-center gap-2 pt-1.5 ps-3 pe-1.5">
        <Mark tone={model.tone} live={model.live} label={model.status} />
        {onOpenExperiment ? (
          <Tooltip interactive content={m.a11y_open_item({ name: autoDir(title) })} className="min-w-0">
            <button className="tool-target experiment-run-link block max-w-full truncate font-medium" aria-label={titled} {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => onOpenExperiment(model.run.experimentId, intent))}>{name}</button>
          </Tooltip>
        ) : <Tooltip interactive content={autoDir(title)} className="min-w-0"><span className="block truncate font-medium">{name}</span></Tooltip>}
        {runCount !== undefined && runNo !== undefined && runCount > 1 && <span className="shrink-0 text-ink-subtext">{m.run_number({ number: ltr(fmtNumber(runNo)) })}</span>}
        {model.where && <span className="hidden min-w-0 truncate text-ink-subtext @lg:block">{model.where}</span>}
        <span className="flex-1" />
        <Controls model={model} expanded onToggle={onToggle} />
      </div>
      <div className="px-3 pb-4">
        {model.notice ? <RunContactNotice run={model.run} className="mt-0.5" /> : (
          <p className="m-0 mt-0.5 leading-snug">
            <Lead model={model} stop />
            <Sentence text={model.flagDetail ?? model.sentence.text} tone={model.tone} />
            {age !== null && <span className="whitespace-nowrap text-xs text-ink-subtext"> {timeAgo(age)}</span>}
          </p>
        )}
        {model.stale && !failed && <p className="m-0 mt-1.5 text-xs text-ink-subtext">{model.stale.why} · {model.stale.age}</p>}
        {model.note && <p className={cn("m-0 mt-1.5", model.tone === "revising" ? "text-sm" : "text-xs text-ink-subtext")}><span className={model.tone === "revising" ? "tool-running-shimmer" : undefined}>{model.note}</span></p>}
        {flag && <Issue model={model} flag={flag} agent={agent} onOpenRun={onOpenRun} />}
        {!model.pre && !flag && monitor?.status === "loading" && <div className="mt-3 h-16" />}
        {failed && (
          <div className={cn("flex items-center gap-2 text-xs text-ink-subtext", model.stale ? "mt-1.5" : "mt-3", !data && !model.pre && "h-16")}>
            {model.stale ? `${model.stale.why} · ${model.stale.age}` : m.run_monitor_load_failed()}
            <Button size="small" variant="ghost" onClick={monitor.retry}><RefreshCw size={13} />{m.app_retry()}</Button>
          </div>
        )}
        {!model.pre && !flag && data && <Hero model={model} monitor={data} />}
        {!model.pre && !flag && data && namedStages(data) && <div className="mt-3"><Phases model={model} monitor={data} /></div>}
        <div className="mt-3"><Captions model={model} /></div>
      </div>
      <Rail model={model} kind="thick" hover />
    </div>
  );
}
