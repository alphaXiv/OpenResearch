import { useQuery } from "@tanstack/react-query";
import { FlaskConical, Folder } from "lucide-react";
import { useCallback, useId, useState } from "react";
import { createPortal } from "react-dom";
import { getSessionExperimentsQuery } from "../queries/chat";
import { m } from "../paraglide/messages.js";
import { fmtNumber, ltr } from "../i18n";
import { fmtDuration, runDisplayStatus, timeAgo } from "../api";
import type { ChatSession, RunDisplayStatus, SessionExperiment } from "../api";
import { harnessLabel } from "./ModelPicker";
import { HarnessLogo } from "./HarnessLogo";
import { statusLabel, statusStyle } from "./StatusBadge";
import { useMeasure, usePopoverPosition } from "./tourGeometry";
import { Badge } from "./ui";

const CARD_WIDTH = 320;
const GAP = 8;

const ICON_TONES: Partial<Record<RunDisplayStatus, string>> = {
  starting: "text-accent-green",
  running: "text-accent-green",
  cancelling: "text-accent-amber",
  failed: "text-accent-red",
};

function ExperimentRow({ row, sessionProjectId, tabbable, onFocus, onOpen }: {
  row: SessionExperiment;
  sessionProjectId: string;
  tabbable: boolean;
  onFocus: () => void;
  onOpen: () => void;
}) {
  const { experiment, currentRun, projectName } = row;
  const status = currentRun ? runDisplayStatus(currentRun) : null;
  const tone = status ? ICON_TONES[status] ?? "text-subtext" : "text-muted";
  const live = status !== null && statusStyle(status).live;
  const elapsed = currentRun && (live ? fmtDuration(Date.now() - currentRun.createdAt) : timeAgo(currentRun.endedAt ?? currentRun.createdAt));

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
        <FlaskConical
          size={14}
          className={`mt-[3px] shrink-0 ${tone} ${live ? "motion-safe:animate-[or-experiment-pulse_3s_ease-in-out_infinite]" : ""}`}
          aria-hidden="true"
        />
        <span className="min-w-0 flex-1">
          <span className="block break-words">{experiment.title?.trim() || experiment.slug}</span>
          <span className="mt-0.5 flex flex-wrap items-center gap-x-1.5 text-xs text-subtext">
            {status ? <span className="text-text">{statusLabel(status)}</span> : <span>{m.experiments_not_run_yet()}</span>}
            {elapsed && <span className="tabular-nums">{elapsed}</span>}
            {experiment.archived && <span>{m.tree_archived()}</span>}
            {experiment.projectId !== sessionProjectId && (
              <span className="ms-auto flex min-w-0 items-center gap-1">
                <Folder size={12} className="shrink-0" aria-hidden="true" />
                <span className="truncate">{projectName}</span>
              </span>
            )}
          </span>
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
  const measure = useMeasure();
  const headingId = useId();
  const { data, isPending, error } = useQuery(getSessionExperimentsQuery(session.id));
  const [focusedId, setFocusedId] = useState<string>();
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
  const tabbableId = data?.some(({ experiment }) => experiment.id === focusedId) ? focusedId : data?.[0]?.experiment.id;

  return createPortal(
    <div
      ref={setCard}
      id={cardId}
      data-session-hover-card={session.id}
      role="group"
      aria-label={title}
      tabIndex={-1}
      className="fixed z-60 flex max-h-[min(24rem,calc(100vh-16px))] flex-col rounded-lg border border-border bg-background p-4 text-sm text-text shadow-menu focus-visible:outline-2 focus-visible:outline-focus-border"
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
                    {data.map((row) => (
                      <ExperimentRow
                        key={row.experiment.id}
                        row={row}
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
