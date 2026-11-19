import { useWorkspace } from "../workspace";
import { useQuery } from "@tanstack/react-query";
import { FlaskConical, Folder, HelpCircle, RefreshCw, TriangleAlert } from "lucide-react";
import { useCallback, useEffect, useId, useMemo, useState } from "react";
import { createPortal } from "react-dom";

import { m } from "../paraglide/messages.js";
import { autoDir, fmtNumber, ltr } from "../i18n";
import { fmtDuration, timeAgo } from "../api";
import type { ChatSession, SessionExperiment } from "../api";
import { hostForRun } from "../engineHosts";
import { runActive, runDisplay, type RunDisplayStatus } from "../runDisplay";
import { runModel, type RunModel } from "../runMonitor/model";
import { shortLabels, summarize } from "../runMonitor/summary";
import type { ShownRun } from "../runMonitor/types";
import { Block } from "./run/blocks/Block";
import { Lead } from "./run/Lead";
import { Mark } from "./run/Mark";
import { Rail } from "./run/Rail";
import { Delta } from "./run/Delta";
import { seriesOf } from "./run/blocks/series";
import { liveChart, type StripMonitor } from "./run/RunStrip";
import { SummarySentence } from "./run/RunSummary";
import { Sentence } from "./run/Sentence";
import { useRunMonitors } from "./run/useRunMonitor";
import { harnessLabel } from "./ModelPicker";
import { HarnessLogo } from "./HarnessLogo";
import { statusLabel, statusStyle } from "./StatusBadge";
import { useMeasure, usePopoverPosition } from "./tourGeometry";
import { Badge, Button } from "./ui";
import { cn } from "./ui/cn";

const CARD_WIDTH = 360;
const GAP = 8;
export const CARD_FADE_MS = 120;

const ICON_TONES: Partial<Record<RunDisplayStatus, string>> = {
  submitting: "text-mark-green",
  starting: "text-mark-green",
  running: "text-mark-green",
  flagged: "text-mark-amber",
  flagged_ended: "text-mark-amber",
  cancelling: "text-mark-amber",
  cancellation_pending: "text-mark-amber",
  failed: "text-accent-red",
  auto_stopped: "text-accent-orange",
  revising: "text-accent-purple",
};

/**
 * A run the chat's composer shows, as its strip draws it: where it stands, its number, its live chart and its bar.
 * Among several, under a summary that says when the first is done, it keeps to its count, its number and its bar.
 */
function RunPeek({ model, monitor, alone }: { model: RunModel; monitor: StripMonitor; alone: boolean }) {
  const data = monitor && monitor.status !== "loading" ? monitor.data : null;
  const chart = useMemo(() => alone ? liveChart(model, data) : undefined, [alone, data, model.live]);
  const drawn = useMemo(() => data && chart ? seriesOf(data, chart).filter((line) => !line.ghost).map((line) => autoDir(line.label)).join(" · ") : "", [data, chart]);
  const { stage } = model.sentence;
  // The stage's words leave the headline out, so it gets a line of its own unless the run's sentence already says it.
  const headline = model.headline?.known && (stage || !model.headline.inSentence) ? model.headline : null;
  const stale = model.stale && [model.stale.why, model.stale.age].filter(Boolean).join(" · ");
  const failed = monitor?.status === "error" ? monitor : null;
  // What the strip says when the run's view is old or could not be read, so the card never passes it off as current.
  const health = (stale || failed) && (
    <div className="mt-0.5 flex min-w-0 items-center gap-2 text-ink-subtext">
      <span className="min-w-0 truncate">{stale || m.run_monitor_load_failed()}</span>
      {failed && <Button size="small" variant="ghost" className="-my-1" onClick={failed.retry}><RefreshCw size={13} />{m.app_retry()}</Button>}
    </div>
  );
  const number = headline && <>
    {autoDir(headline.label)} <span className="font-medium tabular-nums text-text">{headline.value}</span>
    {headline.delta && <> <Delta delta={headline.delta} said={alone} />{alone && !headline.delta.flat && <> {m.run_than_parent()}</>}</>}
  </>;

  if (!alone) {
    return (
      <div className="-mt-0.5 ps-[30px] pe-1.5 pb-2 text-xs">
        <div className="truncate">
          <Lead model={model} />
          <Sentence text={stage?.count || model.sentence.text} tone={model.tone} />
          {number && <span className="text-ink-subtext"> · {number}</span>}
        </div>
        {health}
        {model.live && <div className="relative mt-1.5 h-[3px] overflow-hidden rounded-full"><Rail model={model} kind="edge" /></div>}
      </div>
    );
  }

  return (
    <div className="-mt-0.5 ps-[30px] pe-1.5 pb-2 text-xs">
      <div className="flex min-w-0 items-center gap-2">
        <span className="flex min-w-0 flex-1 items-center whitespace-nowrap">
          <Lead model={model} />
          {stage ? <>
            <span className="min-w-0 shrink truncate"><Sentence text={stage.name} tone={model.tone} /></span>
            <span className="ms-1 shrink-0"><Sentence text={stage.count} tone={model.tone} /></span>
          </> : <span className="min-w-0 truncate"><Sentence text={model.sentence.text} tone={model.tone} /></span>}
        </span>
        {model.eta && <span className="shrink-0 tabular-nums text-ink-subtext">{model.eta}</span>}
      </div>
      {number && <div className="mt-0.5 truncate text-ink-subtext">{number}</div>}
      {health}
      {data && chart && <div className="mt-2">
        {drawn !== autoDir(headline?.label ?? "") && <div className="truncate text-ink-subtext">{drawn}</div>}
        <div className="h-14"><Block monitor={data} block={chart} size="strip" tone={model.tone} live={model.live} /></div>
      </div>}
      {model.live && <div className="relative mt-1.5 h-[3px] overflow-hidden rounded-full"><Rail model={model} kind="edge" /></div>}
    </div>
  );
}

function ExperimentRow({ row, peek, sessionProjectId, tabbable, onFocus, onOpen }: {
  row: SessionExperiment;
  peek: React.ReactNode;
  sessionProjectId: string;
  tabbable: boolean;
  onFocus: () => void;
  onOpen: () => void;
}) {
  const { experiment, currentRun, projectName } = row;
  const display = currentRun ? runDisplay(currentRun) : null;
  const status = display?.status ?? null;
  const tone = status ? ICON_TONES[status] ?? "text-subtext" : "text-muted";
  const live = status !== null && statusStyle(status).live;
  const elapsed = !peek && currentRun && (live ? fmtDuration(Date.now() - currentRun.createdAt) : timeAgo(currentRun.endedAt ?? currentRun.createdAt));
  const elsewhere = experiment.projectId !== sessionProjectId;

  return (
    <li>
      <button
        type="button"
        data-experiment-row=""
        tabIndex={tabbable ? 0 : -1}
        className="flex w-full items-start gap-2 rounded-md px-1.5 py-1.5 text-start hover:bg-surface focus-visible:-outline-offset-2 focus-visible:outline-2 focus-visible:outline-focus-border"
        onFocus={onFocus}
        onClick={onOpen}
      >
        {status === "flagged" || status === "flagged_ended" ? <TriangleAlert size={14} className={`mt-[3px] shrink-0 ${tone}`} aria-hidden="true" /> : <FlaskConical
          size={14}
          className={`mt-[3px] shrink-0 ${tone} ${live ? "motion-safe:animate-[or-experiment-pulse_3s_ease-in-out_infinite]" : ""}`}
          aria-hidden="true"
        />}
        <span className="min-w-0 flex-1">
          <span className="block break-words">{experiment.title?.trim() || experiment.slug}</span>
          {(!peek || experiment.archived || elsewhere) && <span className="mt-0.5 flex flex-wrap items-center gap-x-1.5 text-xs text-subtext">
            {!peek && (status ? <span className="text-text">{statusLabel(status, display?.stopping)}</span> : <span>{m.experiments_not_run_yet()}</span>)}
            {elapsed && <span className="tabular-nums">{elapsed}</span>}
            {experiment.archived && <span>{m.tree_archived()}</span>}
            {elsewhere && (
              <span className="ms-auto flex min-w-0 items-center gap-1">
                <Folder size={12} className="shrink-0" aria-hidden="true" />
                <span className="truncate">{projectName}</span>
              </span>
            )}
          </span>}
        </span>
      </button>
      {peek}
    </li>
  );
}

export function SessionHoverCard({ session, need, runs, motion, modelName, harnessLogo, anchor, cardId, cardRef, container, onOpenExperiment, onReturnToRow, onContinue }: {
  session: ChatSession;
  need: string | null;
  /** The chat's runs its composer shows, each drawn under its experiment. */
  runs: readonly ShownRun[];
  motion: "enter" | "warm" | "leave";
  modelName: string | null;
  harnessLogo?: string;
  anchor: DOMRect;
  cardId: string;
  cardRef: React.RefObject<HTMLDivElement | null>;
  container: Element;
  onOpenExperiment: (projectId: string, experimentId: string) => void;
  onReturnToRow: () => void;
  onContinue: () => void;
}) {
  const { getSessionExperimentsQuery, useEngineHosts } = useWorkspace().queries;
  const hosts = useEngineHosts();

  const measure = useMeasure();
  const headingId = useId();
  const { data, isPending, error } = useQuery(getSessionExperimentsQuery(session.id));
  const [focusedId, setFocusedId] = useState<string>();
  const [now, setNow] = useState(Date.now);
  // An experiment shows one run: one waiting on a flag, else one still going, else its newest.
  const latest = useMemo(() => {
    const rank = (item: ShownRun) => [item.run.monitor?.flag ? 1 : 0, runActive(item.run.status) ? 1 : 0, item.run.createdAt];
    const outranks = (a: ShownRun, b: ShownRun) => {
      const [x, y] = [rank(a), rank(b)];
      const at = x.findIndex((value, index) => value !== y[index]);
      return at >= 0 && x[at] > y[at];
    };
    return runs.filter((item) => !runs.some((other) => other.run.experimentId === item.run.experimentId && outranks(other, item)));
  }, [runs]);
  const latestRuns = useMemo(() => latest.map((item) => item.run), [latest]);
  const monitors = useRunMonitors(latestRuns, () => true);
  const live = latest.some((item) => runActive(item.run.status));
  const models = latest.map((item, index) => {
    const monitor = monitors[index];
    const host = hostForRun(hosts, item.run);
    return runModel({
      item,
      monitor: monitor && monitor.status !== "loading" ? monitor.data : null,
      monitorFailed: monitor?.status === "error",
      shown: true,
      agent: harnessLabel(session.harness, session.harnessName),
      now,
      nextRetryMs: host?.status === "backoff" ? host.nextRetryMs : null,
    });
  });
  // Several runs read as the composer's summary does: one sentence over them all, and no chart for each.
  const titles = latest.map((item) => item.experimentName ?? item.experimentSlug ?? item.run.experimentId);
  const summary = latest.length > 1 ? summarize(models, shortLabels(titles), latest[0].watch, now) : null;
  useEffect(() => {
    if (!live) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [live]);
  const setCard = useCallback((element: HTMLDivElement | null) => {
    cardRef.current = element;
    const dispose = measure.ref(element);
    return () => { cardRef.current = null; dispose?.(); };
  }, [cardRef, measure.ref]);
  const width = Math.min(CARD_WIDTH, window.innerWidth - 16);
  const side = anchor.right + GAP + width <= window.innerWidth ? "right"
    : anchor.left - GAP - width >= 0 ? "left"
      : anchor.top > window.innerHeight / 2 ? "above" : "below";
  const { x, y: fallbackY } = usePopoverPosition(
    { x: anchor.x, y: anchor.y, width: anchor.width, height: anchor.height, anchor: side, distance: GAP },
    measure,
  );
  const cardHeight = cardRef.current?.getBoundingClientRect().height ?? measure.offsetHeight;
  let y = fallbackY;
  if (side === "left" || side === "right") {
    const bottomAligned = anchor.bottom - cardHeight;
    if (anchor.top >= 0 && anchor.top + cardHeight <= window.innerHeight) {
      y = anchor.top;
    } else if (bottomAligned >= 0 && anchor.bottom <= window.innerHeight) {
      y = bottomAligned;
    } else {
      y = Math.max(0, Math.min(anchor.top, window.innerHeight - cardHeight));
    }
  }
  const title = session.title?.trim() || m.chat_untitled();
  /** Where the card slides from and back to: a few pixels toward its row. */
  const offset = { right: "-4px 0", left: "4px 0", above: "0 4px", below: "0 -4px" }[side];
  const experiments = isPending || error || (data && data.length > 0);
  const tabbableId = data?.some(({ experiment }) => experiment.id === focusedId) ? focusedId : data?.[0]?.experiment.id;

  return createPortal(
    <div
      ref={setCard}
      id={cardId}
      data-session-hover-card={session.id}
      role="group"
      aria-label={title}
      tabIndex={-1}
      inert={motion === "leave"}
      className={cn(
        "fixed z-60 flex max-h-[min(28rem,calc(100vh-16px))] flex-col rounded-lg border border-border bg-background p-4 text-sm text-text shadow-menu focus-visible:outline-2 focus-visible:outline-focus-border",
        "transition-[opacity,translate] ease-out",
        motion === "leave" ? "pointer-events-none opacity-0 motion-safe:[translate:var(--card-from)]" : "duration-150",
        motion === "enter" && "starting:opacity-0 motion-safe:starting:[translate:var(--card-from)]",
      )}
      style={{
        width,
        left: x,
        top: y,
        visibility: measure.offsetHeight === 0 ? "hidden" : undefined,
        transitionDuration: motion === "leave" ? `${CARD_FADE_MS}ms` : undefined,
        "--card-from": offset,
      } as React.CSSProperties}
      onClick={(event) => event.stopPropagation()}
      onKeyDown={(event) => {
        const rows = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>("[data-experiment-row]"));
        const index = rows.findIndex((row) => row === event.target);
        const onCard = event.target === event.currentTarget;
        const next = event.key === "ArrowDown" ? Math.min(index + 1, rows.length - 1)
          : event.key === "ArrowUp" ? Math.max(index - 1, 0)
            : event.key === "Home" ? 0
              : event.key === "End" ? rows.length - 1
                : null;
        if (next !== null && rows.length > 0) {
          event.preventDefault();
          rows[next].focus();
        } else if (event.key === "Tab" && event.shiftKey) {
          event.preventDefault();
          if (onCard) onReturnToRow();
          else event.currentTarget.focus();
        } else if (event.key === "Tab" && (!onCard || rows.length === 0)) {
          event.preventDefault();
          onContinue();
        }
      }}
    >
      <div className="shrink-0 font-semibold break-words">{title}</div>
      {need && (
        <div className="mt-2 flex shrink-0 gap-1.5 text-subtext">
          <HelpCircle size={14} className="mt-[3px] shrink-0 text-accent-purple" aria-hidden="true" />
          <span className="min-w-0 wrap-anywhere" dir="auto">{need}</span>
        </div>
      )}
      {experiments && <div className="mt-3 flex min-h-0 flex-col border-t border-border pt-2">
        {(!data || data.length > 0) && (
          <div className="mb-1 flex shrink-0 items-center gap-1.5 text-xs font-medium text-subtext">
            <span id={headingId}>{m.app_experiments()}</span>
            {!!data && <Badge size="small" className="tabular-nums">{fmtNumber(data.length)}</Badge>}
          </div>
        )}
        {summary && (
          <div className="mb-1.5 flex shrink-0 items-start gap-2 text-xs">
            <span className="flex h-4 shrink-0 items-center gap-1">
              {models.map((model) => <Mark key={model.run.id} tone={model.tone} live={model.live} label={model.status} />)}
            </span>
            <span className="min-w-0">
              <SummarySentence parts={summary.parts} />
              {summary.firstDone && <span className="text-ink-subtext"> · <Sentence text={autoDir(summary.firstDone)} tone="running" /></span>}
            </span>
          </div>
        )}
        <div className="-mx-1.5 min-h-0 overflow-y-auto overscroll-contain">
          {isPending ? <div className="px-1.5 text-subtext">{m.common_loading()}</div>
            : error ? <div className="px-1.5 text-subtext">{m.common_failed_to_load({ error: ltr(error.message) })}</div>
              : data && data.length > 0 ? (
                  <ul role="list" aria-labelledby={headingId} className="m-0 list-none p-0">
                    {data.map((row) => {
                      const index = latest.findIndex((item) => item.run.experimentId === row.experiment.id);
                      return <ExperimentRow
                        key={row.experiment.id}
                        row={row}
                        peek={index >= 0 && <RunPeek model={models[index]} monitor={monitors[index]} alone={!summary} />}
                        sessionProjectId={session.projectId}
                        tabbable={row.experiment.id === tabbableId}
                        onFocus={() => setFocusedId(row.experiment.id)}
                        onOpen={() => onOpenExperiment(row.experiment.projectId, row.experiment.id)}
                      />;
                    })}
                  </ul>
                ) : null}
        </div>
      </div>}
      <div className={cn("flex shrink-0 items-center gap-1.5 text-xs text-muted", experiments ? "mt-3" : "mt-1.5")}>
        <HarnessLogo harness={session.harness} logo={harnessLogo} size={12} />
        <span className="min-w-0 break-words">{harnessLabel(session.harness, session.harnessName)}{modelName ? ` · ${modelName}` : ""}</span>
      </div>
    </div>,
    container,
  );
}
