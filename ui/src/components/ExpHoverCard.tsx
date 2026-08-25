// Floating detail card shown while hovering an experiment node in the tree.
// Rendered through a portal at a fixed viewport position (never inside the
// ReactFlow node: the canvas transform would scale it with zoom, and growing
// the node itself would invalidate the tree layout's fixed NODE_W/NODE_H).
// Pointer-only by design — everything it shows is also reachable through the
// node's own views, so keyboard/touch users lose a shortcut, not a capability.
// Client-only (portals straight into document.body).

import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { createPortal } from "react-dom";
import { FolderTree, GitBranch, Terminal } from "lucide-react";
import { parseDiff, type FileData } from "react-diff-view";
import {
  backendKind,
  fmtDuration,
  getRunDiff,
  runDisplayStatus,
  timeAgo,
  type Experiment,
  type Run,
} from "../api";
import { BackendBadge } from "./BackendLogos";
import { countChanges } from "./GitDiff";
import { StatusBadge } from "./StatusBadge";
import { useMeasure, usePopoverPosition } from "./tourGeometry";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";

const CARD_W = 380;
const GAP = 12; // node ↔ card

// Hover-intent timings: long enough that sweeping the cursor across the tree
// opens nothing, short enough to feel deliberate. The close grace lets the
// cursor cross the gap onto the card itself.
const HOVER_OPEN_MS = 350;
const HOVER_CLOSE_MS = 150;

// Broadcast channel for canvas pan/zoom: TreeView's onMoveStart pings it and
// every open card dismisses. A single top-level subscriber is deliberate —
// @xyflow's useOnViewportChange stores ONE callback globally, so per-node
// registrations silently overwrite each other.
const treeViewportMoves = new EventTarget();
export function dismissTreeHoverCards() {
  treeViewportMoves.dispatchEvent(new Event("move"));
}

/** Hover-intent state for a node's detail card. `rect` is non-null while the
 * card should be open; it re-measures whenever `refreshKey` changes so SSE
 * updates that re-lay-out the tree can't leave the card pointing at a stale
 * position, and closes on any canvas pan/zoom (dismissTreeHoverCards). */
export function useHoverIntent(ref: RefObject<HTMLElement | null>, refreshKey: unknown) {
  const [rect, setRect] = useState<DOMRect | null>(null);
  const openTimer = useRef<number | undefined>(undefined);
  const closeTimer = useRef<number | undefined>(undefined);
  useEffect(() => {
    const dismiss = () => {
      window.clearTimeout(openTimer.current);
      window.clearTimeout(closeTimer.current);
      setRect(null);
    };
    treeViewportMoves.addEventListener("move", dismiss);
    return () => {
      treeViewportMoves.removeEventListener("move", dismiss);
      window.clearTimeout(openTimer.current);
      window.clearTimeout(closeTimer.current);
    };
  }, []);
  useEffect(() => {
    setRect((prev) => {
      if (!prev) return prev;
      const next = ref.current?.getBoundingClientRect() ?? null;
      // Keep the old rect when nothing moved, so SSE ticks that change data
      // without re-laying-out the tree don't churn renders.
      if (
        next &&
        prev.x === next.x &&
        prev.y === next.y &&
        prev.width === next.width &&
        prev.height === next.height
      )
        return prev;
      return next;
    });
  }, [ref, refreshKey]);
  const onMouseEnter = useCallback(() => {
    window.clearTimeout(closeTimer.current);
    window.clearTimeout(openTimer.current);
    openTimer.current = window.setTimeout(() => {
      setRect(ref.current?.getBoundingClientRect() ?? null);
    }, HOVER_OPEN_MS);
  }, [ref]);
  const onMouseLeave = useCallback(() => {
    window.clearTimeout(openTimer.current);
    window.clearTimeout(closeTimer.current);
    closeTimer.current = window.setTimeout(() => setRect(null), HOVER_CLOSE_MS);
  }, []);
  const keepOpen = useCallback(() => window.clearTimeout(closeTimer.current), []);
  return { rect, onMouseEnter, onMouseLeave, keepOpen };
}

interface DiffStat {
  fileCount: number;
  additions: number;
  deletions: number;
  truncated: boolean;
}

function fmtCreated(ms: number): string {
  const d = new Date(ms);
  const opts: Intl.DateTimeFormatOptions =
    d.getFullYear() === new Date().getFullYear()
      ? { month: "short", day: "numeric" }
      : { month: "short", day: "numeric", year: "numeric" };
  return d.toLocaleDateString(undefined, opts);
}

export function ExpHoverCard({
  exp,
  runs,
  latestRun,
  parentSlug,
  anchor,
  onOpenLogs,
  onOpenCode,
  onMouseEnter,
  onMouseLeave,
}: {
  exp: Experiment;
  runs: Run[];
  latestRun: Run | null;
  parentSlug: string | null;
  /** Viewport rect of the hovered node (kept fresh by useHoverIntent). */
  anchor: DOMRect;
  onOpenLogs?: (intent: TabOpenIntent) => void;
  onOpenCode: (intent: TabOpenIntent) => void;
  onMouseEnter: () => void;
  onMouseLeave: () => void;
}) {
  const measure = useMeasure();
  // Prefer beside the node; on windows too narrow for either side, go
  // above/below — usePopoverPosition only clamps, and a clamped side
  // placement would sit the card on top of the node it describes.
  const fitsRight = anchor.right + GAP + CARD_W <= window.innerWidth;
  const fitsLeft = anchor.x - GAP - CARD_W >= 0;
  const side = fitsRight
    ? "right"
    : fitsLeft
      ? "left"
      : anchor.y > window.innerHeight / 2
        ? "above"
        : "below";
  const { x, y } = usePopoverPosition(
    { x: anchor.x, y: anchor.y, width: anchor.width, height: anchor.height, anchor: side, distance: GAP },
    measure,
  );
  const [diffStat, setDiffStat] = useState<DiffStat | null>(null);

  // Diffstat of the branch vs its parent — only defined for non-baseline runs
  // that committed something (the endpoint 400s otherwise).
  const diffRunId = exp.parentExperimentId && latestRun?.commitSha ? latestRun.id : null;
  useEffect(() => {
    // Reset on every run change so a previous run's stat can't linger next
    // to the new run's status while (or if) the new fetch resolves.
    setDiffStat(null);
    if (!diffRunId) return;
    let cancelled = false;
    getRunDiff(diffRunId)
      .then((p) => {
        // Same parser and counting helpers as the Changes tab, so renames,
        // quoted paths and binary files are counted and labeled identically.
        let text = p.diff;
        if (p.truncated) {
          // The backend byte-caps mid-line, which can crash the parser (a cut
          // inside an @@ header) — drop the trailing partial file so the
          // counts stay honest lower bounds.
          const cut = text.lastIndexOf("\ndiff --git ");
          text = cut !== -1 ? text.slice(0, cut + 1) : text.slice(0, text.lastIndexOf("\n") + 1);
        }
        let files: FileData[] = [];
        try {
          files = text.trim() ? parseDiff(text) : [];
        } catch {
          return; // malformed even after trimming — skip the row
        }
        // A truncated single-file diff can trim down to a bare header that
        // parses as one file with no hunks; "≥ +0 −0 · 1+ files" is noise.
        if (p.truncated && files.every((f) => f.hunks.length === 0)) return;
        let additions = 0;
        let deletions = 0;
        for (const f of files) {
          const c = countChanges(f);
          additions += c.additions;
          deletions += c.deletions;
        }
        if (!cancelled) {
          setDiffStat({
            fileCount: files.length,
            additions,
            deletions,
            truncated: p.truncated,
          });
        }
      })
      .catch(() => {
        // Diffstat is a nice-to-have; drop the row on fetch or parse failure.
      });
    return () => {
      cancelled = true;
    };
  }, [diffRunId]);

  const counts = { done: 0, failed: 0, cancelled: 0, live: 0 };
  for (const r of runs) {
    if (r.status === "done") counts.done += 1;
    else if (r.status === "failed") counts.failed += 1;
    else if (r.status === "cancelled") counts.cancelled += 1;
    else counts.live += 1; // starting | running
  }
  const duration = latestRun
    ? fmtDuration((latestRun.endedAt ?? Date.now()) - latestRun.createdAt)
    : null;
  // Local runs leave resultMarkdown empty on success (the agent freezes its
  // findings into the experiment description instead); on failure it holds a
  // short error worth surfacing — but never in both slots at once.
  const failureNote =
    latestRun?.status === "failed" && latestRun.resultMarkdown ? latestRun.resultMarkdown : null;
  const body = exp.description || (failureNote ? null : latestRun?.resultMarkdown) || null;

  // Clamped by default; "Show more" appears when the clamp actually hides
  // content and stays while expanded so "Show less" remains reachable.
  const bodyRef = useRef<HTMLDivElement>(null);
  const [expanded, setExpanded] = useState(false);
  const [clamped, setClamped] = useState(false);
  useEffect(() => {
    // Re-collapse when SSE swaps the text out from under an expanded card.
    setExpanded(false);
  }, [body]);
  useEffect(() => {
    const el = bodyRef.current;
    if (el) setClamped(el.scrollHeight > el.clientHeight + 1);
  }, [body, expanded]);

  return createPortal(
    <div
      ref={measure.ref}
      className="exp-hover-card fixed z-60 bg-background border border-border rounded-lg shadow-[0_12px_32px_rgba(0,_0,_0,_0.18)] py-3.5 px-4 text-sm text-text [&_.hc-mono]:font-mono [&_.hc-head]:flex [&_.hc-head]:items-baseline [&_.hc-head]:justify-between [&_.hc-head]:gap-2.5 [&_.hc-slug]:font-mono [&_.hc-slug]:text-md [&_.hc-slug]:font-semibold [&_.hc-slug]:min-w-0 [&_.hc-slug]:overflow-hidden [&_.hc-slug]:text-ellipsis [&_.hc-slug]:whitespace-nowrap [&_.hc-title]:mt-[3px] [&_.hc-title]:text-text [&_.hc-actions]:flex [&_.hc-actions]:items-center [&_.hc-actions]:gap-1.5 [&_.hc-actions]:mt-2.5 [&_.hc-actions_button]:inline-flex [&_.hc-actions_button]:items-center [&_.hc-actions_button]:justify-center [&_.hc-actions_button]:gap-[5px] [&_.hc-actions_button]:min-w-21 [&_.hc-actions_button]:py-1.5 [&_.hc-actions_button]:px-2.5 [&_.hc-actions_button]:border [&_.hc-actions_button]:border-border [&_.hc-actions_button]:rounded-md [&_.hc-actions_button]:bg-background [&_.hc-actions_button]:text-text [&_.hc-actions_button]:text-sm [&_.hc-actions_button]:font-medium [&_.hc-actions_button:hover]:border-[color-mix(in_oklab,_var(--border)_55%,_var(--text))] [&_.hc-actions_button:hover]:bg-canvas [&_.hc-body]:mt-2.5 [&_.hc-body]:border-t [&_.hc-body]:border-t-border-variant [&_.hc-body]:pt-2.5 [&_.hc-body]:leading-[1.6] [&_.hc-body]:whitespace-pre-line [&_.hc-body]:line-clamp-10 [&_.hc-body.expanded]:block [&_.hc-body.expanded]:line-clamp-none [&_.hc-body.expanded]:max-h-[45vh] [&_.hc-body.expanded]:overflow-y-auto [&_.hc-body.expanded]:overflow-x-hidden [&_.hc-body.expanded]:pb-1 [&_.hc-toggle]:mt-1 [&_.hc-toggle]:text-xs [&_.hc-toggle]:font-medium [&_.hc-toggle]:text-muted [&_.hc-toggle:hover]:text-text [&_.hc-failure]:mt-2 [&_.hc-failure]:text-accent-red [&_.hc-failure]:line-clamp-3 [&_.hc-stats]:mt-2.5 [&_.hc-stats]:border-t [&_.hc-stats]:border-t-border-variant [&_.hc-stats]:pt-2.5 [&_.hc-stats]:flex [&_.hc-stats]:items-center [&_.hc-stats]:gap-3 [&_.hc-stats]:flex-wrap [&_.hc-stats]:text-xs [&_.hc-stats]:text-text [&_.hc-git]:mt-2.5 [&_.hc-git]:pt-2 [&_.hc-git]:border-t [&_.hc-git]:border-t-border-variant [&_.hc-git]:text-xs [&_.hc-git]:text-text [&_.hc-git]:flex [&_.hc-git]:flex-col [&_.hc-git]:gap-1 [&_.hc-git-row]:flex [&_.hc-git-row]:items-center [&_.hc-git-row]:gap-2.5 [&_.hc-git-row]:flex-wrap [&_.hc-git-row]:min-w-0 [&_.hc-branch]:inline-flex [&_.hc-branch]:items-center [&_.hc-branch]:gap-1 [&_.hc-branch]:font-mono [&_.hc-branch]:min-w-0 [&_.hc-branch]:overflow-hidden [&_.hc-branch]:text-ellipsis [&_.hc-branch]:whitespace-nowrap [&_.hc-foot]:mt-2 [&_.hc-foot]:flex [&_.hc-foot]:items-center [&_.hc-foot]:justify-between [&_.hc-foot]:gap-2.5 [&_.hc-foot]:text-2xs [&_.hc-foot]:text-muted [&_.hc-foot_.hc-mono]:min-w-0 [&_.hc-foot_.hc-mono]:overflow-hidden [&_.hc-foot_.hc-mono]:text-ellipsis [&_.hc-foot_.hc-mono]:whitespace-nowrap"
      style={{
        width: CARD_W,
        left: x,
        top: y,
        // Hide the pre-measure frame (the position hooks need a real height).
        visibility: measure.offsetHeight === 0 ? "hidden" : undefined,
      }}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
    >
      <div className="hc-head">
        <span className="hc-slug">{exp.slug}</span>
        <StatusBadge status={latestRun ? runDisplayStatus(latestRun) : "idle"} />
      </div>
      {exp.title && <div className="hc-title">{exp.title}</div>}
      <div className="hc-actions">
        {onOpenLogs && (
          <button
            type="button"
            {...tabOpenGestureHandlers<HTMLButtonElement>(onOpenLogs)}
          >
            <Terminal size={13} />
            Logs
          </button>
        )}
        <button
          type="button"
          {...tabOpenGestureHandlers<HTMLButtonElement>(onOpenCode)}
        >
          <FolderTree size={13} />
          Code
        </button>
      </div>
      {body && (
        <div className={`hc-body${expanded ? " expanded" : ""}`} ref={bodyRef}>
          {body}
        </div>
      )}
      {body && (clamped || expanded) && (
        <button type="button" className="hc-toggle" onClick={() => setExpanded((v) => !v)}>
          {expanded ? "Show less" : "Show more"}
        </button>
      )}
      {failureNote && <div className="hc-failure">{failureNote}</div>}
      <div className="hc-stats">
        <span>
          {runs.length === 1 ? "1 run" : `${runs.length} runs`}
          {counts.done > 0 && ` · ${counts.done} done`}
          {counts.failed > 0 && ` · ${counts.failed} failed`}
          {counts.cancelled > 0 && ` · ${counts.cancelled} cancelled`}
          {counts.live > 0 && ` · ${counts.live} live`}
        </span>
        {latestRun && backendKind(latestRun.backend) && <BackendBadge backend={latestRun.backend} />}
        {duration && <span>{duration}</span>}
        {latestRun && <span>{timeAgo(latestRun.createdAt)}</span>}
      </div>
      <div className="hc-git">
        <div className="hc-git-row">
          <span className="hc-branch" title={exp.branchName}>
            <GitBranch size={12} />
            {exp.branchName}
          </span>
          {parentSlug && (
            <span>
              from <span className="hc-mono">{parentSlug}</span>
            </span>
          )}
        </div>
        {diffStat && diffStat.fileCount > 0 && (
          <div
            className="hc-git-row"
            title={`Committed changes vs ${parentSlug ?? "parent"}${diffStat.truncated ? " (diff truncated — counts are lower bounds)" : ""}`}
          >
            <span>
              {diffStat.truncated && "≥ "}
              <span className="diff-stat-add text-accent-green">+{diffStat.additions}</span>{" "}
              <span className="diff-stat-del text-accent-red">−{diffStat.deletions}</span>
              {" · "}
              {diffStat.fileCount === 1 && !diffStat.truncated
                ? "1 file"
                : `${diffStat.fileCount}${diffStat.truncated ? "+" : ""} files`}
            </span>
          </div>
        )}
      </div>
      <div className="hc-foot">
        <span className="hc-mono">$ {exp.runCommand}</span>
        <span>created {fmtCreated(exp.createdAt)}</span>
      </div>
    </div>,
    document.body,
  );
}
