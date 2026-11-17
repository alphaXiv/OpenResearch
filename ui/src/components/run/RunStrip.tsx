import type { Run } from "../../api";
import { RefreshCw } from "lucide-react";
import { autoDir } from "../../i18n";
import { m } from "../../paraglide/messages.js";
import type { LogSeek } from "../../logWindow";
import type { RunModel } from "../../runMonitor/model";
import type { ChartBlock, RunMonitor } from "../../runMonitor/types";
import type { TabOpenIntent } from "../../tabPreview";
import { RunContactNotice } from "../RunContactNotice";
import { Button } from "../ui";
import { cn } from "../ui/cn";
import { Block } from "./blocks/Block";
import { Legend } from "./blocks/Legend";
import { seriesOf } from "./blocks/series";
import { Delta } from "./Delta";
import { Issue } from "./Issue";
import { RunLine, type RunLabel } from "./RunLine";
import { Sentence } from "./Sentence";

type OpenRun = (runId: string, intent: TabOpenIntent, seek?: LogSeek) => void;

export type StripMonitor = { status: "loading" } | { status: "error"; retry: () => void; data: RunMonitor | null } | { status: "ready"; data: RunMonitor } | null;

function Hero({ model, monitor, block }: { model: RunModel; monitor: RunMonitor; block: ChartBlock }) {
  const series = seriesOf(monitor, block);
  const headline = model.headline;
  const drawn = series.filter((line) => !line.ghost).map((line) => line.label);
  const legend = series.length > 1;
  return (
    <div className="mt-3 flex h-16 items-stretch gap-5">
      {headline?.known && (
        <div className="shrink-0">
          <div className="text-xs text-ink-subtext">{headline.label}</div>
          <div className="text-xl font-semibold leading-7 tabular-nums">{headline.value}</div>
          <div className="h-4 text-xs">{headline.delta && <><Delta delta={headline.delta} said />{!headline.delta.flat && <span className="text-ink-subtext"> {m.run_than_parent()}</span>}</>}</div>
        </div>
      )}
      <div className="flex min-w-0 flex-1 flex-col overflow-hidden">
        {drawn.length > 0 && <div className={cn("truncate text-xs text-ink-subtext", legend && "@2xl:hidden")}>{drawn.map(autoDir).join(" · ")}</div>}
        <div className="min-h-0 flex-1">
          <Block monitor={monitor} block={block} size="strip" projectId={model.run.projectId} tone={model.tone} live={model.live} />
        </div>
      </div>
      {legend && <div className="hidden w-24 shrink-0 flex-col justify-center @2xl:flex"><Legend series={series} /></div>}
    </div>
  );
}

export function RunStrip({ model, monitor, agent, label, open, onToggle, onOpenRun, onOpenRunPage }: {
  model: RunModel;
  open: boolean;
  monitor: StripMonitor;
  agent: string;
  label?: RunLabel;
  onToggle: () => void;
  onOpenRun?: OpenRun;
  onOpenRunPage?: (run: Run, intent: TabOpenIntent) => void;
}) {
  const data = monitor && monitor.status !== "loading" ? monitor.data : null;
  const phase = data?.progress ? data.phases[data.progress.phase]?.name : undefined;
  const chart = open && model.live ? data?.view.blocks.find((block): block is ChartBlock => "chart" in block && (!block.phase || block.phase === phase) && seriesOf(data, block).some((line) => !line.ghost && line.points.filter(([, y]) => y !== null).length >= 2)) : undefined;
  const flag = model.flag?.record;
  const notice = model.notice && model.notice.kind !== "cancelling";
  const expanded = open && Boolean(chart || flag || model.note || notice || monitor?.status === "error");

  return (
    <RunLine model={model} agent={agent} label={label} onToggle={onToggle} onOpenRunPage={onOpenRunPage} expanded={expanded} expandable={!open || expanded || monitor?.status === "loading"}>
      {expanded && <div className="px-3 pb-3">
        {notice && <RunContactNotice run={model.run} className="mt-1.5" />}
        {model.stale && monitor?.status !== "error" && <p className="m-0 mt-1.5 text-xs text-ink-subtext">{model.stale.why} · {model.stale.age}</p>}
        {model.note && <p className="m-0 mt-1.5 text-xs text-ink-subtext">{model.note}</p>}
        {flag && model.flagDetail && model.flagDetail !== model.sentence.text && <p className="m-0 mt-1.5 text-sm"><Sentence text={model.flagDetail} tone={model.tone} /></p>}
        {flag && <Issue model={model} flag={flag} agent={agent} onOpenRun={onOpenRun} />}
        {monitor?.status === "error" && <div className="mt-1.5 flex items-center gap-2 text-xs text-ink-subtext">
          {model.stale ? `${model.stale.why} · ${model.stale.age}` : m.run_monitor_load_failed()}
          <Button size="small" variant="ghost" onClick={monitor.retry}><RefreshCw size={13} />{m.app_retry()}</Button>
        </div>}
        {chart && data && !flag && <Hero model={model} monitor={data} block={chart} />}
      </div>}
    </RunLine>
  );
}
