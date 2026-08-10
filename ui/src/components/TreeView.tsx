import {
  Background,
  BackgroundVariant,
  Handle,
  Position,
  ReactFlow,
  type Edge,
  type Node,
  type NodeProps,
} from "@xyflow/react";
import { Ellipsis, FolderTree, Terminal } from "lucide-react";
import { GitHubMark } from "./BackendLogos";
import { memo, useMemo, useRef } from "react";
import {
  githubBranchUrl,
  runDisplayStatus,
  timeAgo,
  type Experiment,
  type Project,
  type Run,
} from "../api";
import type { ExperimentView } from "./DetailDrawer";
import type { CodeView } from "./CodeTab";
import { ExpHoverCard, dismissTreeHoverCards, useHoverIntent } from "./ExpHoverCard";
import { StatusBadge } from "./StatusBadge";

const EMPTY_STATE_CLASS_NAME = [
  "empty-state [position:absolute] [inset:0] [display:flex] [flex-direction:column] [align-items:center]",
  "[justify-content:center] [padding:24px] [text-align:center] [color:var(--subtext)] [&_p]:[max-width:46ch]",
  "[&_p]:[margin:0] [&_p]:[font-size:var(--fs-md)] [&_p]:[line-height:1.5] [&_p]:[text-wrap:balance]",
  "[&_p.empty-state-title]:[font-size:var(--fs-2xl)] [&_p.empty-state-title]:[font-weight:var(--fw-regular)]",
  "[&_p.empty-state-title]:[color:var(--text)] [&_p.empty-state-hint]:[font-size:var(--fs-lg)]",
  "[&_p.empty-state-hint]:[color:var(--subtext)] empty-state-cta [gap:6px]",
].join(" ");

const NODE_W = 264;
const NODE_H = 132;
const GAP_X = 44;
const GAP_Y = 72;
const MAX_SQUARES = 8;
// Keep in sync with the inline `.elided-node` classes below.
const ELIDED_W = 148;
const ELIDED_H = 44;

type ExpNodeData = {
  exp: Experiment;
  latestRun: Run | null;
  runs: Run[]; // oldest → newest
  isBaseline: boolean;
  parentSlug: string | null;
  githubOwner: string;
  githubRepo: string;
  onOpenView: (id: string, view: ExperimentView) => void;
  onOpenCode: (experimentId: string, branch: string, view: CodeView) => void;
};
type ExpFlowNode = Node<ExpNodeData, "exp">;

type ElidedNodeData = {
  count: number;
  onShowProjectScope: () => void;
};
type ElidedFlowNode = Node<ElidedNodeData, "elided">;
type FlowNode = ExpFlowNode | ElidedFlowNode;

interface TreeNode {
  exp: Experiment;
  children: TreeNode[];
}

/** What the layout actually draws: real experiments plus "…" placeholders
 * standing in for experiments from other tasks (Current task scope only). */
type DisplayNode =
  | { kind: "exp"; exp: Experiment; children: DisplayNode[] }
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
 * nowhere. An always-true predicate (Project scope) reproduces the forest
 * as-is. Elided ids key off the region root, so they're stable across
 * renders. */
function elideForeignRegions(roots: TreeNode[], mine: (n: TreeNode) => boolean): DisplayNode[] {
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

function nodeWidth(node: DisplayNode): number {
  return node.kind === "exp" ? NODE_W : ELIDED_W;
}

function nodeId(node: DisplayNode): string {
  return node.kind === "exp" ? node.exp.id : node.id;
}

function subtreeWidth(node: DisplayNode): number {
  if (node.children.length === 0) return nodeWidth(node);
  const cw =
    node.children.reduce((s, c) => s + subtreeWidth(c), 0) + GAP_X * (node.children.length - 1);
  return Math.max(nodeWidth(node), cw);
}

function runSquareClass(status: string): string {
  if (status === "done") return "pass";
  if (status === "failed") return "fail";
  if (status === "running" || status === "starting" || status === "cancelling") return "live";
  return "other";
}

const ExpNode = memo(function ExpNode({ data }: NodeProps<ExpFlowNode>) {
  const { exp, latestRun, runs, isBaseline, parentSlug, githubOwner, githubRepo, onOpenView, onOpenCode } = data;
  const status = latestRun ? runDisplayStatus(latestRun) : undefined;
  const live = status === "running" || status === "starting" || status === "cancelling";
  const kind = isBaseline ? "Baseline" : live ? "Running" : "Experiment";
  const squares = runs.slice(-MAX_SQUARES);

  // `data` is rebuilt on every experiments/runs change (a superset of the
  // re-layouts that matter), so it doubles as the hover card's re-measure
  // key. Canvas pan/zoom dismissal arrives via dismissTreeHoverCards, wired
  // to the ReactFlow onMoveStart prop below.
  const rootRef = useRef<HTMLDivElement>(null);
  const hover = useHoverIntent(rootRef, data);

  return (
    <div
      ref={rootRef}
      className={`exp-node [width:264px] [border:1px_solid_var(--border)] [border-radius:var(--radius-md)] [background:var(--base)] [padding:10px_12px] [box-shadow:0_1px_2px_rgba(0,_0,_0,_0.04)] [font-size:var(--fs-md)] [transition:box-shadow_120ms_ease] [&:hover]:[box-shadow:0_2px_8px_rgba(0,_0,_0,_0.08)] [&.live]:[border-color:var(--accent-teal)] [&.live]:[box-shadow:0_2px_12px_rgba(32,_154,_132,_0.2)] [&_.node-overview-link]:[display:block] [&_.node-overview-link]:[width:100%] [&_.node-overview-link]:[padding:0] [&_.node-overview-link]:[border:0] [&_.node-overview-link]:[background:transparent] [&_.node-overview-link]:[color:inherit] [&_.node-overview-link]:[font:inherit] [&_.node-overview-link]:[text-align:left] [&_.node-overview-link]:[cursor:pointer] [&_.node-overview-link:hover_.node-slug]:[text-decoration:underline] [&_.node-overview-link:hover_.node-slug]:[text-underline-offset:3px] [&_.node-overview-link:focus-visible]:[outline:2px_solid_var(--accent)] [&_.node-overview-link:focus-visible]:[outline-offset:4px] [&_.node-overview-link:focus-visible]:[border-radius:var(--radius-xs)] [&_.node-eyebrow]:[display:flex] [&_.node-eyebrow]:[align-items:center] [&_.node-eyebrow]:[justify-content:space-between] [&_.node-eyebrow]:[gap:8px] [&_.node-eyebrow]:[margin-bottom:6px] [&_.node-eyebrow]:[font-size:var(--fs-2xs)] [&_.node-eyebrow]:[font-weight:var(--fw-medium)] [&_.node-eyebrow]:[color:var(--muted)] [&_.node-head]:[display:flex] [&_.node-head]:[align-items:center] [&_.node-head]:[gap:7px] [&_.node-head]:[min-width:0] [&_.node-status]:[width:8px] [&_.node-status]:[height:8px] [&_.node-status]:[border-radius:50%] [&_.node-status]:[flex-shrink:0] [&_.node-slug]:[font-family:var(--mono)] [&_.node-slug]:[font-size:var(--fs-sm)] [&_.node-slug]:[font-weight:var(--fw-semibold)] [&_.node-slug]:[color:var(--text)] [&_.node-slug]:[flex:1] [&_.node-slug]:[min-width:0] [&_.node-slug]:[overflow:hidden] [&_.node-slug]:[text-overflow:ellipsis] [&_.node-slug]:[white-space:nowrap] [&_.node-title]:[margin-top:4px] [&_.node-title]:[color:var(--text)] [&_.node-title]:[font-size:var(--fs-sm)] [&_.node-title]:[display:-webkit-box] [&_.node-title]:[-webkit-line-clamp:2] [&_.node-title]:[-webkit-box-orient:vertical] [&_.node-title]:[overflow:hidden] [&_.node-meta]:[margin-top:8px] [&_.node-meta]:[display:flex] [&_.node-meta]:[align-items:center] [&_.node-meta]:[gap:8px] [&_.node-meta]:[font-size:var(--fs-2xs)] [&_.node-meta]:[color:var(--muted)] [&_.node-actions]:[margin-top:8px] [&_.node-actions]:[padding-top:6px] [&_.node-actions]:[border-top:1px_solid_var(--border-variant)] [&_.node-actions]:[display:flex] [&_.node-actions]:[align-items:center] [&_.node-actions]:[gap:3px] [&_.node-action]:[display:inline-flex] [&_.node-action]:[align-items:center] [&_.node-action]:[gap:5px] [&_.node-action]:[padding:3px_6px] [&_.node-action]:[font-size:var(--fs-xs)] [&_.node-action]:[font-weight:var(--fw-medium)] [&_.node-action]:[color:var(--text)] [&_.node-action]:[border-radius:var(--radius-sm)] [&_.node-action]:[text-decoration:none] [&_.node-action:hover]:[color:var(--text)] [&_.node-action:hover]:[background:var(--surface)] [&_.node-action-ext]:[margin-left:auto] [&_.node-action-ext]:[padding:3px_5px] ${live ? "live" : ""}`}
      onMouseEnter={hover.onMouseEnter}
      onMouseLeave={hover.onMouseLeave}
    >
      <Handle type="target" position={Position.Top} />
      <div
        role="button"
        tabIndex={0}
        className="node-overview-link nodrag"
        onClick={() => onOpenView(exp.id, "overview")}
        onKeyDown={(event) => {
          if (event.key !== "Enter" && event.key !== " ") return;
          event.preventDefault();
          onOpenView(exp.id, "overview");
        }}
      >
        <div className="node-eyebrow">
          <span>{kind}</span>
          <StatusBadge status={status ?? "idle"} />
        </div>
        <div className="node-head">
          <span className="node-slug">{exp.slug}</span>
        </div>
        {(exp.title || exp.description) && (
          <div className="node-title">{exp.title || exp.description}</div>
        )}
        <div className="node-meta">
          <span>Runs</span>
          {squares.length > 0 ? (
            <span className="run-squares [display:flex] [align-items:center] [gap:3px]">
              {squares.map((run) => (
                <span
                  key={run.id}
                  className={`run-sq [width:9px] [height:9px] [flex-shrink:0] [&.pass]:[background:var(--accent-green)] [&.fail]:[border:1.5px_solid_color-mix(in_oklab,_var(--accent-red)_55%,_transparent)] [&.live]:[background:var(--accent-teal)] [&.live]:[animation:or-pulse_1.2s_ease-in-out_infinite] [&.other]:[border:1.5px_solid_var(--border)] ${runSquareClass(runDisplayStatus(run))}`}
                  title={runDisplayStatus(run)}
                />
              ))}
            </span>
          ) : (
            <span>no runs</span>
          )}
          <span style={{ flex: 1 }} />
          {latestRun && <span>{timeAgo(latestRun.createdAt)}</span>}
        </div>
      </div>
      {/* Direct view shortcuts — code always, logs once there's a run. */}
      <div className="node-actions" onClick={(e) => e.stopPropagation()}>
        {runs.length > 0 && (
          <button
            className="node-action"
            title="Open logs"
            onClick={() => onOpenView(exp.id, "terminal")}
          >
            <Terminal size={13} />
            Logs
          </button>
        )}
        <button
          className="node-action"
          title={`Browse code on ${exp.branchName}`}
          onClick={() => onOpenCode(exp.id, exp.branchName, "files")}
        >
          <FolderTree size={13} />
          Code
        </button>
        {/* Icon-only: labeled actions + the link overflow the card's fixed width. */}
        {githubOwner && githubRepo && <a
          className="node-action node-action-ext"
          title={`Open ${exp.branchName} on GitHub`}
          aria-label={`Open ${exp.branchName} on GitHub`}
          href={githubBranchUrl(githubOwner, githubRepo, exp.branchName)}
          target="_blank"
          rel="noopener noreferrer"
          onClick={(e) => e.stopPropagation()}
        >
          <GitHubMark size={13} />
        </a>}
      </div>
      <Handle type="source" position={Position.Bottom} />
      {/* Node and card share one leave handler — React's enter/leave pairing
        * across the portal (fiber-tree walk) relies on it; don't split them. */}
      {hover.rect && (
        <ExpHoverCard
          exp={exp}
          runs={runs}
          latestRun={latestRun}
          parentSlug={parentSlug}
          anchor={hover.rect}
          onOpenLogs={runs.length > 0 ? () => onOpenView(exp.id, "terminal") : undefined}
          onOpenCode={() => onOpenCode(exp.id, exp.branchName, "files")}
          onMouseEnter={hover.keepOpen}
          onMouseLeave={hover.onMouseLeave}
        />
      )}
    </div>
  );
});

const ElidedNode = memo(function ElidedNode({ data }: NodeProps<ElidedFlowNode>) {
  const { count, onShowProjectScope } = data;
  // A div, not a <button>: ReactFlow's <Handle> renders divs, which are
  // invalid inside button elements. tabIndex opts the pill back into the tab
  // order that nodesFocusable={false} removes — it's the only node whose whole
  // body is a single action.
  return (
    <div
      className="elided-node [width:148px] [height:44px] [display:flex] [align-items:center] [gap:8px] [padding:6px_10px] [border:1px_dashed_var(--border)] [border-radius:var(--radius-md)] [background:color-mix(in_oklab,_var(--text)_3%,_transparent)] [color:var(--muted)] [font-size:var(--fs-2xs)] [font-weight:var(--fw-medium)] [text-align:left] [transition:border-color_120ms_ease,_color_120ms_ease] [&:hover]:[border-color:var(--text)] [&:hover]:[color:var(--text)] [&_.elided-node-label]:[display:flex] [&_.elided-node-label]:[flex-direction:column] [&_.elided-node-label]:[line-height:1.3] [&_.elided-node-sub]:[color:var(--muted)]"
      role="button"
      tabIndex={0}
      title="Switch to Entire project to see all experiments"
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
        {count} {count === 1 ? "experiment" : "experiments"}
        <span className="elided-node-sub">other tasks</span>
      </span>
      <Handle type="source" position={Position.Bottom} />
    </div>
  );
});

const nodeTypes = { exp: ExpNode, elided: ElidedNode };

const defaultEdgeOptions = {
  type: "default", // bezier
  style: { stroke: "var(--text)", strokeWidth: 1.5, opacity: 0.3 },
};
// A per-edge `style` replaces defaultEdgeOptions.style wholesale, so the
// dashed variant re-carries the base stroke.
const elidedEdgeStyle = { ...defaultEdgeOptions.style, strokeDasharray: "4 4" };

export function TreeView({
  experiments,
  runs,
  project,
  onOpenView,
  onOpenCode,
  agentSessionId,
  onShowProjectScope,
}: {
  experiments: Experiment[];
  runs: Run[];
  /** Owning project — supplies owner/repo for the GitHub branch links. */
  project: Project;
  /** Open an experiment view as a right-pane tab (card shortcut buttons). */
  onOpenView: (id: string, view: ExperimentView) => void;
  /** Browse an experiment branch's code in the project-level Code tab. */
  onOpenCode: (experimentId: string, branch: string, view: CodeView) => void;
  /** Current task scope: show only this chat session's experiments, eliding the rest.
   * Null = Entire project scope (the whole forest). */
  agentSessionId: string | null;
  /** Leave Current task scope (clicking an elided "…" pill). */
  onShowProjectScope: () => void;
}) {
  const { nodes, edges } = useMemo(() => {
    const runsByExp = new Map<string, Run[]>();
    for (const run of runs) {
      const list = runsByExp.get(run.experimentId);
      if (list) list.push(run);
      else runsByExp.set(run.experimentId, [run]);
    }
    for (const list of runsByExp.values()) list.sort((a, b) => a.createdAt - b.createdAt);

    const nodes: FlowNode[] = [];
    const edges: Edge[] = [];
    // Project scope (null session) accepts every node, so the elision pass is
    // an identity transform and produces no pills.
    const isMine = (n: TreeNode) =>
      !agentSessionId || n.exp.chatSessionId === agentSessionId;
    const roots = elideForeignRegions(buildForest(experiments), isMine);
    const slugById = new Map(experiments.map((e) => [e.id, e.slug]));

    function layout(node: DisplayNode, cx: number, y: number) {
      const x = cx - nodeWidth(node) / 2;
      if (node.kind === "exp") {
        const expRuns = runsByExp.get(node.exp.id) ?? [];
        nodes.push({
          id: node.exp.id,
          type: "exp",
          position: { x, y },
          data: {
            exp: node.exp,
            latestRun: expRuns[expRuns.length - 1] ?? null,
            runs: expRuns,
            isBaseline: !node.exp.parentExperimentId,
            parentSlug: node.exp.parentExperimentId
              ? (slugById.get(node.exp.parentExperimentId) ?? null)
              : null,
            githubOwner: project.githubEnabled ? project.githubOwner : "",
            githubRepo: project.githubEnabled ? project.githubRepo : "",
            onOpenView,
            onOpenCode,
          },
        });
      } else {
        // Pills are shorter than cards; center them within the row.
        nodes.push({
          id: node.id,
          type: "elided",
          position: { x, y: y + (NODE_H - ELIDED_H) / 2 },
          data: { count: node.count, onShowProjectScope },
        });
      }
      if (node.children.length === 0) return;
      const totalW =
        node.children.reduce((s, c) => s + subtreeWidth(c), 0) +
        GAP_X * (node.children.length - 1);
      let childX = cx - totalW / 2;
      for (const child of node.children) {
        const cw = subtreeWidth(child);
        const elided = node.kind === "elided" || child.kind === "elided";
        edges.push({
          id: `e-${nodeId(node)}-${nodeId(child)}`,
          source: nodeId(node),
          target: nodeId(child),
          ...(elided ? { style: elidedEdgeStyle } : {}),
        });
        layout(child, childX + cw / 2, y + NODE_H + GAP_Y);
        childX += cw + GAP_X;
      }
    }

    let rx = 0;
    for (const root of roots) {
      const w = subtreeWidth(root);
      layout(root, rx + w / 2, 0);
      rx += w + GAP_X;
    }
    return { nodes, edges };
  }, [
    experiments,
    runs,
    onOpenView,
    onOpenCode,
    project.githubOwner,
    project.githubRepo,
    project.githubEnabled,
    agentSessionId,
    onShowProjectScope,
  ]);

  if (experiments.length === 0) {
    return (
      <div className={EMPTY_STATE_CLASS_NAME}>
        <p className="empty-state-title">No experiments yet</p>
        <p className="empty-state-hint">Ask the agent in chat to create and run your first experiment.</p>
      </div>
    );
  }

  // Only Current task scope can filter a non-empty forest down to nothing.
  if (nodes.length === 0 && agentSessionId) {
    return (
      <div className={EMPTY_STATE_CLASS_NAME}>
        <p className="empty-state-title">No experiments from the current task yet</p>
        <p className="empty-state-hint">
          Ask in this task to create one, or switch to Entire project to see all experiments.
        </p>
      </div>
    );
  }

  return (
    <ReactFlow
      className="[&_.react-flow\_\_node.react-flow\_\_node-exp.selectable]:[cursor:default] [&_.react-flow\_\_node.react-flow\_\_node-elided.selectable]:[cursor:pointer] [&_.react-flow\_\_handle]:[opacity:0] [&_.react-flow\_\_handle]:[pointer-events:none] [&_.react-flow\_\_attribution]:[display:none]!"
      // fitView only runs on mount, so remount when the scope changes to re-fit.
      key={agentSessionId ?? "project"}
      nodes={nodes}
      edges={edges}
      nodeTypes={nodeTypes}
      defaultEdgeOptions={defaultEdgeOptions}
      nodesDraggable={false}
      nodesConnectable={false}
      nodesFocusable={false}
      onMoveStart={dismissTreeHoverCards}
      minZoom={0.15}
      fitView
      fitViewOptions={{ padding: 0.25, maxZoom: 1 }}
    >
      <Background variant={BackgroundVariant.Dots} color="var(--dots-strong)" gap={28} size={1.6} />
    </ReactFlow>
  );
}
