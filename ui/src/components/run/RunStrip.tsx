import type { Run } from "../../api";
import { Menu } from "@base-ui/react/menu";
import { ChevronDown, RefreshCw } from "lucide-react";
import { useContext, useMemo } from "react";
import { autoDir } from "../../i18n";
import { m } from "../../paraglide/messages.js";
import type { LogSeek } from "../../logWindow";
import type { RunModel } from "../../runMonitor/model";
import type { ChartBlock, RunMonitor } from "../../runMonitor/types";
import type { TabOpenIntent } from "../../tabPreview";
import { RunContactDetails, useRunContact, type ContactAction } from "../RunContactNotice";
import { Button, MenuItem } from "../ui";
import { WorkspaceDomContext } from "../../WorkspaceDom";
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

/** The chart a live run is drawn by: the view's first in the current stage with a line of its own to draw. */
export function liveChart(model: RunModel, data: RunMonitor | null): ChartBlock | undefined {
  if (!model.live || !data) return undefined;
  const phase = data.progress ? data.phases[data.progress.phase]?.name : undefined;
  return data.view.blocks.find((block): block is ChartBlock => "chart" in block
    && (!block.phase || block.phase === phase)
    && seriesOf(data, block).some((line) => !line.ghost && line.points.filter(([, y]) => y !== null).length >= 2));
}

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

/** A contact notice's first action as a compact button, the rest in its menu. */
function ContactActions({ actions }: { actions: ContactAction[] }) {
  const { portalContainer } = useContext(WorkspaceDomContext);
  const [main, ...more] = actions;
  if (!main) return null;

  const Icon = main.icon;
  return (
    <div className="flex">
      <Button size="compact" variant={main.tone === "danger" ? "danger" : "default"} className={cn(more.length > 0 && "rounded-e-none")}
        disabled={main.disabled} aria-haspopup={main.dialog ? "dialog" : undefined} onClick={main.run}>
        {Icon && <Icon size={13} />}{main.label}
      </Button>
      {more.length > 0 && <Menu.Root modal={false}>
        <Menu.Trigger render={<Button size="compact" className="rounded-s-none border-s-0 px-1" aria-label={m.run_more_actions()} />}>
          <ChevronDown size={13} aria-hidden="true" />
        </Menu.Trigger>
        <Menu.Portal container={portalContainer}>
          <Menu.Positioner className="z-70" positionMethod="fixed" side="bottom" align="end" sideOffset={4} collisionPadding={8}>
            <Menu.Popup className="min-w-36 rounded-md border border-border bg-background p-1 shadow-menu outline-none">
              {more.map(({ key, label, icon: ItemIcon, tone, disabled, run }) => (
                <Menu.Item key={key} nativeButton disabled={disabled} onClick={run}
                  render={<MenuItem size="compact" danger={tone === "danger"} className="justify-start data-highlighted:bg-surface" />}>
                  {ItemIcon && <ItemIcon size={13} aria-hidden="true" />}{label}
                </Menu.Item>
              ))}
            </Menu.Popup>
          </Menu.Positioner>
        </Menu.Portal>
      </Menu.Root>}
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
  const chart = useMemo(() => liveChart(model, data), [data, model.live]);
  const flag = model.flag?.record;
  const contact = useRunContact(model.run);
  const shownContact = model.notice ? contact : null;
  // The header's sentence and buttons already say a host is down or needs signing in to.
  const cause = model.notice?.kind !== "unreachable" && model.notice?.kind !== "needs_auth";
  const notice = shownContact && Boolean(shownContact.rentalBilling || cause && shownContact.detail || shownContact.next);
  const hero = !flag && (chart || model.headline?.known);
  const content = Boolean(hero || flag || model.note || notice || model.stale && !model.live || monitor?.status === "error");
  const mayHaveContent = monitor?.status === "loading"
    && (model.flag !== null || model.live && !model.pre && model.run.monitor?.setup == null);
  const expanded = open && (content || mayHaveContent);
  const stale = model.stale && [model.stale.why, model.stale.age].filter(Boolean).join(" · ");

  return (
    <RunLine model={model} agent={agent} label={label} actions={shownContact?.actions.length ? <ContactActions actions={shownContact.actions} /> : null} error={shownContact?.error} onToggle={onToggle} onOpenRunPage={onOpenRunPage} expanded={expanded} expandable={content || mayHaveContent}>
      {expanded && content && <div className="px-3 pb-3">
        {notice && <div role="status" className="mt-1.5 min-w-0 text-sm"><RunContactDetails contact={shownContact} cause={cause} /></div>}
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
