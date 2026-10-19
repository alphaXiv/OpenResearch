import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { useLocale } from "../locale";
import {
  applyNodeChanges,
  Background,
  BackgroundVariant,
  Handle,
  MiniMap,
  Panel,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  type Edge,
  type Node,
  type NodeChange,
  type NodeProps,
  type Viewport,
} from "@xyflow/react";
import {
  Archive,
  ChevronDown,
  ChevronUp,
  CircleStop,
  Ellipsis,
  FolderTree,
  Map as MapIcon,
  Maximize2,
  RotateCcw,
  Search,
  Terminal,
  X,
  ZoomIn,
  ZoomOut,
  Workflow,
} from "lucide-react";
import { createPortal } from "react-dom";
import { BackendBadge, GitHubMark } from "./BackendLogos";
import { memo, useCallback, useEffect, useMemo, useRef, useState, type Dispatch, type ReactNode, type SetStateAction } from "react";
import {
  githubBranchUrl,
  fmtDuration,
  fmtNumber,
  runDisplayStatus,
  timeAgo,
  type Experiment,
  type Project,
  type Run,
} from "../api";
import type { ExperimentView } from "./DetailDrawer";
import type { CodeView } from "./CodeTab";
import { ExpHoverCard, dismissTreeHoverCards, useHoverIntent } from "./ExpHoverCard";
import { ArchiveMenu, type ArchiveActions } from "./ArchiveMenu";
import { usePopover } from "./ModelPicker";
import { MenuItem } from "./ui";
import { statusLabel, StatusBadge } from "./StatusBadge";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";
import { TreeInspector } from "./TreeInspector";
import type { ExperimentTreeState } from "../workspaceState";
import { compactTreeLayout, type TreeLayoutNode } from "../treeLayout";

const EMPTY_STATE_CLASS_NAME = [
  "empty-state absolute inset-0 flex flex-col items-center",
  "justify-center p-6 text-center text-subtext [&_p]:max-w-[46ch]",
  "[&_p]:m-0 [&_p]:text-sm [&_p]:leading-normal [&_p]:text-balance",
  "[&_p.empty-state-title]:text-2xl [&_p.empty-state-title]:font-normal",
  "[&_p.empty-state-title]:text-text [&_p.empty-state-hint]:text-lg",
  "[&_p.empty-state-hint]:text-subtext empty-state-cta gap-1.5",
].join(" ");

const NODE_W = 264;
const NODE_H = 220;
const MAX_SQUARES = 8;
// Keep in sync with the inline `.elided-node` classes below.
const ELIDED_W = 148;
const ELIDED_H = 44;
const ARCHIVED_W = 160;

type ExpNodeData = {
  exp: Experiment;
  latestRun: Run | null;
  liveRun: Run | null;
  runs: Run[]; // oldest → newest
  isBaseline: boolean;
  parentSlug: string | null;
  githubOwner: string;
  githubRepo: string;
  onOpenView: (id: string, view: ExperimentView, intent: TabOpenIntent) => void;
  onOpenCode: (
    experimentId: string,
    branch: string,
    view: CodeView,
    intent: TabOpenIntent,
  ) => void;
  onArchive: (id: string, direction: "ancestors" | "descendants" | "only", archived: boolean) => void;
  actions: ArchiveActions;
  onCancel?: (runId: string) => Promise<void>;
  cancelling: boolean;
  isSelected?: boolean;
  isAncestor?: boolean;
  isDimmed?: boolean;
  onSelect?: () => void;
};
type ExpFlowNode = Node<ExpNodeData, "exp">;

type ArchivedNodeData = {
  rootId: string;
  count: number;
  onRestoreRegion: (id: string) => void;
};
type ArchivedFlowNode = Node<ArchivedNodeData, "archived">;

type ElidedNodeData = {
  count: number;
  onShowProjectScope: () => void;
};
type ElidedFlowNode = Node<ElidedNodeData, "elided">;
type FlowNode = ExpFlowNode | ArchivedFlowNode | ElidedFlowNode;

interface TreeNode {
  exp: Experiment;
  children: TreeNode[];
}

/** What the layout draws: experiment cards and hidden-region pills. */
type DisplayNode =
  | { kind: "exp"; exp: Experiment; children: DisplayNode[] }
  | { kind: "archived"; id: string; rootId: string; count: number; children: DisplayNode[] }
  | { kind: "elided"; id: string; count: number; children: DisplayNode[] };

function buildForest(experiments: Experiment[]): TreeNode[] {
  const byId = new Map(experiments.map((e) => [e.id, { exp: e, children: [] as TreeNode[] }]));
  const roots: TreeNode[] = [];
  for (const e of experiments) {
    const node = byId.get(e.id)!;
    const parent = e.parentExperimentId ? byId.get(e.parentExperimentId) : undefined;
    if (parent) parent.children.push(node);
    else roots.push(node);
  }
  const byCreated = (a: TreeNode, b: TreeNode) => a.exp.createdAt - b.exp.createdAt;
  const sortRec = (n: TreeNode) => {
    n.children.sort(byCreated);
    n.children.forEach(sortRec);
  };
  roots.sort(byCreated);
  roots.forEach(sortRec);
  return roots;
}

/** Keep the nodes `mine` accepts, collapse each maximal rejected region on the
 * path to them into one "…" pill, and drop rejected subtrees that lead
 * nowhere. Elided ids key off the region root, so they're stable across renders. */
function elideHiddenRegions(roots: TreeNode[], mine: (n: TreeNode) => boolean): DisplayNode[] {
  // Memoized so the pass stays linear on deep chains.
  const sizes = new Map<TreeNode, number>();
  const size = (n: TreeNode): number => {
    const memo = sizes.get(n) ?? 1 + n.children.reduce((s, c) => s + size(c), 0);
    sizes.set(n, memo);
    return memo;
  };
  const mines = new Map<TreeNode, boolean>();
  const hasMine = (n: TreeNode): boolean => {
    const memo = mines.get(n) ?? (mine(n) || n.children.some(hasMine));
    mines.set(n, memo);
    return memo;
  };
  function visit(node: TreeNode): DisplayNode[] {
    if (mine(node)) {
      const children: DisplayNode[] = [];
      let foreignCount = 0;
      for (const c of node.children) {
        if (hasMine(c)) children.push(...visit(c));
        else foreignCount += size(c);
      }
      // The pill goes after the real children (not createdAt-sorted) so the
      // placeholder stays out of the chronological left-to-right reading.
      if (foreignCount > 0)
        children.push({
          kind: "elided",
          id: `el-${node.exp.id}`,
          count: foreignCount,
          children: [],
        });
      return [{ kind: "exp", exp: node.exp, children }];
    }
    if (!hasMine(node)) return [];
    let count = 0;
    const mineChildren: DisplayNode[] = [];
    // Walk the foreign region rooted here, tallying every node in it and
    // recursing out through each kept descendant; `size(c)` swallows whole
    // foreign subtrees that contain nothing kept.
    (function absorb(n: TreeNode) {
      count += 1;
      for (const c of n.children) {
        if (mine(c)) mineChildren.push(...visit(c));
        else if (hasMine(c)) absorb(c);
        else count += size(c);
      }
    })(node);
    return [{ kind: "elided", id: `el-${node.exp.id}`, count, children: mineChildren }];
  }
  return roots.flatMap(visit);
}

function collapseArchivedRegions(roots: DisplayNode[]): DisplayNode[] {
  function visit(node: DisplayNode): DisplayNode {
    if (node.kind !== "exp" || !node.exp.archived)
      return { ...node, children: node.children.map(visit) };
    let count = 0;
    const children: DisplayNode[] = [];
    function absorb(archived: DisplayNode & { kind: "exp" }) {
      count += 1;
      for (const child of archived.children) {
        if (child.kind === "exp" && child.exp.archived) absorb(child);
        else children.push(visit(child));
      }
    }
    absorb(node);
    return { kind: "archived", id: `ar-${node.exp.id}`, rootId: node.exp.id, count, children };
  }
  return roots.map(visit);
}

function nodeWidth(node: DisplayNode): number {
  return node.kind === "exp" ? NODE_W : node.kind === "archived" ? ARCHIVED_W : ELIDED_W;
}

function nodeHeight(node: DisplayNode): number {
  return node.kind === "exp" ? NODE_H : ELIDED_H;
}

function nodeId(node: DisplayNode): string {
  return node.kind === "exp" ? node.exp.id : node.id;
}

function runSquareClass(status: string): string {
  if (status === "done") return "pass";
  if (status === "failed") return "fail";
  if (status === "running" || status === "starting" || status === "cancelling") return "live";
  return "other";
}

function TreeControls({
  onResetLayout,
  showMiniMap,
  onToggleMiniMap,
}: {
  onResetLayout: () => void;
  showMiniMap: boolean;
  onToggleMiniMap: () => void;
}) {
  const { zoomIn, zoomOut, fitView } = useReactFlow();
  return (
    <Panel position="bottom-left" className="nodrag m-3">
      <div
        className="flex items-center gap-0.5 rounded-lg border border-border bg-background p-1 shadow-menu"
        role="toolbar"
        aria-label={m.tree_canvas_controls()}
      >
        <button
          type="button"
          onClick={() => zoomIn({ duration: 150 })}
          className="flex h-7 w-7 items-center justify-center rounded-md text-text transition-colors hover:bg-surface active:bg-hover-subtle cursor-pointer"
          title={m.image_zoom_in()}
          aria-label={m.image_zoom_in()}
        >
          <ZoomIn size={15} />
        </button>
        <button
          type="button"
          onClick={() => zoomOut({ duration: 150 })}
          className="flex h-7 w-7 items-center justify-center rounded-md text-text transition-colors hover:bg-surface active:bg-hover-subtle cursor-pointer"
          title={m.image_zoom_out()}
          aria-label={m.image_zoom_out()}
        >
          <ZoomOut size={15} />
        </button>
        <div className="mx-0.5 h-3.5 w-px bg-border-variant" />
        <button
          type="button"
          onClick={() => fitView({ duration: 250, padding: 0.25, maxZoom: 1 })}
          className="flex h-7 w-7 items-center justify-center rounded-md text-text transition-colors hover:bg-surface active:bg-hover-subtle cursor-pointer"
          title={m.image_zoom_reset()}
          aria-label={m.image_zoom_reset()}
        >
          <Maximize2 size={14} />
        </button>
        <button
          type="button"
          onClick={onResetLayout}
          className="flex h-7 w-7 items-center justify-center rounded-md text-text transition-colors hover:bg-surface active:bg-hover-subtle cursor-pointer"
          title={m.tree_arrange_hint()}
          aria-label={m.tree_arrange_hint()}
        >
          <RotateCcw size={14} />
        </button>
        <div className="mx-0.5 h-3.5 w-px bg-border-variant" />
        <button
          type="button"
          onClick={onToggleMiniMap}
          className={`flex h-7 w-7 items-center justify-center rounded-md transition-colors hover:bg-surface cursor-pointer ${
            showMiniMap ? "bg-surface text-primary" : "text-text"
          }`}
          title={m.tree_toggle_minimap()}
          aria-label={m.tree_toggle_minimap()}
          aria-pressed={showMiniMap}
        >
          <MapIcon size={14} />
        </button>
      </div>
    </Panel>
  );
}

function TreeSearchToolbar({
  search, onSearchChange, statusFilter, onStatusFilterChange,
  matchCount, currentMatchIndex, onNextMatch, onPrevMatch,
  onArrange, autoLayout, onToggleAutoLayout, hasNodes, actions,
}: {
  search: string;
  onSearchChange: (value: string) => void;
  statusFilter: "all" | "active" | "failed";
  onStatusFilterChange: (filter: "all" | "active" | "failed") => void;
  matchCount: number;
  currentMatchIndex: number;
  onNextMatch: () => void;
  onPrevMatch: () => void;
  onArrange: () => void;
  autoLayout: boolean;
  onToggleAutoLayout: () => void;
  hasNodes: boolean;
  actions?: ReactNode;
}) {
  const searchInputRef = useRef<HTMLInputElement>(null);
  const filtering = search.trim().length > 0 || statusFilter !== "all";

  return (
    <div className="tree-search-toolbar z-50 flex shrink-0 flex-wrap items-center gap-2 border-b border-border bg-background px-3 py-2" role="toolbar" aria-label={m.app_experiment_view()}>
      <div className="relative flex min-w-44 flex-1 basis-52 items-center" role="search">
        <Search size={14} className="pointer-events-none absolute start-2.5 text-muted" aria-hidden="true" />
        <input
          ref={searchInputRef}
          type="search"
          value={search}
          onChange={(event) => onSearchChange(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              if (event.shiftKey) onPrevMatch();
              else onNextMatch();
            } else if (event.key === "Escape") {
              event.stopPropagation();
              onSearchChange("");
            }
          }}
          placeholder={m.experiments_search()}
          aria-label={m.experiments_search()}
          className={`h-8 w-full min-w-0 rounded-lg border border-border-variant bg-surface/50 ps-8 text-xs text-text placeholder:text-muted focus:border-border-strong focus:outline-2 focus:outline-accent [&::-webkit-search-cancel-button]:appearance-none ${filtering ? "pe-28" : "pe-3"}`}
        />
        {filtering && (
          <div className="absolute end-1.5 flex items-center gap-0.5">
            <span className="px-1 text-xs tabular-nums text-subtext" role="status" aria-live="polite">{matchCount > 0 ? `${currentMatchIndex + 1}/${matchCount}` : "0"}</span>
            <button type="button" disabled={matchCount === 0} onClick={onPrevMatch} className="flex h-6 w-5 cursor-pointer items-center justify-center rounded text-subtext hover:bg-hover-subtle hover:text-text disabled:pointer-events-none disabled:opacity-30" title={`${m.pdf_preview_previous_match()} (Shift+Enter)`} aria-label={m.pdf_preview_previous_match()}><ChevronUp size={12} /></button>
            <button type="button" disabled={matchCount === 0} onClick={onNextMatch} className="flex h-6 w-5 cursor-pointer items-center justify-center rounded text-subtext hover:bg-hover-subtle hover:text-text disabled:pointer-events-none disabled:opacity-30" title={`${m.pdf_preview_next_match()} (Enter)`} aria-label={m.pdf_preview_next_match()}><ChevronDown size={12} /></button>
            {search && <button type="button" onClick={() => { onSearchChange(""); searchInputRef.current?.focus(); }} className="ms-0.5 flex h-6 w-5 cursor-pointer items-center justify-center rounded text-subtext hover:bg-hover-subtle hover:text-text" title={m.experiments_clear_search()} aria-label={m.experiments_clear_search()}><X size={12} /></button>}
          </div>
        )}
      </div>
      <div className="inline-flex h-8 shrink-0 items-center gap-0.5 rounded-lg bg-surface p-0.5" role="group" aria-label={m.app_filter_experiments()}>
        {(["all", "active", "failed"] as const).map((key) => (
          <button key={key} type="button" aria-pressed={statusFilter === key} onClick={() => onStatusFilterChange(key)} className={`h-7 cursor-pointer rounded-md px-2.5 text-xs font-medium transition-colors ${statusFilter === key ? "bg-background text-text shadow-xs" : "text-subtext hover:text-text"}`}>
            {key === "all" ? m.chat_panel_all() : key === "active" ? m.chat_panel_active() : m.status_failed()}
          </button>
        ))}
      </div>
      <div className="inline-flex h-8 shrink-0 items-center gap-0.5 rounded-lg border border-border-variant bg-surface/50 p-0.5">
        <button type="button" onClick={onArrange} disabled={!hasNodes} className="inline-flex h-7 cursor-pointer items-center gap-1.5 rounded-md px-2 text-xs font-medium text-text transition-colors hover:bg-hover-subtle disabled:cursor-default disabled:opacity-40" title={m.tree_arrange_hint()} aria-label={m.tree_arrange_hint()}><Workflow size={14} aria-hidden="true" />{m.tree_arrange()}</button>
        <button type="button" onClick={onToggleAutoLayout} aria-pressed={autoLayout} className={`inline-flex h-6 cursor-pointer items-center gap-1 rounded-md px-1.5 text-xs font-medium transition-colors ${autoLayout ? "bg-primary-subtle text-primary" : "text-muted hover:bg-hover-subtle hover:text-text"}`} title={m.tree_auto_layout_hint()} aria-label={m.tree_auto_layout_hint()}><span className={`h-1.5 w-1.5 rounded-full ${autoLayout ? "bg-primary" : "bg-muted"}`} aria-hidden="true" />{m.tree_auto_layout()}</button>
      </div>
      {actions && <div className="ms-auto flex shrink-0 items-center">{actions}</div>}
    </div>
  );
}

const ExpNode = memo(function ExpNode({ data }: NodeProps<ExpFlowNode>) {
  useLocale();
  const {
    exp,
    latestRun,
    liveRun,
    runs,
    isBaseline,
    parentSlug,
    githubOwner,
    githubRepo,
    onOpenView,
    onOpenCode,
    onArchive,
    actions,
    onCancel,
    cancelling,
    isSelected,
    isAncestor,
    isDimmed,
    onSelect,
  } = data;
  const logsRun = liveRun ?? latestRun;
  const status = liveRun
    ? cancelling
      ? "cancelling"
      : runDisplayStatus(liveRun)
    : latestRun
      ? runDisplayStatus(latestRun)
      : undefined;
  const live = status === "running" || status === "starting" || status === "cancelling";
  const kind = isBaseline ? m.tree_baseline() : live ? m.tree_running() : m.tree_experiment();
  const squares = runs.slice(-MAX_SQUARES);

  const rootRef = useRef<HTMLDivElement>(null);
  const hover = useHoverIntent(rootRef, data);

  return (
    <div
      ref={rootRef}
      className={`exp-node w-66 border rounded-md bg-background py-2.5 px-3 text-sm transition-all duration-120 ease-standard cursor-grab active:cursor-grabbing ${
        isSelected
          ? "border-accent-teal ring-2 ring-accent-teal shadow-tree-live"
          : isAncestor
            ? "border-accent-teal ring-1 ring-accent-teal/50 shadow-tree"
            : live
              ? "border-accent-teal shadow-tree-live live"
              : "border-border shadow-tree hover:shadow-tree-hover"
      } ${isDimmed ? "opacity-25 hover:opacity-100" : ""} ${exp.archived ? "opacity-80" : ""} [&_.node-overview-link]:block [&_.node-overview-link]:w-full [&_.node-overview-link]:p-0 [&_.node-overview-link]:border-0 [&_.node-overview-link]:bg-transparent [&_.node-overview-link]:text-inherit [&_.node-overview-link]:[font:inherit] [&_.node-overview-link]:text-start [&_.node-overview-link]:cursor-pointer [&_.node-overview-link:hover_.node-slug]:underline [&_.node-overview-link:hover_.node-slug]:underline-offset-[3px] [&_.node-overview-link:focus-visible]:outline-2 [&_.node-overview-link:focus-visible]:outline-solid [&_.node-overview-link:focus-visible]:outline-accent [&_.node-overview-link:focus-visible]:outline-offset-4 [&_.node-overview-link:focus-visible]:rounded-xs [&_.node-eyebrow]:flex [&_.node-eyebrow]:items-center [&_.node-eyebrow]:justify-between [&_.node-eyebrow]:gap-2 [&_.node-eyebrow]:mb-1.5 [&_.node-eyebrow]:text-xs [&_.node-eyebrow]:font-medium [&_.node-eyebrow]:text-muted [&_.node-head]:flex [&_.node-head]:items-center [&_.node-head]:gap-[7px] [&_.node-head]:min-w-0 [&_.node-status]:w-2 [&_.node-status]:h-2 [&_.node-status]:rounded-full [&_.node-status]:shrink-0 [&_.node-slug]:text-sm [&_.node-slug]:font-semibold [&_.node-slug]:text-text [&_.node-slug]:flex-1 [&_.node-slug]:min-w-0 [&_.node-slug]:overflow-hidden [&_.node-slug]:text-ellipsis [&_.node-slug]:whitespace-nowrap [&_.node-title]:mt-1 [&_.node-title]:text-text [&_.node-title]:text-sm [&_.node-title]:line-clamp-2 [&_.node-meta]:mt-2 [&_.node-meta]:flex [&_.node-meta]:items-center [&_.node-meta]:gap-2 [&_.node-meta]:text-xs [&_.node-meta]:text-muted [&_.node-actions]:mt-2 [&_.node-actions]:pt-1.5 [&_.node-actions]:border-t [&_.node-actions]:border-t-border-variant [&_.node-actions]:flex [&_.node-actions]:items-center [&_.node-actions]:gap-[3px] [&_.node-action]:inline-flex [&_.node-action]:items-center [&_.node-action]:gap-[5px] [&_.node-action]:py-[3px] [&_.node-action]:px-1.5 [&_.node-action]:text-sm [&_.node-action]:font-medium [&_.node-action]:text-text [&_.node-action]:rounded-sm [&_.node-action]:no-underline [&_.node-action:hover]:text-text [&_.node-action:hover]:bg-surface [&_.node-action-ext]:ms-auto [&_.node-action-ext]:py-[3px] [&_.node-action-ext]:px-[5px]`}
      onClick={onSelect}
      onMouseEnter={hover.onMouseEnter}
      onMouseLeave={hover.onMouseLeave}
    >
      <Handle type="target" position={Position.Top} />
      <div
        role="button"
        tabIndex={0}
        className="node-overview-link"
        onDoubleClick={() => onOpenView(exp.id, "overview", "keepOpen")}
        onAuxClick={(e) => {
          if (e.button === 1) {
            e.preventDefault();
            onOpenView(exp.id, "overview", "keepOpen");
          }
        }}
      >
        <div className="node-eyebrow">
          <span className="truncate">{exp.archived ? m.tree_archived() : kind}</span>
          <div className="flex items-center gap-1.5 shrink-0">
            {logsRun?.backend && (
              <span className="max-w-24 truncate [&_.backend-detail]:hidden [&_.backend-name]:text-xs [&_svg]:w-3.5 [&_svg]:h-3.5">
                <BackendBadge backend={logsRun.backend} />
              </span>
            )}
            <StatusBadge status={status ?? "idle"} />
          </div>
        </div>
        <div className="node-head">
          <span className="node-slug">{exp.slug}</span>
        </div>
        {exp.title && (
          <div className="node-title" title={exp.title}>{exp.title}</div>
        )}
        {exp.description && (
          <p className="mt-1 mb-0 text-xs leading-relaxed text-subtext line-clamp-2" title={exp.description}>
            {exp.description}
          </p>
        )}
        <div className="node-meta">
          <span>{m.tree_view_runs()}</span>
          {squares.length > 0 ? (
            <span className="run-squares flex items-center gap-[3px]">
              {squares.map((run) => (
                <span
                  key={run.id}
                  className={`run-sq w-[9px] h-[9px] shrink-0 [&.pass]:bg-accent-green [&.fail]:border-[1.5px] [&.fail]:border-danger-outline [&.live]:bg-accent-teal [&.live]:animate-[or-pulse_1.2s_ease-in-out_infinite] [&.other]:border-[1.5px] [&.other]:border-border ${runSquareClass(runDisplayStatus(run))}`}
                  title={statusLabel(runDisplayStatus(run))}
               />
              ))}
            </span>
          ) : (
            <span>{m.tree_view_no_runs()}</span>
          )}
          {logsRun?.endedAt != null && (
            <span className="tabular-nums">· {fmtDuration(logsRun.endedAt - logsRun.createdAt)}</span>
          )}
          <span className="flex-1" />
          {latestRun && <span>{timeAgo(latestRun.createdAt)}</span>}
        </div>
      </div>
      {/* Direct view shortcuts — code always, logs once there's a run. */}
      <div className="node-actions nodrag" onClick={(e) => e.stopPropagation()}>
        {runs.length > 0 && (
          <button
            type="button"
            className="node-action cursor-pointer"
            title={m.tree_view_open_logs()}
            {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
              onOpenView(exp.id, "terminal", intent),
            )}
          >
            <Terminal size={13} />
            {m.tree_view_logs()}
          </button>
        )}
        <button
          type="button"
          className="node-action cursor-pointer"
          title={m.a11y_browse_code_on({ branch: ltr(exp.branchName) })}
          {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
            onOpenCode(exp.id, exp.branchName, "files", intent),
          )}
        >
          <FolderTree size={13} />
          {m.tree_view_code()}
        </button>
        {liveRun && onCancel && (
          <button
            type="button"
            className="node-action text-accent-red hover:bg-danger-subtle hover:text-accent-red disabled:opacity-60 cursor-pointer"
            title={cancelling ? m.experiments_stop_requested() : m.experiments_stop_run()}
            disabled={cancelling}
            onClick={async (e) => {
              e.stopPropagation();
              await onCancel(liveRun.id);
            }}
          >
            <CircleStop size={13} />
            {cancelling ? m.common_stopping() : m.common_stop()}
          </button>
        )}
        {/* Icon-only: labeled actions + the link overflow the card's fixed width. */}
        {githubOwner && githubRepo && <a
          className="node-action cursor-pointer"
          title={m.a11y_open_on_github({ name: ltr(exp.branchName) })}
          aria-label={m.a11y_open_on_github({ name: ltr(exp.branchName) })}
          href={githubBranchUrl(githubOwner, githubRepo, exp.branchName)}
          target="_blank"
          rel="noopener noreferrer"
          onClick={(e) => e.stopPropagation()}
        >
          <GitHubMark size={13} />
        </a>}
        <ArchiveMenu id={exp.id} name={exp.title || exp.slug} actions={actions} onArchive={onArchive} compact />
      </div>
      <Handle type="source" position={Position.Bottom} />
      {/* Node and card share one leave handler */}
      {hover.rect && (
        <ExpHoverCard
          exp={exp}
          runs={runs}
          latestRun={latestRun}
          liveRun={liveRun}
          parentSlug={parentSlug}
          anchor={hover.rect}
          onOpenLogs={runs.length > 0
            ? (intent) => onOpenView(exp.id, "terminal", intent)
            : undefined}
          onOpenCode={(intent) => onOpenCode(exp.id, exp.branchName, "files", intent)}
          onCancel={onCancel}
          cancelling={cancelling}
          onMouseEnter={hover.keepOpen}
          onMouseLeave={hover.onMouseLeave}
       />
      )}
    </div>
  );
});

const ArchivedNode = memo(function ArchivedNode({ data }: NodeProps<ArchivedFlowNode>) {
  useLocale();
  const { rootId, count, onRestoreRegion } = data;
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menu = usePopover(triggerRef);
  const [position, setPosition] = useState({ top: 0, left: 0 });
  const toggle = () => {
    if (!menu.open && triggerRef.current) {
      const rect = triggerRef.current.getBoundingClientRect();
      const below = window.innerHeight - rect.bottom >= 44;
      setPosition({
        top: below ? rect.bottom + 4 : Math.max(4, rect.top - 44),
        left: Math.max(4, Math.min(rect.left, window.innerWidth - 232)),
      });
    }
    menu.setOpen((open) => !open);
  };
  return (
    <div className="archived-node w-40 h-11 cursor-grab active:cursor-grabbing">
      <Handle type="target" position={Position.Top} />
      <button
        type="button"
        ref={triggerRef}
        className="nodrag w-full h-full flex items-center justify-center gap-2 px-3 border border-border rounded-full bg-background text-text text-sm font-medium whitespace-nowrap transition-[background-color,border-color] duration-120 ease-standard hover:bg-surface hover:border-text cursor-pointer"
        aria-haspopup="menu"
        aria-expanded={menu.open}
        onClick={toggle}
      >
        <Archive size={15} className="shrink-0 text-subtext" aria-hidden="true" />
        <span>{m.tree_archived_count({ count: fmtNumber(count) })}</span>
      </button>
      <Handle type="source" position={Position.Bottom} />
      {menu.open && createPortal(
        <div ref={menu.ref} className="option-menu fixed z-70 min-w-56 rounded-lg border border-border bg-background p-1.5 shadow-menu" style={position}>
          <MenuItem onClick={() => {
            menu.setOpen(false);
            onRestoreRegion(rootId);
          }}>
            {count === 1 ? m.tree_restore_only() : m.tree_restore_archived_count({ count: fmtNumber(count) })}
          </MenuItem>
        </div>, document.body,
      )}
    </div>
  );
});

const ElidedNode = memo(function ElidedNode({ data }: NodeProps<ElidedFlowNode>) {
  useLocale();
  const { count, onShowProjectScope } = data;
  return (
    <div
      className="elided-node w-37 h-11 flex items-center gap-2 py-1.5 px-2.5 border border-dashed border-border rounded-md bg-hover-faint text-muted text-sm font-medium text-start transition-[border-color,color] duration-120 ease-standard cursor-grab active:cursor-grabbing [&:hover]:border-text [&:hover]:text-text [&_.elided-node-label]:flex [&_.elided-node-label]:flex-col [&_.elided-node-label]:leading-[1.3] [&_.elided-node-sub]:text-muted"
      role="button"
      tabIndex={0}
      title={m.tree_view_switch_to_entire_project_to_see_all_experiments()}
      onClick={onShowProjectScope}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onShowProjectScope();
        }
      }}
    >
      <Handle type="target" position={Position.Top} />
      <Ellipsis size={14} />
      <span className="elided-node-label">
        {count === 1 ? m.tree_one_experiment() : m.tree_experiment_count({ count: fmtNumber(count) })}
        <span className="elided-node-sub">{m.tree_hidden_experiments()}</span>
      </span>
      <Handle type="source" position={Position.Bottom} />
    </div>
  );
});

const nodeTypes = { exp: ExpNode, archived: ArchivedNode, elided: ElidedNode };

const defaultEdgeOptions = {
  type: "default", // bezier
  style: { stroke: "var(--text)", strokeWidth: 1.5, opacity: 0.3 },
};
const elidedEdgeStyle = { ...defaultEdgeOptions.style, strokeDasharray: "4 4" };

/** Inner canvas component with access to ReactFlow context hooks */
function TreeCanvasBody({
  nodes,
  edges,
  matchingIds,
  currentMatchIndex,
  onMatchIndexChange,
  search,
  onSearchChange,
  statusFilter,
  onStatusFilterChange,
  onResetLayout,
  showMiniMap,
  onToggleMiniMap,
  cancelError,
  onDismissCancelError,
  onMoveStart,
  onMoveEnd,
  onNodesChange,
  onPaneClick,
  viewport,
  agentSessionId,
  children,
  autoLayout,
  onToggleAutoLayout,
  toolbarActions,
  emptyState,
}: {
  nodes: FlowNode[];
  edges: Edge[];
  matchingIds: string[];
  currentMatchIndex: number;
  onMatchIndexChange: (index: number) => void;
  search: string;
  onSearchChange: (value: string) => void;
  statusFilter: "all" | "active" | "failed";
  onStatusFilterChange: (filter: "all" | "active" | "failed") => void;
  onResetLayout: () => void;
  showMiniMap: boolean;
  onToggleMiniMap: () => void;
  cancelError: string | null;
  onDismissCancelError: () => void;
  onMoveStart: () => void;
  onMoveEnd: (event: unknown, nextViewport: Viewport) => void;
  onNodesChange: (changes: NodeChange<FlowNode>[]) => void;
  onPaneClick: () => void;
  viewport: Viewport | null;
  agentSessionId: string | null;
  children?: ReactNode;
  autoLayout: boolean;
  onToggleAutoLayout: () => void;
  toolbarActions?: ReactNode;
  emptyState?: ReactNode;
}) {
  const { setCenter, getNode, fitView } = useReactFlow();

  const navigateToMatch = useCallback(
    (index: number) => {
      if (matchingIds.length === 0) return;
      const targetId = matchingIds[index];
      onMatchIndexChange(index);
      const targetNode = getNode(targetId);
      if (targetNode) {
        setCenter(
          targetNode.position.x + NODE_W / 2,
          targetNode.position.y + NODE_H / 2,
          {
            duration: 300,
            zoom: 0.85,
          },
        );
      }
    },
    [matchingIds, onMatchIndexChange, getNode, setCenter],
  );

  const handleNextMatch = () => {
    if (matchingIds.length === 0) return;
    const next = (currentMatchIndex + 1) % matchingIds.length;
    navigateToMatch(next);
  };

  const handlePrevMatch = () => {
    if (matchingIds.length === 0) return;
    const prev =
      (currentMatchIndex - 1 + matchingIds.length) % matchingIds.length;
    navigateToMatch(prev);
  };

  const handleAutoArrange = () => {
    onResetLayout();
    requestAnimationFrame(
      () => void fitView({ duration: 300, padding: 0.25, maxZoom: 1 }),
    );
  };

  return (
    <>
      <TreeSearchToolbar
        search={search}
        onSearchChange={onSearchChange}
        statusFilter={statusFilter}
        onStatusFilterChange={onStatusFilterChange}
        matchCount={matchingIds.length}
        currentMatchIndex={currentMatchIndex}
        onNextMatch={handleNextMatch}
        onPrevMatch={handlePrevMatch}
        onArrange={handleAutoArrange}
        autoLayout={autoLayout}
        onToggleAutoLayout={() => {
          onToggleAutoLayout();
          if (!autoLayout) requestAnimationFrame(() => void fitView({ duration: 250, padding: 0.2, maxZoom: 1 }));
        }}
        hasNodes={nodes.length > 0}
        actions={toolbarActions}
      />
      <div className="tree-canvas relative min-h-0 flex-1">
        {emptyState ?? <ReactFlow
          className="[&_.react-flow\_\_node.react-flow\_\_node-exp.selectable]:cursor-default [&_.react-flow\_\_node.react-flow\_\_node-archived.selectable]:cursor-pointer [&_.react-flow\_\_node.react-flow\_\_node-elided.selectable]:cursor-pointer [&_.react-flow\_\_handle]:opacity-0 [&_.react-flow\_\_handle]:pointer-events-none [&_.react-flow\_\_attribution]:hidden!"
          key={agentSessionId ?? "project"}
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          defaultEdgeOptions={defaultEdgeOptions}
          nodesDraggable={true}
          nodesConnectable={false}
          nodesFocusable={false}
          onNodesChange={onNodesChange}
          onPaneClick={onPaneClick}
          onMoveStart={onMoveStart}
          onMoveEnd={onMoveEnd}
          minZoom={0.15}
          defaultViewport={viewport ?? undefined}
          fitView={viewport === null}
          fitViewOptions={{ padding: 0.25, maxZoom: 1 }}
        >
          <Background
            variant={BackgroundVariant.Dots}
            color="var(--dots-strong)"
            gap={28}
            size={1.6}
          />

          <TreeControls
            onResetLayout={handleAutoArrange}
            showMiniMap={showMiniMap}
            onToggleMiniMap={onToggleMiniMap}
          />

          {showMiniMap && (
            <MiniMap
              nodeColor={(node) => {
                if (node.type === "archived") return "var(--subtext)";
                if (node.type === "elided") return "var(--border)";
                const d = node.data as ExpNodeData;
                const status = d?.latestRun
                  ? runDisplayStatus(d.latestRun)
                  : "idle";
                if (status === "done") return "var(--accent-green)";
                if (status === "failed") return "var(--accent-red)";
                if (
                  status === "running" ||
                  status === "starting" ||
                  status === "cancelling"
                )
                  return "var(--accent-teal)";
                return "var(--border-variant)";
              }}
              nodeStrokeWidth={1}
              nodeBorderRadius={4}
              className="!bottom-14 !right-3 rounded-lg border border-border bg-background shadow-menu"
              pannable
              zoomable
            />
          )}

          {cancelError && (
            <Panel position="top-center" className="nodrag m-3">
              <div
                className="flex items-center gap-2 rounded-lg border border-danger/30 bg-background px-3 py-1.5 text-xs text-accent-red shadow-menu"
                role="alert"
              >
                <span>
                  {m.experiments_table_stop_failed()} {cancelError}
                </span>
                <button
                  type="button"
                  onClick={onDismissCancelError}
                  className="text-subtext hover:text-text cursor-pointer"
                  aria-label={m.file_viewer_dismiss()}
                >
                  <X size={13} />
                </button>
              </div>
            </Panel>
          )}
        </ReactFlow>}
        {children}
      </div>
    </>
  );
}

export function TreeView({
  experiments,
  archiveActions,
  showArchived,
  runs,
  project,
  onOpenView,
  onOpenCode,
  onArchive,
  agentSessionId,
  onShowProjectScope,
  onRestoreRegion,
  viewport,
  onViewportChange,
  onCancel,
  state,
  onStateChange,
  toolbarActions,
}: {
  experiments: Experiment[];
  archiveActions: Map<string, ArchiveActions>;
  showArchived: boolean;
  runs: Run[];
  /** Owning project — supplies owner/repo for the GitHub branch links. */
  project: Project;
  /** Open an experiment view as a right-pane tab (card shortcut buttons). */
  onOpenView: (id: string, view: ExperimentView, intent: TabOpenIntent, runId?: string) => void;
  /** Browse an experiment branch's code in the project-level Code tab. */
  onOpenCode: (
    experimentId: string,
    branch: string,
    view: CodeView,
    intent: TabOpenIntent,
  ) => void;
  onArchive: (id: string, direction: "ancestors" | "descendants" | "only", archived: boolean) => void;
  /** Current task scope: show only this chat session's experiments, eliding the rest.
   * Null = Entire project scope (the whole forest). */
  agentSessionId: string | null;
  /** Leave Current task scope from an elided pill. */
  onShowProjectScope: () => void;
  onRestoreRegion: (id: string) => void;
  /** Preserve the canvas transform while the experiments pane is unmounted. */
  viewport: Viewport | null;
  onViewportChange: (viewport: Viewport) => void;
  onCancel?: (runId: string) => Promise<void>;
  state: ExperimentTreeState;
  onStateChange: Dispatch<SetStateAction<ExperimentTreeState>>;
  toolbarActions?: ReactNode;
}) {
  const { selectedId, search, statusFilter, showMiniMap } = state;
  const autoLayout = state.autoLayout ?? Object.keys(state.positions).length === 0;
  const [nodeSizes, setNodeSizes] = useState<Record<string, { width: number; height: number }>>({});
  const updateState = useCallback(
    (patch: Partial<ExperimentTreeState>) => {
      onStateChange((current) => ({ ...current, ...patch }));
    },
    [onStateChange],
  );
  const setSelectedId = useCallback(
    (id: string | null) => updateState({ selectedId: id }),
    [updateState],
  );
  const [pendingCancellation, setPendingCancellation] = useState<
    ReadonlySet<string>
  >(new Set());
  const [cancelError, setCancelError] = useState<string | null>(null);

  // User-dragged custom positions: nodeId -> { x, y }
  const customPositions = useRef(new Map(Object.entries(autoLayout ? {} : state.positions)));

  // Node synchronization depends on this handler; a new function on every
  // render would rebuild the tree and trigger another node-state update.
  const handleCancel = useMemo(
    () =>
      onCancel
        ? async (runId: string) => {
            setCancelError(null);
            setPendingCancellation((current) => new Set(current).add(runId));
            try {
              await onCancel(runId);
            } catch (cause) {
              setPendingCancellation((current) => {
                const next = new Set(current);
                next.delete(runId);
                return next;
              });
              setCancelError(
                cause instanceof Error ? cause.message : String(cause),
              );
            }
          }
        : undefined,
    [onCancel],
  );

  const runsByExp = useMemo(() => {
    const map = new Map<string, Run[]>();
    for (const run of runs) {
      const list = map.get(run.experimentId);
      if (list) list.push(run);
      else map.set(run.experimentId, [run]);
    }
    for (const list of map.values())
      list.sort((a, b) => a.createdAt - b.createdAt);
    return map;
  }, [runs]);

  const slugById = useMemo(
    () => new Map(experiments.map((e) => [e.id, e.slug])),
    [experiments],
  );

  // Compute search matches
  const { matchingIds, matchingSet, isFilterActive } = useMemo(() => {
    const active = search.trim().length > 0 || statusFilter !== "all";
    if (!active) {
      return {
        matchingIds: [],
        matchingSet: new Set<string>(),
        isFilterActive: false,
      };
    }
    const terms = search.trim().toLowerCase().split(/\s+/).filter(Boolean);
    const ids: string[] = [];
    const set = new Set<string>();

    for (const exp of experiments) {
      const expRuns = runsByExp.get(exp.id) ?? [];
      const live = expRuns.some(
        (r) => r.status === "running" || r.status === "starting",
      );
      const failed =
        !live &&
        expRuns.length > 0 &&
        expRuns[expRuns.length - 1].status === "failed";
      if (statusFilter === "active" && !live) continue;
      if (statusFilter === "failed" && !failed) continue;
      if (terms.length > 0) {
        const text = [
          exp.slug,
          exp.title ?? "",
          exp.description ?? "",
          exp.branchName,
        ]
          .join(" ")
          .toLowerCase();
        if (!terms.every((term) => text.includes(term))) continue;
      }
      ids.push(exp.id);
      set.add(exp.id);
    }
    return { matchingIds: ids, matchingSet: set, isFilterActive: true };
  }, [experiments, runsByExp, search, statusFilter]);

  const currentMatchIndex = Math.min(
    state.matchIndex,
    Math.max(0, matchingIds.length - 1),
  );

  // Compute lineage (ancestors back to baseline + direct children) of selected experiment
  const selectedLineage = useMemo(() => {
    if (!selectedId)
      return { nodeIds: new Set<string>(), edgeIds: new Set<string>() };
    const nodeIds = new Set<string>([selectedId]);
    const edgeIds = new Set<string>();
    const byId = new Map(experiments.map((e) => [e.id, e]));

    let curr = byId.get(selectedId);
    while (curr && curr.parentExperimentId) {
      const parentId = curr.parentExperimentId;
      nodeIds.add(parentId);
      edgeIds.add(`e-${parentId}-${curr.id}`);
      curr = byId.get(parentId);
    }

    for (const exp of experiments) {
      if (exp.parentExperimentId === selectedId) {
        nodeIds.add(exp.id);
        edgeIds.add(`e-${selectedId}-${exp.id}`);
      }
    }

    return { nodeIds, edgeIds };
  }, [selectedId, experiments]);

  const forest = useMemo(() => {
    const isMine = (n: TreeNode) => !agentSessionId || n.exp.chatSessionId === agentSessionId;
    const scopedRoots = elideHiddenRegions(buildForest(experiments), isMine);
    return showArchived ? scopedRoots : collapseArchivedRegions(scopedRoots);
  }, [experiments, agentSessionId, showArchived]);

  const layoutPositions = useMemo(() => {
    const sized = (node: DisplayNode): TreeLayoutNode => ({
      id: nodeId(node),
      width: nodeSizes[nodeId(node)]?.width ?? nodeWidth(node),
      height: nodeSizes[nodeId(node)]?.height ?? nodeHeight(node),
      children: node.children.map(sized),
    });
    return compactTreeLayout(forest.map(sized));
  }, [forest, nodeSizes]);

  const computedTree = useMemo(() => {
    const nodes: FlowNode[] = [];
    const edges: Edge[] = [];

    function visit(node: DisplayNode) {
      const position = layoutPositions.get(nodeId(node))!;
      const measured = nodeSizes[nodeId(node)];
      if (node.kind === "exp") {
        const expRuns = runsByExp.get(node.exp.id) ?? [];
        const latestRun = expRuns[expRuns.length - 1] ?? null;
        const liveRun =
          expRuns.find(
            (r) => r.status === "running" || r.status === "starting",
          ) ?? null;
        const cancelling = Boolean(
          liveRun &&
          (liveRun.cancelRequested || pendingCancellation.has(liveRun.id)),
        );
        nodes.push({
          id: node.exp.id,
          type: "exp",
          position,
          measured,
          data: {
            exp: node.exp,
            latestRun,
            liveRun,
            runs: expRuns,
            isBaseline: !node.exp.parentExperimentId,
            parentSlug: node.exp.parentExperimentId
              ? (slugById.get(node.exp.parentExperimentId) ?? null)
              : null,
            githubOwner: project.githubEnabled ? project.githubOwner : "",
            githubRepo: project.githubEnabled ? project.githubRepo : "",
            onOpenView,
            onOpenCode,
            onArchive,
            actions: archiveActions.get(node.exp.id)!,
            onCancel: handleCancel,
            cancelling,
          },
        });
      } else if (node.kind === "archived") {
        nodes.push({
          id: node.id,
          type: "archived",
          position,
          measured,
          data: { rootId: node.rootId, count: node.count, onRestoreRegion },
        });
      } else {
        nodes.push({
          id: node.id,
          type: "elided",
          position,
          measured,
          data: { count: node.count, onShowProjectScope },
        });
      }
      for (const child of node.children) {
        const elided =
          node.kind !== "exp" ||
          child.kind !== "exp" ||
          node.exp.archived ||
          child.exp.archived;
        edges.push({
          id: `e-${nodeId(node)}-${nodeId(child)}`,
          source: nodeId(node),
          target: nodeId(child),
          ...(elided ? { style: elidedEdgeStyle } : {}),
        });
        visit(child);
      }
    }

    forest.forEach(visit);
    return { nodes, edges };
  }, [
    forest,
    layoutPositions,
    nodeSizes,
    archiveActions,
    runsByExp,
    onOpenView,
    onOpenCode,
    onArchive,
    project.githubOwner,
    project.githubRepo,
    project.githubEnabled,
    onShowProjectScope,
    onRestoreRegion,
    handleCancel,
    pendingCancellation,
    slugById,
  ]);

  // Local node positions state to support dragging
  const [nodes, setNodes] = useState<FlowNode[]>(() =>
    computedTree.nodes.map((node) => {
      const position = customPositions.current.get(node.id);
      return position ? { ...node, position } : node;
    }),
  );
  const nodesRef = useRef(nodes);
  nodesRef.current = nodes;
  const capturePositions = useCallback(() => {
    const positions = new Map(customPositions.current);
    for (const node of nodesRef.current) {
      positions.set(node.id, { ...(customPositions.current.get(node.id) ?? node.position) });
    }
    return Object.fromEntries(positions);
  }, []);

  useEffect(() => {
    customPositions.current = new Map(Object.entries(autoLayout ? {} : state.positions));
  }, [state.positions, autoLayout]);

  // Data refreshes during a drag must preserve the in-progress coordinates.
  useEffect(() => {
    setNodes(
      computedTree.nodes.map((node) => {
        const customPos = customPositions.current.get(node.id);
        return customPos ? { ...node, position: customPos } : node;
      }),
    );
  }, [computedTree.nodes, state.positions, autoLayout]);

  const onNodesChange = useCallback(
    (changes: NodeChange<FlowNode>[]) => {
      setNodes((nds) => applyNodeChanges(changes, nds));
      setNodeSizes((current) => {
        let next = current;
        for (const change of changes) {
          if (change.type !== "dimensions" || !change.dimensions) continue;
          const { width, height } = change.dimensions;
          if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) continue;
          if (current[change.id]?.width === width && current[change.id]?.height === height) continue;
          if (next === current) next = { ...current };
          next[change.id] = { width, height };
        }
        return next;
      });
      for (const change of changes) {
        if (change.type === "position" && change.position) {
          customPositions.current.set(change.id, change.position);
        }
      }
      // Keep dragging local and save the final coordinates when the gesture ends.
      if (
        changes.some(
          (change) =>
            change.type === "position" && change.position && !change.dragging,
        )
      ) {
        updateState({ positions: capturePositions(), autoLayout: false });
      }
    },
    [updateState, capturePositions],
  );

  const handleResetLayout = useCallback(() => {
    customPositions.current.clear();
    setNodes(computedTree.nodes);
    updateState({ positions: {}, autoLayout: true });
  }, [computedTree.nodes, updateState]);

  const handleToggleAutoLayout = useCallback(() => {
    if (autoLayout) updateState({ positions: capturePositions(), autoLayout: false });
    else handleResetLayout();
  }, [autoLayout, capturePositions, handleResetLayout, updateState]);

  // Decorate nodes with interactive selection, ancestor lineage, and search dimming
  const displayNodes = useMemo(() => {
    return nodes.map((node) => {
      if (node.type !== "exp") return node;
      const isSelected = node.id === selectedId;
      const isAncestor = selectedLineage.nodeIds.has(node.id) && !isSelected;
      const isDimmed = isFilterActive && !matchingSet.has(node.id);
      return {
        ...node,
        data: {
          ...node.data,
          isSelected,
          isAncestor,
          isDimmed,
          onSelect: () => setSelectedId(node.id),
        },
      };
    });
  }, [
    nodes,
    selectedId,
    selectedLineage.nodeIds,
    isFilterActive,
    matchingSet,
    setSelectedId,
  ]);

  // Highlight lineage edges when an experiment is selected
  const displayEdges = useMemo(() => {
    if (!selectedId) return computedTree.edges;
    return computedTree.edges.map((edge) => {
      const isLineage = selectedLineage.edgeIds.has(edge.id);
      if (!isLineage) {
        return {
          ...edge,
          style: { ...(edge.style ?? defaultEdgeOptions.style), opacity: 0.12 },
        };
      }
      return {
        ...edge,
        animated: true,
        style: {
          ...edge.style,
          stroke: "var(--accent-teal)",
          strokeWidth: 2.5,
          opacity: 1,
        },
      };
    });
  }, [computedTree.edges, selectedId, selectedLineage.edgeIds]);

  if (experiments.length === 0) {
    return (
      <div className={EMPTY_STATE_CLASS_NAME}>
        <p className="empty-state-title">{m.tree_view_no_experiments_yet()}</p>
        <p className="empty-state-hint">
          {m.tree_view_ask_the_agent_in_chat_to_create_and()}
        </p>
      </div>
    );
  }

  // Only Current task scope can filter a non-empty forest down to nothing.
  if (computedTree.nodes.length === 0 && agentSessionId) {
    return (
      <div className={EMPTY_STATE_CLASS_NAME}>
        <p className="empty-state-title">
          {m.tree_view_no_experiments_from_the_current_task_yet()}
        </p>
        <p className="empty-state-hint">
          {m.tree_view_ask_in_this_task_to_create_one_or()}
        </p>
      </div>
    );
  }

  const selectedExp = selectedId
    ? (experiments.find((e) => e.id === selectedId) ?? null)
    : null;
  const selectedExpRuns = selectedExp
    ? (runsByExp.get(selectedExp.id) ?? [])
    : [];

  return (
    <ReactFlowProvider key={agentSessionId ?? "project"}>
      <div className="tree-workspace relative flex h-full min-h-0 w-full flex-col overflow-hidden bg-background">
        <TreeCanvasBody
          nodes={displayNodes}
          edges={displayEdges}
          matchingIds={matchingIds}
          currentMatchIndex={currentMatchIndex}
          onMatchIndexChange={(matchIndex) => updateState({ matchIndex })}
          search={search}
          onSearchChange={(search) => updateState({ search, matchIndex: 0 })}
          statusFilter={statusFilter}
          onStatusFilterChange={(statusFilter) =>
            updateState({ statusFilter, matchIndex: 0 })
          }
          onResetLayout={handleResetLayout}
          showMiniMap={showMiniMap}
          onToggleMiniMap={() =>
            onStateChange((current) => ({
              ...current,
              showMiniMap: !current.showMiniMap,
            }))
          }
          cancelError={cancelError}
          onDismissCancelError={() => setCancelError(null)}
          onMoveStart={dismissTreeHoverCards}
          onMoveEnd={(_, nextViewport) => onViewportChange(nextViewport)}
          onNodesChange={onNodesChange}
          onPaneClick={() => setSelectedId(null)}
          viewport={viewport}
          agentSessionId={agentSessionId}
          autoLayout={autoLayout}
          onToggleAutoLayout={handleToggleAutoLayout}
          toolbarActions={toolbarActions}
        >
          {/* Slide-over Inspector Panel */}
          {selectedExp && (
            <TreeInspector
              experiment={selectedExp}
              project={project}
              runs={selectedExpRuns}
              parentSlug={
                selectedExp.parentExperimentId
                  ? (slugById.get(selectedExp.parentExperimentId) ?? null)
                  : null
              }
              onClose={() => setSelectedId(null)}
              onOpenView={onOpenView}
              onOpenCode={onOpenCode}
              onCancel={handleCancel}
              onSelectParent={(parentId) => setSelectedId(parentId)}
              tab={state.inspectorTab}
              onTabChange={(inspectorTab) => updateState({ inspectorTab })}
            />
          )}
        </TreeCanvasBody>
      </div>
    </ReactFlowProvider>
  );
}
