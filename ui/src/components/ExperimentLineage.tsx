import { ChevronRight } from "lucide-react";
import { useEffect, useState } from "react";
import { timeAgo, fmtNumber } from "../api";
import { getLocale } from "../paraglide/runtime.js";
import { m } from "../paraglide/messages.js";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";
import { ancestorsOf, laneCount, shortName, type HistoryNode, type HistoryRow, type HistoryStatus } from "./experimentLineageModel";

export type HistoryOrder = "lineage" | "oldest" | "newest";

const LANE = 14;
const RAIL_PAD = 15;
const laneX = (lane: number) => RAIL_PAD + LANE / 2 + lane * LANE;

const STATUS_WORD: Record<HistoryStatus, () => string> = {
  done: m.status_done,
  failed: m.status_failed,
  cancelled: m.status_cancelled,
  lost: m.status_lost,
  running: m.status_running,
  none: m.history_status_not_run,
};
const STATUS_TONE: Record<HistoryStatus, string> = {
  done: "var(--accent-green)",
  failed: "var(--accent-red)",
  cancelled: "var(--muted)",
  lost: "var(--muted)",
  running: "var(--accent-teal)",
  none: "var(--muted)",
};
// Only runtime theme variables: an @theme colour that no class uses is dropped from the build.
const INK = "var(--lineage)";
const LINE_OPACITY = 0.48;
const DOT_OPACITY = 1;

/** Status as a shape, so colour is never the only signal: ● done, ◆ failed, ⊘ cancelled, ◐ running, ○ not run. */
function StatusShape({ status, cx, cy, r, color, background = "var(--base)" }: {
  status: HistoryStatus;
  cx: number;
  cy: number;
  r: number;
  color: string;
  background?: string;
}) {
  const stroke = Math.max(1.25, r * 0.38);
  const inner = r - stroke / 2;
  if (status === "done") return <circle cx={cx} cy={cy} r={r} fill={color} />;
  if (status === "failed") {
    const d = r * 1.18;
    return <path d={`M${cx} ${cy - d}L${cx + d} ${cy}L${cx} ${cy + d}L${cx - d} ${cy}Z`} fill={color} />;
  }
  const ring = <circle cx={cx} cy={cy} r={inner} fill={background} stroke={color} strokeWidth={stroke} />;
  if (status === "running")
    return (
      <g>
        {ring}
        <path d={`M${cx} ${cy - inner}A${inner} ${inner} 0 0 1 ${cx} ${cy + inner}Z`} fill={color} />
      </g>
    );
  if (status === "cancelled" || status === "lost") {
    const s = (r - stroke) * 0.7;
    return (
      <g>
        {ring}
        <line x1={cx - s} y1={cy + s} x2={cx + s} y2={cy - s} stroke={color} strokeWidth={stroke} />
      </g>
    );
  }
  return ring;
}

function StatusLabel({ status, word = true, size = 12 }: { status: HistoryStatus; word?: boolean; size?: number }) {
  return (
    <span className="inline-flex items-center gap-1.5 whitespace-nowrap text-sm text-text">
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} className="shrink-0" aria-hidden="true">
        <StatusShape status={status} cx={size / 2} cy={size / 2} r={size * 0.33} color={STATUS_TONE[status]} />
      </svg>
      {word ? STATUS_WORD[status]() : <span className="sr-only">{STATUS_WORD[status]()}</span>}
    </span>
  );
}

function stamp(ms: number): string {
  return new Intl.DateTimeFormat(getLocale(), { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" }).format(ms);
}
function When({ ms }: { ms: number }) {
  return (
    <span className="whitespace-nowrap text-xs tabular-nums text-subtext" title={stamp(ms)}>
      {timeAgo(ms)}
    </span>
  );
}

export interface Placed {
  row: HistoryRow;
  top: number;
  height: number;
  y: number;
}
function edgePath(parent: Placed, child: Placed): string {
  const from = laneX(parent.row.lane);
  const to = laneX(child.row.lane);
  if (from === to) return `M${from} ${parent.y + 6}V${child.y - 6}`;
  const bend = child.top - 7;
  return `M${from} ${parent.y + 6}V${bend}A7 7 0 0 0 ${from + 7} ${bend + 7}H${to - 7}A7 7 0 0 1 ${to} ${bend + 14}V${child.y - 6}`;
}
function rowHead(row: HistoryRow): HistoryNode {
  return row.kind === "collapsed" ? row.head : row.node;
}

export function Rail({ placed, height, path, selectedId }: {
  placed: Placed[];
  height: number;
  path: ReadonlySet<string>;
  selectedId: string | null;
}) {
  const byId = new Map<string, Placed>();
  for (const item of placed) if (item.row.kind !== "lead-in") byId.set(rowHead(item.row).id, item);
  const edges: { d: string; hot: boolean; dotted: boolean }[] = [];
  for (const item of placed) {
    if (item.row.kind === "lead-in") {
      const target = byId.get(item.row.node.id);
      if (target)
        edges.push({
          d: `M${laneX(0)} ${item.top + 6}V${target.y - 6}`,
          hot: path.has(item.row.node.id) && path.has(item.row.from.id),
          dotted: true,
        });
      continue;
    }
    const node = rowHead(item.row);
    const parent = node.parent ? byId.get(node.parent.id) : undefined;
    if (parent) edges.push({ d: edgePath(parent, item), hot: path.has(node.id) && path.has(node.parent!.id), dotted: false });
  }
  const width = RAIL_PAD + LANE * laneCount(placed.map((item) => item.row)) + 8;
  const line = (edge: (typeof edges)[number], key: number) => (
    <path
      key={key}
      d={edge.d}
      fill="none"
      stroke={edge.hot ? "var(--primary)" : INK}
      strokeOpacity={edge.hot ? 1 : LINE_OPACITY}
      strokeWidth={edge.hot ? 2 : 1.5}
      strokeLinecap="round"
      strokeDasharray={edge.dotted ? "1.5 3.5" : undefined}
    />
  );
  return (
    <svg className="pointer-events-none absolute left-0 top-0 z-10" width={width} height={height} aria-hidden="true">
      {edges.filter((edge) => !edge.hot).map(line)}
      {edges.filter((edge) => edge.hot).map(line)}
      {placed.map((item, index) => {
        if (item.row.kind === "lead-in") return null;
        const cx = laneX(item.row.lane);
        if (item.row.kind === "collapsed")
          return (
            <g key={index}>
              <circle cx={cx} cy={item.y + 2.5} r={3.5} fill="var(--base)" stroke={INK} strokeOpacity={DOT_OPACITY} strokeWidth={1.25} />
              <circle cx={cx} cy={item.y - 1.5} r={3.5} fill="var(--base)" stroke={INK} strokeOpacity={DOT_OPACITY} strokeWidth={1.25} />
            </g>
          );
        const node = item.row.node;
        const selected = node.id === selectedId;
        const hot = path.has(node.id);
        const color = hot ? "var(--primary)" : INK;
        return (
          <g key={index} opacity={hot ? 1 : DOT_OPACITY}>
            {!node.parent && !node.missingParentId && <circle cx={cx} cy={item.y} r={7} fill="none" stroke={color} strokeWidth={1} opacity={0.7} />}
            {selected && <circle cx={cx} cy={item.y} r={8.5} fill="var(--primary)" fillOpacity={0.18} />}
            <StatusShape status={node.status} cx={cx} cy={item.y} r={selected ? 4.6 : 4} color={color} background={selected ? "var(--highlight)" : "var(--base)"} />
          </g>
        );
      })}
    </svg>
  );
}

function LineageStep({ node, current, onSelect }: { node: HistoryNode; current?: boolean; onSelect: (id: string, intent: TabOpenIntent) => void }) {
  return (
    <button
      type="button"
      disabled={current}
      aria-current={current ? "true" : undefined}
      {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => onSelect(node.id, intent))}
      className={`flex min-w-0 items-center gap-2 rounded-sm px-1.5 py-1 text-start ${current ? "bg-highlight" : "hover:bg-hover-faint"}`}
    >
      <StatusLabel status={node.status} word={false} size={11} />
      {node.code && <span className="w-7 shrink-0 font-mono text-xs text-subtext">{node.code}</span>}
      <span className={`min-w-0 flex-1 truncate text-sm text-text ${current ? "font-medium" : ""}`}>{node.label}</span>
      <When ms={node.createdAt} />
    </button>
  );
}

export function ExperimentLineage({ node, onSelect }: { node: HistoryNode; onSelect: (id: string, intent: TabOpenIntent) => void }) {
  const [showAllAncestors, setShowAllAncestors] = useState(false);
  useEffect(() => setShowAllAncestors(false), [node.id]);
  const ancestors = ancestorsOf(node);
  const folded = ancestors.slice(0, Math.max(0, ancestors.length - 4));
  const visibleAncestors = showAllAncestors ? ancestors : ancestors.slice(-4);
  return (
    <section className="flex flex-col gap-2">
      <h2 className="m-0 text-sm font-semibold text-text">{m.history_lineage()}</h2>
      <div className="flex flex-col">
        {ancestors.length > 0 && <div className="mb-1 text-sm text-subtext">{m.history_ancestors()}</div>}
        {folded.length > 0 && (
          <button
            type="button"
            aria-expanded={showAllAncestors}
            onClick={() => setShowAllAncestors((show) => !show)}
            className="flex min-w-0 items-center gap-1.5 px-1.5 py-1 text-start text-xs text-subtext hover:text-text"
            title={folded.map((step) => step.label).join(" → ")}
          >
            <ChevronRight size={11} className={`shrink-0 ${showAllAncestors ? "rotate-90" : ""}`} />
            <span className="truncate">
              {showAllAncestors ? m.common_show_less() : m.history_earlier_steps({ count: fmtNumber(folded.length), steps: folded.map((step) => shortName(step, 18)).join(" → ") })}
            </span>
          </button>
        )}
        {visibleAncestors.map((step) => (
          <LineageStep key={step.id} node={step} onSelect={onSelect} />
        ))}
        <LineageStep node={node} current onSelect={onSelect} />
        {node.children.length > 0 && (
          <div className="mt-3 flex flex-col gap-1 pt-3">
            <div className="text-sm text-subtext">{m.history_children()}</div>
            {node.children.map((child) => <LineageStep key={child.id} node={child} onSelect={onSelect} />)}
          </div>
        )}
      </div>
    </section>
  );
}
