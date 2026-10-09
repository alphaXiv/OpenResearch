import { useQuery } from "@tanstack/react-query";
import { ChevronRight, Copy, GitBranch, Search, Terminal, X } from "lucide-react";
import {
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { timeAgo, type Experiment, type Project, type Run } from "../api";
import { fmtNumber, ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import { listChatSessionsQuery } from "../queries/chat";
import { Md } from "./Md";
import {
  ancestorsOf,
  buildHistoryNodes,
  compactHistoryId,
  describeAttemptChange,
  diffBaseOf,
  groupHistory,
  groupKeyOf,
  historyBackendLabel,
  historyFit,
  historyJobId,
  historyStatus,
  laneCount,
  layoutChronological,
  layoutLineage,
  originOf,
  PANE_W,
  shortName,
  subtreeOf,
  type HistoryGroup,
  type HistoryGrouping,
  type HistoryNode,
  type HistoryRow,
  type HistoryStatus,
} from "./experimentHistoryModel";

type HistoryOrder = "lineage" | "oldest" | "newest";

// Rail geometry: a fixed 14 px lane pitch, 40 px rows, forks bend across the
// boundary above the child row. Lane 0 lines up with the chapter chevrons.
const ROW_H = 40;
const LEAD_IN_H = 22;
const LANE = 14;
const RAIL_PAD = 15;
const laneX = (lane: number) => RAIL_PAD + LANE / 2 + lane * LANE;

const STATUS_WORD: Record<HistoryStatus, () => string> = {
  done: m.status_done,
  failed: m.status_failed,
  cancelled: m.status_cancelled,
  running: m.status_running,
  none: m.history_status_not_run,
};
/** "3 done", for experiments in a chapter and attempts of one experiment alike. */
const STATUS_COUNT: Record<HistoryStatus, (inputs: { count: string }) => string> = {
  done: m.history_count_done,
  failed: m.history_count_failed,
  cancelled: m.history_count_cancelled,
  running: m.history_count_running,
  none: m.history_count_not_run,
};
const STATUS_TONE: Record<HistoryStatus, string> = {
  done: "var(--accent-green)",
  failed: "var(--accent-red)",
  cancelled: "var(--muted)",
  running: "var(--accent-teal)",
  none: "var(--muted)",
};
// Only runtime theme variables: an @theme colour that no class uses is dropped from the build.
const INK = "var(--text)";
const LINE_OPACITY = 0.38;
const DOT_OPACITY = 0.55;

// ---- preferences ----------------------------------------------------------------------
const PREFS_KEY = "orx:history-view";
interface HistoryPrefs {
  grouping: HistoryGrouping;
  order: HistoryOrder;
}
function readPrefs(): HistoryPrefs {
  const fallback: HistoryPrefs = { grouping: "task", order: "lineage" };
  try {
    const saved = JSON.parse(localStorage.getItem(PREFS_KEY) ?? "{}") as Partial<HistoryPrefs>;
    return {
      grouping: saved.grouping === "lineage" || saved.grouping === "none" ? saved.grouping : fallback.grouping,
      order: saved.order === "oldest" || saved.order === "newest" ? saved.order : fallback.order,
    };
  } catch {
    return fallback;
  }
}
function usePrefs(): [HistoryPrefs, (patch: Partial<HistoryPrefs>) => void] {
  const [prefs, setPrefs] = useState(readPrefs);
  const update = (patch: Partial<HistoryPrefs>) =>
    setPrefs((current) => {
      const next = { ...current, ...patch };
      try {
        localStorage.setItem(PREFS_KEY, JSON.stringify(next));
      } catch {
        // storage unavailable: keep the choice for this session only
      }
      return next;
    });
  return [prefs, update];
}

/** Width of an element, measured again whenever the element itself is replaced. */
function useWidth<T extends HTMLElement>() {
  const [element, setElement] = useState<T | null>(null);
  const [width, setWidth] = useState(1200);
  useLayoutEffect(() => {
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => setWidth(entry.contentRect.width));
    observer.observe(element);
    return () => observer.disconnect();
  }, [element]);
  return [setElement, width] as const;
}

function toggled(set: ReadonlySet<string>, key: string): Set<string> {
  const next = new Set(set);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  return next;
}

// ---- small pieces ---------------------------------------------------------------------
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
  if (status === "cancelled") {
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

/** Attempts oldest to newest: filled = done, outline = failed, outline with slash = cancelled, pulsing = running. */
function AttemptMarks({ runs, max = 5 }: { runs: Run[]; max?: number }) {
  if (!runs.length) return <span className="text-xs text-muted">–</span>;
  const shown = runs.slice(-max);
  const earlier = runs.length - shown.length;
  const counts = new Map<HistoryStatus, number>();
  for (const run of runs) counts.set(historyStatus(run), (counts.get(historyStatus(run)) ?? 0) + 1);
  const label = [...counts].map(([status, count]) => STATUS_COUNT[status]({ count: fmtNumber(count) })).join(", ");
  return (
    <span className="inline-flex items-center gap-1" role="img" aria-label={m.history_attempts_label({ count: fmtNumber(runs.length), summary: label })}>
      {earlier > 0 && <span className="text-xs tabular-nums text-subtext">+{earlier}</span>}
      {shown.map((run) => {
        const status = historyStatus(run);
        const color = STATUS_TONE[status];
        return (
          <svg key={run.id} width={8} height={8} viewBox="0 0 8 8" aria-hidden="true" className={status === "running" ? "animate-pulse" : undefined}>
            {status === "done" || status === "running" ? (
              <rect width={8} height={8} rx={1} fill={color} />
            ) : (
              <rect x={0.75} y={0.75} width={6.5} height={6.5} rx={1} fill="none" stroke={color} strokeWidth={1.5} />
            )}
            {status === "cancelled" && <line x1={1.5} y1={6.5} x2={6.5} y2={1.5} stroke={color} strokeWidth={1.3} />}
          </svg>
        );
      })}
    </span>
  );
}

function stamp(ms: number): string {
  return new Intl.DateTimeFormat(getLocale(), { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" }).format(ms);
}
function day(ms: number): string {
  return new Intl.DateTimeFormat(getLocale(), { day: "numeric", month: "short" }).format(ms);
}
function When({ ms }: { ms: number }) {
  return (
    <span className="whitespace-nowrap text-xs tabular-nums text-subtext" title={stamp(ms)}>
      {timeAgo(ms)}
    </span>
  );
}

function Segmented<T extends string>({ label, value, options, onChange }: {
  label: string;
  value: T;
  options: [T, string][];
  onChange: (value: T) => void;
}) {
  return (
    <span className="inline-flex items-center gap-2">
      <span className="text-xs font-semibold uppercase tracking-wider text-muted">{label}</span>
      <span className="inline-flex overflow-hidden rounded-sm border border-border-variant" role="group" aria-label={label}>
        {options.map(([option, text]) => (
          <button
            key={option}
            type="button"
            aria-pressed={value === option}
            onClick={() => onChange(option)}
            className={`px-2 py-0.5 text-xs ${value === option ? "bg-panel font-medium text-text" : "text-subtext hover:text-text"}`}
          >
            {text}
          </button>
        ))}
      </span>
    </span>
  );
}

// ---- rail -----------------------------------------------------------------------------
interface Placed {
  row: HistoryRow;
  top: number;
  height: number;
  y: number;
}
function place(rows: HistoryRow[]): Placed[] {
  let top = 0;
  return rows.map((row) => {
    const height = row.kind === "lead-in" ? LEAD_IN_H : ROW_H;
    const placed = { row, top, height, y: top + height / 2 };
    top += height;
    return placed;
  });
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

function Rail({ placed, height, path, selectedId }: {
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

// ---- chapter --------------------------------------------------------------------------
export function ChapterHeader({ group, open, showSummary, onToggle }: { group: HistoryGroup; open: boolean; showSummary: boolean; onToggle: () => void }) {
  const count = (status: HistoryStatus) => group.nodes.filter((node) => node.status === status).length;
  const summary = (["done", "failed", "cancelled", "running", "none"] as const)
    .map((status) => [count(status), status] as const)
    .filter(([n]) => n)
    .map(([n, status]) => STATUS_COUNT[status]({ count: fmtNumber(n) }))
    .join(" · ");
  const first = day(group.nodes[0].createdAt);
  const last = day(group.nodes.at(-1)!.createdAt);
  return (
    <button
      type="button"
      aria-expanded={open}
      onClick={onToggle}
      className="flex h-9 w-full items-center gap-2 border-t border-divider-faint px-4 pt-1.5 text-start"
    >
      <ChevronRight size={12} className={`shrink-0 text-muted transition-transform ${open ? "rotate-90" : ""}`} />
      <span className="truncate text-xs font-semibold uppercase tracking-wider text-subtext">{group.title}</span>
      <span className="text-xs font-medium tabular-nums text-muted">{group.nodes.length}</span>
      {showSummary && (
        <span className="ms-auto whitespace-nowrap text-xs text-subtext">
          {summary}
          <span className="text-muted"> · {first === last ? first : `${first} – ${last}`}</span>
        </span>
      )}
    </button>
  );
}

// ---- detail pane ----------------------------------------------------------------------
function Section({ label, children }: { label: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-2">
      <h3 className="m-0 text-xs font-semibold uppercase tracking-wider text-subtext">{label}</h3>
      {children}
    </section>
  );
}

function LineageStep({ node, current, onSelect }: { node: HistoryNode; current?: boolean; onSelect: (id: string) => void }) {
  return (
    <button
      type="button"
      disabled={current}
      aria-current={current ? "true" : undefined}
      onClick={() => onSelect(node.id)}
      className={`flex min-w-0 items-center gap-2 rounded-sm px-1.5 py-1 text-start ${current ? "bg-highlight" : "hover:bg-hover-faint"}`}
    >
      <StatusLabel status={node.status} word={false} size={11} />
      <span className="w-7 shrink-0 font-mono text-xs text-subtext">{node.code ?? ""}</span>
      <span className={`min-w-0 flex-1 truncate text-sm text-text ${current ? "font-medium" : ""}`}>{node.label}</span>
      <When ms={node.createdAt} />
    </button>
  );
}

export function ExperimentDetail({ node, baselineBranch, overlay, taskTitle, onSelect, onClose, onOpenChanges, onOpenRun }: {
  node: HistoryNode;
  baselineBranch: string;
  overlay: boolean;
  taskTitle: (task: string | null) => string;
  onSelect: (id: string) => void;
  onClose: () => void;
  onOpenChanges: (experimentId: string) => void;
  onOpenRun: (run: Run) => void;
}) {
  const [showFullChange, setShowFullChange] = useState(false);
  const [showAllAncestors, setShowAllAncestors] = useState(false);
  const [showAllAttempts, setShowAllAttempts] = useState(false);
  const changeId = "history-change-" + useId().replace(/:/g, "");
  const asideRef = useRef<HTMLElement>(null);
  useEffect(() => {
    setShowFullChange(false);
    setShowAllAncestors(false);
    setShowAllAttempts(false);
  }, [node.id]);
  // Covering the list takes focus with it, so keyboard focus never stays on a hidden row.
  useEffect(() => {
    if (overlay) asideRef.current?.focus({ preventScroll: true });
  }, [overlay, node.id]);

  const ancestors = ancestorsOf(node);
  const folded = showAllAncestors ? [] : ancestors.slice(0, Math.max(0, ancestors.length - 2));
  const visibleAncestors = showAllAncestors ? ancestors : ancestors.slice(-2);
  const description = (node.experiment.description ?? "").trim();
  const result = node.latestRun?.resultMarkdown?.trim() ?? "";
  const attempts = node.runs;
  const shownAttempts = showAllAttempts || attempts.length <= 4 ? attempts : attempts.slice(-3);
  const diffBase = diffBaseOf(node, baselineBranch);
  const button =
    "inline-flex h-7 items-center gap-1.5 rounded-sm border border-border bg-background px-2.5 text-sm font-medium text-text hover:bg-hover-faint";

  return (
    <aside
      ref={asideRef}
      tabIndex={overlay ? -1 : undefined}
      aria-label={m.history_details_for({ name: ltr(node.experiment.slug) })}
      onKeyDown={(event) => {
        if (event.key === "Escape") onClose();
      }}
      className={`flex min-h-0 flex-col border-s border-border-variant bg-background ${overlay ? "absolute inset-0 z-30" : "shrink-0"}`}
      style={overlay ? undefined : { width: PANE_W }}
    >
      {overlay && (
        <button type="button" onClick={onClose} className="flex items-center gap-1 px-4 pt-3 text-xs font-medium text-subtext hover:text-text">
          <ChevronRight size={12} className="rotate-180" /> {m.history_all_experiments()}
        </button>
      )}
      <header className="flex items-start gap-2 border-b border-divider-faint px-5 pb-3 pt-4">
        <div className="min-w-0 flex-1">
          <div className="flex items-start gap-2">
            <span className="pt-1">
              <StatusLabel status={node.status} word={false} size={14} />
            </span>
            <h2 className="m-0 text-base font-semibold leading-snug text-text">
              {node.code && <span className="me-1.5 font-mono text-sm font-medium text-subtext">{node.code}</span>}
              {node.label}
            </h2>
          </div>
          <div className="mt-1.5 flex min-w-0 items-center gap-1 font-mono text-xs text-subtext">
            <span className="truncate" title={node.experiment.branchName}>{node.experiment.branchName}</span>
            <button
              type="button"
              className="shrink-0 rounded-xs p-0.5 text-muted hover:text-text"
              aria-label={m.history_copy_branch_name()}
              onClick={() => void navigator.clipboard?.writeText(node.experiment.branchName)}
            >
              <Copy size={11} />
            </button>
          </div>
          <div className="mt-1 text-xs text-subtext">
            {node.parent ? (
              <button type="button" className="hover:text-text hover:underline" onClick={() => onSelect(node.parent!.id)}>
                {m.history_origin_from({ name: shortName(node.parent) })}
              </button>
            ) : (
              originOf(node)
            )}
            {` · ${m.history_started({ time: stamp(node.createdAt) })}`}
            {node.task && ` · ${taskTitle(node.task)}`}
          </div>
        </div>
        {!overlay && (
          <button type="button" className="rounded-sm p-1 text-muted hover:bg-hover-faint hover:text-text" aria-label={m.history_close_details()} onClick={onClose}>
            <X size={14} />
          </button>
        )}
      </header>
      <div className="flex min-h-0 flex-col gap-5 overflow-auto px-5 py-4">
        <Section label={m.history_how_it_got_here()}>
          <div className="flex flex-col">
            {folded.length > 0 && (
              <button
                type="button"
                onClick={() => setShowAllAncestors(true)}
                className="flex min-w-0 items-center gap-1.5 px-1.5 py-1 text-start text-xs text-subtext hover:text-text"
                title={folded.map((step) => step.label).join(" → ")}
              >
                <ChevronRight size={11} className="shrink-0" />
                <span className="truncate">
                  {m.history_earlier_steps({ count: fmtNumber(folded.length), steps: folded.map((step) => shortName(step, 18)).join(" → ") })}
                </span>
              </button>
            )}
            {visibleAncestors.map((step) => (
              <LineageStep key={step.id} node={step} onSelect={onSelect} />
            ))}
            <LineageStep node={node} current onSelect={onSelect} />
            {node.children.length > 0 && (
              <div className="mt-1 flex flex-wrap items-center gap-1 px-1.5 text-xs text-subtext">
                {m.history_led_to()}
                {node.children.map((child) => (
                  <button
                    key={child.id}
                    type="button"
                    onClick={() => onSelect(child.id)}
                    title={child.experiment.title ?? child.label}
                    className="rounded-full border border-border-variant px-1.5 py-px text-text hover:bg-hover-faint"
                  >
                    {shortName(child)}
                  </button>
                ))}
              </div>
            )}
          </div>
        </Section>
        <Section label={diffBase ? m.history_what_changed_from({ base: diffBase }) : m.history_what_changed()}>
          {description ? (
            <>
              <p id={changeId} className={`m-0 whitespace-pre-line text-sm leading-relaxed text-subtext ${showFullChange ? "" : "line-clamp-5"}`}>
                {description}
              </p>
              {(description.length > 260 || description.split("\n").length > 5) && (
                <button
                  type="button"
                  aria-expanded={showFullChange}
                  aria-controls={changeId}
                  onClick={() => setShowFullChange((value) => !value)}
                  className="self-start text-xs font-medium text-primary hover:underline"
                >
                  {showFullChange ? m.common_show_less() : m.history_show_all()}
                </button>
              )}
            </>
          ) : (
            <p className="m-0 text-sm text-subtext">{diffBase ? m.history_no_description_diff({ base: diffBase }) : m.history_no_description()}</p>
          )}
        </Section>
        {result && (
          <Section label={m.history_result()}>
            <div className="text-sm text-text">
              <Md text={result} />
            </div>
          </Section>
        )}
        <Section label={m.history_attempts_heading({ count: fmtNumber(attempts.length) })}>
          {attempts.length === 0 ? (
            <p className="m-0 text-sm text-subtext">{m.history_not_run_yet()}</p>
          ) : (
            <ol className="m-0 flex list-none flex-col p-0">
              {attempts.length > shownAttempts.length && (
                <li>
                  <button type="button" onClick={() => setShowAllAttempts(true)} className="flex items-center gap-1 py-1 text-xs text-subtext hover:text-text">
                    <ChevronRight size={11} /> {m.history_earlier_attempts({ count: fmtNumber(attempts.length - shownAttempts.length) })}
                  </button>
                </li>
              )}
              {shownAttempts.map((run) => {
                const index = attempts.indexOf(run);
                return (
                  <li key={run.id} className="border-b border-divider-faint last:border-b-0">
                    <button type="button" onClick={() => onOpenRun(run)} className="flex w-full flex-col gap-0.5 rounded-sm px-1.5 py-2 text-start hover:bg-hover-faint">
                      <span className="flex items-center gap-2 text-sm">
                        <span className="w-5 tabular-nums text-muted">{index + 1}.</span>
                        <span className="font-mono font-medium text-text">{compactHistoryId(run.id)}</span>
                        <StatusLabel status={historyStatus(run)} size={11} />
                        <span className="ms-auto">
                          <When ms={run.createdAt} />
                        </span>
                      </span>
                      <span className="ps-7 text-xs text-subtext">
                        {historyBackendLabel(run)} · {m.history_attempt_commit({ sha: run.commitSha ? ltr(compactHistoryId(run.commitSha, 7)) : m.history_commit_unavailable() })} · {m.history_attempt_job({ id: ltr(historyJobId(run)) })}
                        {index > 0 && <span className="text-muted"> · {describeAttemptChange(run, attempts[index - 1]).toLowerCase()}</span>}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ol>
          )}
        </Section>
        <div className="flex gap-2">
          <button
            type="button"
            className={`${button} disabled:cursor-not-allowed disabled:opacity-50`}
            disabled={!diffBase}
            title={diffBase ? undefined : m.history_no_parent_hint()}
            onClick={() => onOpenChanges(node.id)}
          >
            <GitBranch size={13} /> {diffBase ? m.history_diff_from({ base: diffBase }) : m.history_no_parent_to_diff()}
          </button>
          {node.latestRun && (
            <button type="button" className={button} onClick={() => onOpenRun(node.latestRun!)}>
              <Terminal size={13} /> {m.experiments_table_logs()}
            </button>
          )}
        </div>
      </div>
    </aside>
  );
}

// ---- view -----------------------------------------------------------------------------
export function ExperimentHistory({
  project,
  experiments,
  runs,
  showArchived,
  agentSessionId,
  emptyHint,
  onOpenChanges,
  onOpenRun,
}: {
  project: Project;
  /** Every experiment, archived ones included, so a shown experiment keeps its lineage. */
  experiments: Experiment[];
  runs: Run[];
  showArchived: boolean;
  agentSessionId?: string | null;
  emptyHint?: string;
  onOpenChanges: (experimentId: string) => void;
  onOpenRun: (run: Run) => void;
}) {
  const [prefs, setPrefs] = usePrefs();
  const sessions = useQuery(listChatSessionsQuery(project.id));
  const taskTitles = useMemo(
    () => new Map((sessions.data ?? []).map((session) => [session.id, session.title || undefined])),
    [sessions.data],
  );
  const taskTitle = (task: string | null) => (task ? taskTitles.get(task) || m.history_untitled_task() : m.history_not_in_task());
  const nodes = useMemo(() => buildHistoryNodes(experiments, runs), [experiments, runs]);
  const byId = useMemo(() => new Map(nodes.map((node) => [node.id, node])), [nodes]);

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [pathOnly, setPathOnly] = useState(false);
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [closedGroups, setClosedGroups] = useState<ReadonlySet<string>>(new Set());
  const [rootRef, width] = useWidth<HTMLDivElement>();
  const listRef = useRef<HTMLDivElement>(null);
  const selected = selectedId ? byId.get(selectedId) ?? null : null;

  useEffect(() => {
    if (selectedId && !byId.has(selectedId)) setSelectedId(null);
  }, [byId, selectedId]);

  // The selected experiment's ancestry is drawn in the primary colour; "Path only"
  // also keeps its descendants.
  const path = useMemo(() => new Set(selected ? [...ancestorsOf(selected), selected].map((node) => node.id) : []), [selected]);
  const lineage = useMemo(() => {
    const ids = new Set(path);
    if (selected) for (const node of subtreeOf(selected)) ids.add(node.id);
    return ids;
  }, [path, selected]);

  const scoped = useMemo(
    () => nodes.filter((node) => (!agentSessionId || node.task === agentSessionId) && (showArchived || !node.experiment.archived)),
    [nodes, agentSessionId, showArchived],
  );
  const needle = query.trim().toLowerCase();
  const visible = useMemo(() => {
    let shown = scoped;
    if (needle) {
      const hits = new Set(
        shown
          .filter((node) => [node.experiment.slug, node.experiment.title ?? "", node.code ?? ""].some((text) => text.toLowerCase().includes(needle)))
          .map((node) => node.id),
      );
      // keep ancestors as context so a matching experiment stays attached to its lineage
      for (const id of [...hits]) for (const ancestor of ancestorsOf(byId.get(id)!)) hits.add(ancestor.id);
      shown = shown.filter((node) => hits.has(node.id));
    }
    if (pathOnly && selected) shown = shown.filter((node) => lineage.has(node.id));
    return shown;
  }, [scoped, needle, pathOnly, selected, lineage, byId]);

  const groups = useMemo(
    () => groupHistory(visible, prefs.grouping, (task) => taskTitles.get(task)),
    [visible, prefs.grouping, taskTitles],
  );
  const lineageOrder = prefs.order === "lineage";
  // Nothing folds during a search, which could otherwise fold a match away.
  const rowsByGroup = useMemo(
    () =>
      new Map(
        groups.map((group) => [
          group.key,
          lineageOrder
            ? layoutLineage(group, { collapseFinished: !needle, expanded, keep: path })
            : layoutChronological(group, prefs.order === "newest"),
        ]),
      ),
    [groups, lineageOrder, prefs.order, needle, expanded, path],
  );
  const lanes = lineageOrder ? Math.max(1, ...[...rowsByGroup.values()].map(laneCount)) : 0;
  const gutter = lineageOrder ? RAIL_PAD + LANE * lanes + 10 : 16;

  const fit = historyFit(width, Boolean(selected), lineageOrder);

  const rowElement = (id: string) => listRef.current?.querySelector<HTMLElement>(`[data-experiment="${CSS.escape(id)}"]`);
  const reveal = (id: string) => requestAnimationFrame(() => rowElement(id)?.scrollIntoView({ block: "nearest" }));
  const focusRow = (id: string) =>
    requestAnimationFrame(() => {
      const row = rowElement(id);
      row?.focus({ preventScroll: true });
      row?.scrollIntoView({ block: "nearest" });
    });
  const closeDetail = () => {
    const id = selectedId;
    setSelectedId(null);
    if (id) focusRow(id);
  };
  const select = (id: string) => {
    const node = byId.get(id);
    if (!node) return;
    setSelectedId(id);
    // open the chapter holding it and any collapsed branch that hides it
    setClosedGroups((current) => {
      const next = new Set(current);
      next.delete(groupKeyOf(node, prefs.grouping));
      return next;
    });
    reveal(id);
  };

  const order = groups
    .filter((group) => !closedGroups.has(group.key))
    .flatMap((group) => (rowsByGroup.get(group.key) ?? []).flatMap((row) => (row.kind === "experiment" ? [row.node.id] : [])));
  const tabStop = selectedId && order.includes(selectedId) ? selectedId : order[0];
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape" && selectedId) {
      closeDetail();
      return;
    }
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    if (!order.length) return;
    event.preventDefault();
    const index = selectedId ? order.indexOf(selectedId) : -1;
    const next = order[Math.min(order.length - 1, Math.max(0, index + (event.key === "ArrowDown" ? 1 : -1)))];
    setSelectedId(next);
    focusRow(next);
  };

  if (scoped.length === 0) {
    return (
      <div className="empty-state absolute inset-0 flex items-center justify-center p-6 text-center text-sm text-subtext">
        <p className="m-0 max-w-[46ch]">{emptyHint ?? m.history_empty()}</p>
      </div>
    );
  }

  const columns = (
    <>
      <span className="w-24 shrink-0">{m.settings_page_status()}</span>
      {fit.runs && <span className="w-20 shrink-0">{m.history_column_attempts()}</span>}
      {fit.runs && <span className="w-16 shrink-0 text-end">{m.history_column_latest()}</span>}
    </>
  );

  return (
    <div ref={rootRef} className="absolute inset-0 flex min-h-0 bg-background">
      <div className="flex min-w-0 flex-1 flex-col" inert={fit.listCovered}>
        <div className="flex min-h-11 shrink-0 flex-wrap items-center gap-x-4 gap-y-1.5 border-b border-divider-faint px-4 py-1.5 text-xs text-subtext">
          <Segmented label={m.history_group()} value={prefs.grouping} options={[["task", m.history_group_task()], ["lineage", m.history_lineage()], ["none", m.history_group_none()]]} onChange={(grouping) => setPrefs({ grouping })} />
          <Segmented label={m.history_order()} value={prefs.order} options={[["lineage", m.history_lineage()], ["oldest", m.history_order_oldest()], ["newest", m.history_order_newest()]]} onChange={(value) => setPrefs({ order: value })} />
          <label className="flex h-7 min-w-32 max-w-60 flex-1 items-center gap-1.5 rounded-sm border border-border-variant px-2 focus-within:border-border">
            <Search size={12} className="shrink-0 text-muted" />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder={m.app_filter_experiments()}
              aria-label={m.app_filter_experiments()}
              className="min-w-0 flex-1 rounded-none border-0 bg-transparent p-0 text-xs text-text outline-none placeholder:text-muted"
            />
          </label>
          <button
            type="button"
            disabled={!selected}
            aria-pressed={pathOnly && Boolean(selected)}
            onClick={() => setPathOnly((value) => !value)}
            title={m.history_path_only_hint()}
            className={`rounded-sm border px-2 py-0.5 disabled:opacity-40 ${pathOnly && selected ? "border-primary text-primary" : "border-border-variant"}`}
          >
            {m.history_path_only()}
          </button>
          <span className="ms-auto whitespace-nowrap tabular-nums">
            {visible.length === nodes.length
              ? m.tree_experiment_count({ count: fmtNumber(nodes.length) })
              : m.history_filtered_count({ shown: fmtNumber(visible.length), total: fmtNumber(nodes.length) })}
          </span>
        </div>

        <div ref={listRef} role="group" aria-label={m.history_list_label()} onKeyDown={onKeyDown} className="min-h-0 flex-1 overflow-auto pb-10">
          {visible.length === 0 ? (
            <div className="p-8 text-center text-sm text-subtext">
              {query ? m.history_no_match({ query }) : m.history_off_path()}{" "}
              <button
                type="button"
                className="text-primary hover:underline"
                onClick={() => {
                  setQuery("");
                  setPathOnly(false);
                }}
              >
                {m.history_show_all()}
              </button>
            </div>
          ) : (
            <>
              <div className="sticky top-0 z-20 flex h-8 items-center gap-4 border-b border-border-variant bg-background pe-4 text-xs font-medium text-subtext" style={{ paddingInlineStart: gutter }}>
                <span className="min-w-0 flex-[2] truncate">{m.tree_experiment()}</span>
                {fit.whatChanged && <span className="min-w-0 flex-[3]">{m.history_what_changed()}</span>}
                {fit.from && <span className="w-32 shrink-0">{m.history_column_from()}</span>}
                {columns}
              </div>
              {groups.map((group) => {
                const open = !closedGroups.has(group.key);
                const placed = place(rowsByGroup.get(group.key) ?? []);
                const height = placed.reduce((sum, item) => sum + item.height, 0);
                const rule = (left: number) => (
                  <span aria-hidden="true" className="pointer-events-none absolute bottom-0 right-0 h-px bg-divider-faint" style={{ left }} />
                );
                return (
                  <div key={group.key} role="group" aria-label={group.title || m.app_experiments()}>
                    {group.title && <ChapterHeader group={group} open={open} showSummary={fit.chapterSummary} onToggle={() => setClosedGroups((current) => toggled(current, group.key))} />}
                    {open && (
                      <div className="relative" style={{ height }}>
                        {lineageOrder && <Rail placed={placed} height={height} path={path} selectedId={selectedId} />}
                        {placed.map((item) => {
                          const style = { top: item.top, height: item.height, paddingInlineStart: gutter };
                          if (item.row.kind === "lead-in") {
                            const { from, node } = item.row;
                            return (
                              <button
                                key={`lead-${node.id}`}
                                type="button"
                                onClick={() => select(from.id)}
                                className="absolute inset-x-0 flex items-center gap-1.5 whitespace-nowrap pe-4 text-xs text-subtext hover:text-text"
                                style={style}
                              >
                                <span className="min-w-0 truncate">{m.history_origin_from({ name: ltr(from.experiment.slug) })}</span>
                                {from.task !== node.task && <span className="min-w-0 truncate text-muted">· {taskTitle(from.task)}</span>}
                              </button>
                            );
                          }
                          if (item.row.kind === "collapsed") {
                            const { head, hidden } = item.row;
                            const latest = hidden.reduce((max, node) => Math.max(max, node.latestRun?.createdAt ?? node.createdAt), 0);
                            return (
                              <button
                                key={`fold-${head.id}`}
                                type="button"
                                onClick={() => setExpanded((current) => new Set(current).add(head.id))}
                                className="absolute inset-x-0 flex items-center gap-4 pe-4 text-start text-sm text-subtext hover:bg-hover-faint"
                                style={style}
                              >
                                {rule(gutter)}
                                <span className="min-w-0 flex-[2] truncate">
                                  <span className="text-text">{m.history_collapsed_more({ count: fmtNumber(hidden.length) })}</span> · {shortName(head)} → {shortName(hidden.at(-1)!)}
                                </span>
                                {fit.whatChanged && <span className="min-w-0 flex-[3] truncate">{m.history_collapsed_hint()}</span>}
                                <span className="w-24 shrink-0" />
                                {fit.runs && <span className="w-20 shrink-0" />}
                                {fit.runs && (
                                  <span className="w-16 shrink-0 text-end">
                                    <When ms={latest} />
                                  </span>
                                )}
                              </button>
                            );
                          }
                          const node = item.row.node;
                          const isSelected = node.id === selectedId;
                          return (
                            <button
                              key={node.id}
                              type="button"
                              aria-pressed={isSelected}
                              tabIndex={node.id === tabStop ? 0 : -1}
                              data-experiment={node.id}
                              onClick={() => (isSelected ? closeDetail() : setSelectedId(node.id))}
                              className={`absolute inset-x-0 flex scroll-mt-9 items-center gap-4 pe-4 text-start ${isSelected ? "bg-highlight shadow-row-selected" : "hover:bg-hover-faint"}`}
                              style={style}
                            >
                              {rule(gutter)}
                              <span className={`min-w-0 flex-[2] truncate font-mono text-sm text-text ${isSelected ? "font-medium" : ""}`} title={node.experiment.slug}>
                                {node.experiment.slug}
                              </span>
                              {fit.whatChanged && (
                                <span className="min-w-0 flex-[3] truncate text-sm text-subtext" title={node.experiment.title ?? undefined}>
                                  {node.code ? `${node.code}: ` : ""}
                                  {node.label}
                                </span>
                              )}
                              {fit.from && (
                                <span className="w-32 shrink-0 truncate text-xs text-subtext">
                                  {originOf(node, 16)}
                                </span>
                              )}
                              <span className="w-24 shrink-0">
                                <StatusLabel status={node.status} />
                              </span>
                              {fit.runs && (
                                <span className="w-20 shrink-0">
                                  <AttemptMarks runs={node.runs} />
                                </span>
                              )}
                              {fit.runs && (
                                <span className="w-16 shrink-0 text-end">
                                  <When ms={node.latestRun?.createdAt ?? node.createdAt} />
                                </span>
                              )}
                            </button>
                          );
                        })}
                      </div>
                    )}
                  </div>
                );
              })}
            </>
          )}
        </div>
      </div>
      {selected && (
        <ExperimentDetail
          node={selected}
          baselineBranch={project.baselineBranch}
          overlay={fit.overlay}
          taskTitle={taskTitle}
          onSelect={select}
          onClose={closeDetail}
          onOpenChanges={onOpenChanges}
          onOpenRun={onOpenRun}
        />
      )}
    </div>
  );
}
