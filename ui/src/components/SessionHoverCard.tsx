import { useWorkspace } from "../workspace";
import { useQuery } from "@tanstack/react-query";
import { Folder } from "lucide-react";
import { useCallback, useEffect, useId, useState } from "react";
import { createPortal } from "react-dom";

import { m } from "../paraglide/messages.js";
import { autoDir, fmtNumber, ltr } from "../i18n";
import { timeAgo } from "../api";
import type { ChatSession, SessionExperiment } from "../api";
import { runModel, type RunModel } from "../runMonitor/model";
import { harnessLabel } from "./ModelPicker";
import { HarnessLogo } from "./HarnessLogo";
import { Lead } from "./run/Lead";
import { Mark } from "./run/Mark";
import { Headline } from "./run/RunLine";
import { Sentence } from "./run/Sentence";
import { useMeasure, usePopoverPosition } from "./tourGeometry";
import { Badge } from "./ui";
import { cn } from "./ui/cn";

const CARD_WIDTH = 400;
const GAP = 8;

/** Waiting on the user first, then live, then ended in the server's order of newest activity, then never run. */
const rank = (model: RunModel | null) => !model ? 3 : model.needsChoice ? 0 : model.live ? 1 : 2;

/** An experiment as its current run stands, in the run line's words: how far, how long, its number, and a flag's warning. */
function ExperimentRow({ row, model, sessionProjectId, tabbable, onFocus, onOpen }: {
  row: SessionExperiment;
  model: RunModel | null;
  sessionProjectId: string;
  tabbable: boolean;
  onFocus: () => void;
  onOpen: () => void;
}) {
  const { experiment, projectName } = row;
  const stage = model?.sentence.stage;
  const watcher = stage && model.sentence.watcher;
  const time = !model ? null
    : model.live ? model.stale?.age ?? model.eta
    : timeAgo(model.run.endedAt ?? model.run.updatedAt);
  const elsewhere = experiment.projectId !== sessionProjectId;

  return (
    <li>
      <button
        type="button"
        data-experiment-row=""
        tabIndex={tabbable ? 0 : -1}
        className={cn(
          "flex w-full items-start gap-2 rounded-md px-1.5 py-1.5 text-start focus-visible:-outline-offset-2 focus-visible:outline-2 focus-visible:outline-focus-border",
          model?.needsChoice ? "bg-accent-amber-subtle" : "hover:bg-surface",
        )}
        onFocus={onFocus}
        onClick={onOpen}
      >
        <span className="flex h-5 w-3.5 shrink-0 items-center justify-center">
          {model ? <Mark tone={model.tone} live={model.live} label={model.status} />
            : <span aria-hidden="true" className="inline-block size-[7px] rounded-full border border-subtext" />}
        </span>
        <span className="min-w-0 flex-1">
          <span className="flex items-start gap-3">
            <span className="min-w-0 flex-1 break-words">{autoDir(experiment.title?.trim() || experiment.slug)}</span>
            {time && <span className="shrink-0 whitespace-nowrap text-xs leading-5 tabular-nums text-ink-subtext">{time}</span>}
          </span>
          <span className="mt-0.5 block text-xs text-ink-subtext">
            {!model ? m.experiments_not_run_yet()
              : stage ? (
                <span className="flex min-w-0 whitespace-nowrap">
                  <Lead model={model} />
                  {stage.name && <span className="min-w-0 shrink truncate"><Sentence text={stage.name} tone={model.tone} /></span>}
                  {stage.count && <span className={cn("shrink-0", stage.name && "ms-1")}><Sentence text={stage.count} tone={model.tone} /></span>}
                </span>
              )
              : <span className="break-words"><Lead model={model} /><Sentence text={model.sentence.text} tone={model.tone} /></span>}
          </span>
          {model && <span className="mt-0.5 flex text-xs empty:hidden"><Headline model={model} /></span>}
          {watcher && <span className="mt-0.5 block break-words text-xs"><Sentence text={watcher} tone={model.tone} /></span>}
          {model?.needsChoice && <span className="mt-0.5 block text-xs font-medium text-ink-amber">{m.chat_hover_waiting_for_choice()}</span>}
          {(experiment.archived || elsewhere) && (
            <span className="mt-0.5 flex items-center gap-x-1.5 text-xs text-ink-subtext">
              {experiment.archived && <span>{m.tree_archived()}</span>}
              {elsewhere && (
                <span className="flex min-w-0 items-center gap-1">
                  <Folder size={12} className="shrink-0" aria-hidden="true" />
                  <span className="truncate">{autoDir(projectName)}</span>
                </span>
              )}
            </span>
          )}
        </span>
      </button>
    </li>
  );
}

export function SessionHoverCard({ session, modelName, projectName, harnessLogo, anchor, cardId, cardRef, container, onEnter, onLeave, onOpenExperiment, onReturnToRow, onContinue }: {
  session: ChatSession;
  modelName: string | null;
  projectName?: string;
  harnessLogo?: string;
  anchor: DOMRect;
  cardId: string;
  cardRef: React.RefObject<HTMLDivElement | null>;
  container: Element;
  onEnter: () => void;
  onLeave: () => void;
  onOpenExperiment: (projectId: string, experimentId: string) => void;
  onReturnToRow: () => void;
  onContinue: () => void;
}) {
  const { getSessionExperimentsQuery } = useWorkspace().queries;

  const measure = useMeasure();
  const headingId = useId();
  const { data, isPending, error } = useQuery(getSessionExperimentsQuery(session.id));
  const [focusedId, setFocusedId] = useState<string>();
  const [now, setNow] = useState(Date.now);
  const rows = (data ?? []).map((row) => ({
    row,
    model: row.currentRun && runModel({
      item: { sessionId: session.id, run: row.currentRun, experimentName: row.experiment.title?.trim() || row.experiment.slug },
      monitor: null,
      shown: false,
      agent: m.chat_the_agent(),
      now,
      nextRetryMs: null,
    }),
  })).sort((a, b) => rank(a.model) - rank(b.model));
  const live = rows.some(({ model }) => model?.live);
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
  const tabbableId = rows.some(({ row }) => row.experiment.id === focusedId) ? focusedId : rows[0]?.row.experiment.id;

  return createPortal(
    <div
      ref={setCard}
      id={cardId}
      data-session-hover-card={session.id}
      role="group"
      aria-label={title}
      tabIndex={-1}
      className="fixed z-60 flex max-h-[min(32rem,calc(100vh-16px))] flex-col rounded-lg border border-border bg-background p-4 text-sm text-text shadow-menu focus-visible:outline-2 focus-visible:outline-focus-border"
      style={{ width, left: x, top: y, visibility: measure.offsetHeight === 0 ? "hidden" : undefined }}
      onMouseEnter={onEnter}
      onMouseLeave={onLeave}
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
      {projectName && (
        <div className="mt-2 flex shrink-0 items-center gap-1.5 text-subtext">
          <Folder size={14} className="shrink-0" aria-hidden="true" />
          <span className="min-w-0 break-words">{projectName}</span>
        </div>
      )}
      <div className="mt-2 flex shrink-0 items-center gap-1.5 text-subtext">
        <HarnessLogo harness={session.harness} logo={harnessLogo} size={14} />
        <span className="min-w-0 break-words">{harnessLabel(session.harness, session.harnessName)}{modelName ? ` · ${modelName}` : ""}</span>
      </div>
      {(isPending || error || (data && data.length > 0)) && <div className="mt-3 flex min-h-0 flex-col border-t border-border pt-2">
        {(!data || data.length > 0) && (
          <div className="mb-1 flex shrink-0 items-center gap-1.5 text-xs font-medium text-subtext">
            <span id={headingId}>{m.app_experiments()}</span>
            {!!data && <Badge size="small" className="tabular-nums">{fmtNumber(data.length)}</Badge>}
          </div>
        )}
        <div className="-mx-1.5 min-h-0 overflow-y-auto overscroll-contain">
          {isPending ? <div className="px-1.5 text-subtext">{m.common_loading()}</div>
            : error ? <div className="px-1.5 text-subtext">{m.common_failed_to_load({ error: ltr(error.message) })}</div>
              : data && data.length > 0 ? (
                  <ul role="list" aria-labelledby={headingId} className="m-0 list-none p-0">
                    {rows.map(({ row, model }) => (
                      <ExperimentRow
                        key={row.experiment.id}
                        row={row}
                        model={model}
                        sessionProjectId={session.projectId}
                        tabbable={row.experiment.id === tabbableId}
                        onFocus={() => setFocusedId(row.experiment.id)}
                        onOpen={() => onOpenExperiment(row.experiment.projectId, row.experiment.id)}
                      />
                    ))}
                  </ul>
                ) : null}
        </div>
      </div>}
    </div>,
    container,
  );
}
