import type { Run } from "../../api";
import { RefreshCw } from "lucide-react";
import { useMemo } from "react";
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
import { HeadlineStat } from "./HeadlineStat";
import { Issue } from "./Issue";
import { RunLine, type RunLabel } from "./RunLine";
import { Sentence } from "./Sentence";

type OpenRun = (runId: string, intent: TabOpenIntent, seek?: LogSeek) => void;

export type StripMonitor = { status: "loading" } | { status: "error"; retry: () => void; data: RunMonitor | null } | { status: "ready"; data: RunMonitor } | null;

function Hero({ model, monitor, block }: { model: RunModel; monitor: RunMonitor | null; block?: ChartBlock }) {
  const series = useMemo(() => monitor && block ? seriesOf(monitor, block) : [], [monitor, block]);
  const drawn = series.filter((line) => !line.ghost).map((line) => line.label);
  const legend = series.length > 1;
  return (
    <div className="mt-3 flex h-16 items-stretch gap-5">
      {model.headline?.known && <HeadlineStat model={model} />}
      <div className="flex min-w-0 flex-1 flex-col overflow-hidden">
        {drawn.length > 0 && <div className={cn("truncate text-xs text-ink-subtext", legend && "@2xl:hidden")}>{drawn.map(autoDir).join(" · ")}</div>}
        <div className="min-h-0 flex-1">
          {monitor && block && <Block monitor={monitor} block={block} size="strip" tone={model.tone} live={model.live} />}
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
  agent: string | null;
  label?: RunLabel;
  onToggle: () => void;
  onOpenRun?: OpenRun;
  onOpenRunPage?: (run: Run, intent: TabOpenIntent) => void;
}) {
  const data = monitor && monitor.status !== "loading" ? monitor.data : null;
  const chart = useMemo(() => {
    if (!model.live || !data) return undefined;
    const phase = data.progress ? data.phases[data.progress.phase]?.name : undefined;
    return data.view.blocks.find((block): block is ChartBlock => "chart" in block
      && (!block.phase || block.phase === phase)
      && seriesOf(data, block).some((line) => !line.ghost && line.points.filter(([, y]) => y !== null).length >= 2));
  }, [data, model.live]);
  const flag = model.flag?.record;
  const notice = model.notice && model.notice.kind !== "cancelling";
  const hero = !flag && (chart || model.headline?.known);
  const content = Boolean(hero || flag || model.note || notice || model.stale && !model.live || monitor?.status === "error");
  const expanded = open && content;
  const stale = model.stale && [model.stale.why, model.stale.age].filter(Boolean).join(" · ");

  return (
    <RunLine model={model} agent={agent} label={label} onToggle={onToggle} onOpenRunPage={onOpenRunPage} expanded={expanded} expandable={content}>
      {expanded && <div className="px-3 pb-3">
        {notice && <RunContactNotice run={model.run} className="mt-1.5" />}
        {stale && monitor?.status !== "error" && <p className="m-0 mt-1.5 text-xs text-ink-subtext">{stale}</p>}
        {model.note && <p className={cn("m-0 text-xs text-ink-subtext", !notice && !model.stale ? "-mt-0.5" : "mt-1.5")}>{model.note}</p>}
        {flag && !model.sentence.watcher && model.flagDetail && <p className="m-0 mt-1.5 text-sm"><Sentence text={model.flagDetail} tone={model.tone} /></p>}
        {flag && <Issue model={model} flag={flag} agent={agent} onOpenRun={onOpenRun} />}
        {monitor?.status === "error" && <div className="mt-1.5 flex items-center gap-2 text-xs text-ink-subtext">
          {stale ?? m.run_monitor_load_failed()}
          <Button size="small" variant="ghost" onClick={monitor.retry}><RefreshCw size={13} />{m.app_retry()}</Button>
        </div>}
        {hero && <Hero model={model} monitor={data} block={chart} />}
      </div>}
    </RunLine>
  );
}
