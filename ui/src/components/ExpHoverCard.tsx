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
  onOpenLogs?: () => void;
  onOpenCode: () => void;
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
      className="exp-hover-card [position:fixed] [z-index:60] [background:var(--base)] [border:1px_solid_var(--border)] [border-radius:var(--radius-lg)] [box-shadow:0_12px_32px_rgba(0,_0,_0,_0.18)] [padding:14px_16px] [font-size:var(--fs-sm)] [color:var(--text)] [&_.hc-mono]:[font-family:var(--mono)] [&_.hc-head]:[display:flex] [&_.hc-head]:[align-items:baseline] [&_.hc-head]:[justify-content:space-between] [&_.hc-head]:[gap:10px] [&_.hc-slug]:[font-family:var(--mono)] [&_.hc-slug]:[font-size:var(--fs-md)] [&_.hc-slug]:[font-weight:var(--fw-semibold)] [&_.hc-slug]:[min-width:0] [&_.hc-slug]:[overflow:hidden] [&_.hc-slug]:[text-overflow:ellipsis] [&_.hc-slug]:[white-space:nowrap] [&_.hc-title]:[margin-top:3px] [&_.hc-title]:[color:var(--text)] [&_.hc-actions]:[display:flex] [&_.hc-actions]:[align-items:center] [&_.hc-actions]:[gap:6px] [&_.hc-actions]:[margin-top:10px] [&_.hc-actions_button]:[display:inline-flex] [&_.hc-actions_button]:[align-items:center] [&_.hc-actions_button]:[justify-content:center] [&_.hc-actions_button]:[gap:5px] [&_.hc-actions_button]:[min-width:84px] [&_.hc-actions_button]:[padding:6px_10px] [&_.hc-actions_button]:[border:1px_solid_var(--border)] [&_.hc-actions_button]:[border-radius:var(--radius-md)] [&_.hc-actions_button]:[background:var(--base)] [&_.hc-actions_button]:[color:var(--text)] [&_.hc-actions_button]:[font-size:var(--fs-sm)] [&_.hc-actions_button]:[font-weight:var(--fw-medium)] [&_.hc-actions_button:hover]:[border-color:color-mix(in_oklab,_var(--border)_55%,_var(--text))] [&_.hc-actions_button:hover]:[background:var(--canvas)] [&_.hc-body]:[margin-top:10px] [&_.hc-body]:[border-top:1px_solid_var(--border-variant)] [&_.hc-body]:[padding-top:10px] [&_.hc-body]:[line-height:1.6] [&_.hc-body]:[white-space:pre-line] [&_.hc-body]:[display:-webkit-box] [&_.hc-body]:[-webkit-line-clamp:10] [&_.hc-body]:[-webkit-box-orient:vertical] [&_.hc-body]:[overflow:hidden] [&_.hc-body.expanded]:[display:block] [&_.hc-body.expanded]:[-webkit-line-clamp:unset] [&_.hc-body.expanded]:[max-height:45vh] [&_.hc-body.expanded]:[overflow-y:auto] [&_.hc-body.expanded]:[padding-bottom:4px] [&_.hc-toggle]:[margin-top:4px] [&_.hc-toggle]:[font-size:var(--fs-xs)] [&_.hc-toggle]:[font-weight:var(--fw-medium)] [&_.hc-toggle]:[color:var(--muted)] [&_.hc-toggle:hover]:[color:var(--text)] [&_.hc-failure]:[margin-top:8px] [&_.hc-failure]:[color:var(--accent-red)] [&_.hc-failure]:[display:-webkit-box] [&_.hc-failure]:[-webkit-line-clamp:3] [&_.hc-failure]:[-webkit-box-orient:vertical] [&_.hc-failure]:[overflow:hidden] [&_.hc-stats]:[margin-top:10px] [&_.hc-stats]:[border-top:1px_solid_var(--border-variant)] [&_.hc-stats]:[padding-top:10px] [&_.hc-stats]:[display:flex] [&_.hc-stats]:[align-items:center] [&_.hc-stats]:[gap:12px] [&_.hc-stats]:[flex-wrap:wrap] [&_.hc-stats]:[font-size:var(--fs-xs)] [&_.hc-stats]:[color:var(--text)] [&_.hc-git]:[margin-top:10px] [&_.hc-git]:[padding-top:8px] [&_.hc-git]:[border-top:1px_solid_var(--border-variant)] [&_.hc-git]:[font-size:var(--fs-xs)] [&_.hc-git]:[color:var(--text)] [&_.hc-git]:[display:flex] [&_.hc-git]:[flex-direction:column] [&_.hc-git]:[gap:4px] [&_.hc-git-row]:[display:flex] [&_.hc-git-row]:[align-items:center] [&_.hc-git-row]:[gap:10px] [&_.hc-git-row]:[flex-wrap:wrap] [&_.hc-git-row]:[min-width:0] [&_.hc-branch]:[display:inline-flex] [&_.hc-branch]:[align-items:center] [&_.hc-branch]:[gap:4px] [&_.hc-branch]:[font-family:var(--mono)] [&_.hc-branch]:[min-width:0] [&_.hc-branch]:[overflow:hidden] [&_.hc-branch]:[text-overflow:ellipsis] [&_.hc-branch]:[white-space:nowrap] [&_.hc-foot]:[margin-top:8px] [&_.hc-foot]:[display:flex] [&_.hc-foot]:[align-items:center] [&_.hc-foot]:[justify-content:space-between] [&_.hc-foot]:[gap:10px] [&_.hc-foot]:[font-size:var(--fs-2xs)] [&_.hc-foot]:[color:var(--muted)] [&_.hc-foot_.hc-mono]:[min-width:0] [&_.hc-foot_.hc-mono]:[overflow:hidden] [&_.hc-foot_.hc-mono]:[text-overflow:ellipsis] [&_.hc-foot_.hc-mono]:[white-space:nowrap]"
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
          <button type="button" onClick={onOpenLogs}>
            <Terminal size={13} />
            Logs
          </button>
        )}
        <button type="button" onClick={onOpenCode}>
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
              <span className="diff-stat-add [color:var(--accent-green)]">+{diffStat.additions}</span>{" "}
              <span className="diff-stat-del [color:var(--accent-red)]">−{diffStat.deletions}</span>
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
