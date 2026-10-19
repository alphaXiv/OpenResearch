import {
  backendDetail,
  backendKind,
  runDisplayStatus,
  type Experiment,
  type Run,
} from "../api";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";

/** Outcome of an experiment's latest attempt; "none" when it has never run. */
export type HistoryStatus = "done" | "failed" | "cancelled" | "running" | "none";

export interface HistoryNode {
  id: string;
  experiment: Experiment;
  parent: HistoryNode | null;
  /** Recorded parent id that is not among the experiments, or null. */
  missingParentId: string | null;
  children: HistoryNode[];
  /** Attempts, oldest first. */
  runs: Run[];
  latestRun: Run | null;
  status: HistoryStatus;
  /** Short experiment code parsed from the title ("D2", "A"), or null. */
  code: string | null;
  /** Title without its code prefix. */
  label: string;
  /** Chat session (agent task) that created the experiment, if any. */
  task: string | null;
  createdAt: number;
}

export type HistoryGrouping = "task" | "lineage" | "none";

export interface HistoryGroup {
  key: string;
  title: string;
  /** Members, oldest first. */
  nodes: HistoryNode[];
}

export type HistoryRow =
  | { kind: "experiment"; node: HistoryNode; lane: number }
  | { kind: "collapsed"; head: HistoryNode; lane: number; hidden: HistoryNode[] }
  | { kind: "lead-in"; node: HistoryNode; from: HistoryNode; lane: number };

/** Chapter key for experiments that no agent task created. */
export const NO_TASK = "__no_task";

export function historyStatus(run: Run): HistoryStatus {
  const status = runDisplayStatus(run);
  if (status === "done" || status === "failed" || status === "cancelled") return status;
  return "running";
}

const CODE_PREFIX = /^([A-Z][0-9]{0,2}[a-z]?)(?:\s*[:–—-]\s*|\s+)(?=\S)/;

/** Split "D2: re-score C2 at higher n" into its code and a sentence-case label. */
export function splitExperimentTitle(experiment: Experiment): {
  code: string | null;
  label: string;
} {
  const raw = (experiment.title || experiment.description || experiment.slug)
    .split(/\r?\n/)[0]
    .trim();
  const match = raw.match(CODE_PREFIX);
  const rest = match ? raw.slice(match[0].length) : raw;
  return {
    code: match ? match[1] : null,
    label: rest.charAt(0).toUpperCase() + rest.slice(1),
  };
}

export function byCreated(a: HistoryNode, b: HistoryNode): number {
  return a.createdAt - b.createdAt || a.id.localeCompare(b.id);
}

export function buildHistoryNodes(experiments: Experiment[], runs: Run[]): HistoryNode[] {
  const runsByExperiment = new Map<string, Run[]>();
  for (const run of runs) {
    const grouped = runsByExperiment.get(run.experimentId);
    if (grouped) grouped.push(run);
    else runsByExperiment.set(run.experimentId, [run]);
  }
  const nodes = experiments.map((experiment): HistoryNode => {
    const attempts = (runsByExperiment.get(experiment.id) ?? [])
      .slice()
      .sort((a, b) => a.createdAt - b.createdAt || a.id.localeCompare(b.id));
    const latestRun = attempts.at(-1) ?? null;
    return {
      id: experiment.id,
      experiment,
      parent: null,
      missingParentId: null,
      children: [],
      runs: attempts,
      latestRun,
      status: latestRun ? historyStatus(latestRun) : "none",
      ...splitExperimentTitle(experiment),
      task: experiment.chatSessionId ?? null,
      createdAt: experiment.createdAt,
    };
  });
  const byId = new Map(nodes.map((node) => [node.id, node]));
  for (const node of nodes) {
    const parentId = node.experiment.parentExperimentId;
    const parent = parentId ? byId.get(parentId) : undefined;
    if (parent) {
      node.parent = parent;
      parent.children.push(node);
    } else if (parentId) node.missingParentId = parentId;
  }
  for (const node of nodes) node.children.sort(byCreated);
  return nodes;
}

export function ancestorsOf(node: HistoryNode): HistoryNode[] {
  const chain: HistoryNode[] = [];
  for (let current = node.parent; current; current = current.parent) chain.unshift(current);
  return chain;
}

export function subtreeOf(node: HistoryNode): HistoryNode[] {
  return [node, ...node.children.flatMap(subtreeOf)];
}

/** "D2", or the label cut to `max` characters when the title has no code. */
export function shortName(node: HistoryNode, max = 22): string {
  if (node.code) return node.code;
  return node.label.length > max ? node.label.slice(0, max - 1).trimEnd() + "…" : node.label;
}

/** Where an experiment came from, as the row and the detail pane say it. */
export function originOf(node: HistoryNode, max = 22): string {
  if (node.parent) return m.history_origin_from({ name: shortName(node.parent, max) });
  return node.missingParentId ? m.history_origin_missing() : m.history_origin_start();
}

/**
 * What "Diff from …" compares against: the parent, or the project's baseline
 * branch for a starting point. Null when the recorded parent is missing, since
 * the backend cannot diff against it.
 */
export function diffBaseOf(node: HistoryNode, baselineBranch: string): string | null {
  if (node.parent) return shortName(node.parent);
  return node.missingParentId ? null : baselineBranch;
}

function rootOf(node: HistoryNode): HistoryNode {
  let root = node;
  while (root.parent) root = root.parent;
  return root;
}

export function groupKeyOf(node: HistoryNode, grouping: HistoryGrouping): string {
  if (grouping === "none") return "all";
  if (grouping === "task") return node.task ?? NO_TASK;
  return rootOf(node).id;
}

/**
 * Chapters, oldest first, with experiments outside any task last. Task titles
 * come from the caller; a task without one (for example a deleted chat) is
 * "Untitled task".
 */
export function groupHistory(
  nodes: HistoryNode[],
  grouping: HistoryGrouping,
  taskTitle: (task: string) => string | undefined,
): HistoryGroup[] {
  if (grouping === "none") return nodes.length ? [{ key: "all", title: "", nodes: [...nodes].sort(byCreated) }] : [];
  const buckets = new Map<string, HistoryNode[]>();
  for (const node of nodes) {
    const key = groupKeyOf(node, grouping);
    const bucket = buckets.get(key);
    if (bucket) bucket.push(node);
    else buckets.set(key, [node]);
  }
  const groups = [...buckets.entries()].map(([key, members]): HistoryGroup => {
    members.sort(byCreated);
    let title: string;
    if (grouping === "task") title = key === NO_TASK ? m.history_not_in_task() : taskTitle(key) || m.history_untitled_task();
    else {
      const root = members.find((member) => member.id === key) ?? members[0];
      title = (root.code ? root.code + " · " : "") + root.label;
    }
    return { key, title, nodes: members };
  });
  return groups.sort((a, b) => {
    if (a.key === NO_TASK) return 1;
    if (b.key === NO_TASK) return -1;
    return a.nodes[0].createdAt - b.nodes[0].createdAt;
  });
}

/**
 * Lay a chapter out in lineage order, Git-graph style.
 *
 * At each fork the side branches are listed first and the continuing child
 * last, so the parent's line runs straight past the side branches to it. The
 * continuing child is the one needing the most lanes, then the largest, then
 * the newest. A side branch takes its parent's lane + 1 and frees it when it
 * ends, so a long chain never staircases and siblings reuse one lane.
 *
 * With `collapseFinished`, a side branch of four or more experiments that all
 * finished successfully, and holds nothing in `keep`, becomes one summary row
 * unless its head is in `expanded`. An experiment whose parent sits in another
 * chapter is preceded by a lead-in row naming that parent.
 */
export function layoutLineage(
  group: HistoryGroup,
  options: {
    collapseFinished?: boolean;
    expanded?: ReadonlySet<string>;
    keep?: ReadonlySet<string>;
  } = {},
): HistoryRow[] {
  const members = new Set(group.nodes.map((node) => node.id));
  const kids = (node: HistoryNode) => node.children.filter((child) => members.has(child.id));
  const sizes = new Map<string, number>();
  const lanes = new Map<string, number>();
  const size = (node: HistoryNode): number => {
    const cached = sizes.get(node.id);
    if (cached !== undefined) return cached;
    const value = 1 + kids(node).reduce((sum, child) => sum + size(child), 0);
    sizes.set(node.id, value);
    return value;
  };
  const continuing = (children: HistoryNode[]) =>
    [...children].sort(
      (a, b) => lanesNeeded(b) - lanesNeeded(a) || size(b) - size(a) || byCreated(b, a),
    )[0];
  const lanesNeeded = (node: HistoryNode): number => {
    const cached = lanes.get(node.id);
    if (cached !== undefined) return cached;
    const children = kids(node);
    let value = 1;
    if (children.length) {
      const next = continuing(children);
      const sides = children.filter((child) => child !== next);
      value = Math.max(lanesNeeded(next), 1 + Math.max(0, ...sides.map(lanesNeeded)));
    }
    lanes.set(node.id, value);
    return value;
  };

  // A descendant reached through a non-member starts its own root, so it is not part of this branch.
  const branch = (node: HistoryNode): HistoryNode[] => [node, ...kids(node).flatMap(branch)];

  const rows: HistoryRow[] = [];
  const visit = (node: HistoryNode, lane: number, side: boolean) => {
    if (side && options.collapseFinished && !options.expanded?.has(node.id)) {
      const hidden = branch(node);
      if (
        hidden.length >= 4 &&
        hidden.every((member) => member.status === "done") &&
        !hidden.some((member) => options.keep?.has(member.id))
      ) {
        rows.push({ kind: "collapsed", head: node, lane, hidden });
        return;
      }
    }
    rows.push({ kind: "experiment", node, lane });
    const children = kids(node);
    if (!children.length) return;
    const next = continuing(children);
    for (const child of children) if (child !== next) visit(child, lane + 1, true);
    visit(next, lane, false);
  };

  const roots = group.nodes.filter((node) => !node.parent || !members.has(node.parent.id));
  for (const root of roots.sort(byCreated)) {
    if (root.parent) rows.push({ kind: "lead-in", node: root, from: root.parent, lane: 0 });
    visit(root, 0, false);
  }
  return rows;
}

/** Chronological rows; the graph is not drawn in this order. */
export function layoutChronological(group: HistoryGroup, newestFirst = false): HistoryRow[] {
  const ordered = [...group.nodes].sort(byCreated);
  if (newestFirst) ordered.reverse();
  return ordered.map((node) => ({ kind: "experiment", node, lane: 0 }));
}

export function laneCount(rows: HistoryRow[]): number {
  return rows.reduce((max, row) => Math.max(max, row.lane + 1), 0);
}

/** Width of the detail pane when it sits beside the list. */
export const PANE_W = 380;
/** Below this width the detail pane covers the list instead of sitting beside it. */
const OVERLAY_BELOW = 980;
/** Below this list width the "What changed" column is dropped. */
const TITLE_COLUMN_FROM = 760;
/** Below this list width (the default side panel) only the experiment and its status fit. */
const COMPACT_BELOW = 520;

/** What fits in a History view `width` pixels wide. */
export function historyFit(width: number, detailOpen: boolean, lineageOrder: boolean) {
  const overlay = width < OVERLAY_BELOW;
  const listWidth = detailOpen && !overlay ? width - PANE_W : width;
  const compact = listWidth < COMPACT_BELOW;
  return {
    overlay,
    /** The open detail pane covers the list, which leaves the tab order until it closes. */
    listCovered: detailOpen && overlay,
    whatChanged: listWidth >= TITLE_COLUMN_FROM,
    /** Lineage order draws the parent on the rail instead. */
    from: !lineageOrder && !compact,
    /** The Attempts and Latest columns. */
    runs: !compact,
    /** Status counts and dates beside a chapter's title. */
    chapterSummary: !compact,
  };
}

export function compactHistoryId(id: string, length = 8): string {
  return id.length > length ? id.slice(0, length) : id;
}

export function historyBackendLabel(run: Run): string {
  const kind = backendKind(run.backend);
  if (!kind) return "—";
  return [kind, backendDetail(run.backend)].filter(Boolean).join(" · ");
}

export function historyJobId(run: Run): string {
  const jobId = run.backend?.jobId;
  return typeof jobId === "string" || typeof jobId === "number"
    ? String(jobId)
    : "—";
}

export function describeAttemptChange(
  run: Run,
  previousRun: Run | null,
): string {
  if (!previousRun) return m.history_attempt_initial();
  if (!run.commitSha || !previousRun.commitSha) return m.history_attempt_no_provenance();
  if (run.commitSha === previousRun.commitSha) return m.history_attempt_retry();
  return m.history_attempt_new_commit({ sha: ltr(compactHistoryId(run.commitSha, 7)) });
}
