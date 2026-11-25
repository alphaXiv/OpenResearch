import { useCallback, useLayoutEffect, useRef, useState, type FocusEvent, type KeyboardEvent, type MouseEvent, type ReactElement, type ReactNode } from "react";
import { fmtDuration } from "../../../api";
import { autoDir, ltr } from "../../../i18n";
import { m } from "../../../paraglide/messages.js";
import { getLocale } from "../../../paraglide/runtime.js";
import { fmtMagnitude, fmtMeasure, fmtSignal } from "../../../runMonitor/format";
import { versusOf } from "../../../runMonitor/model";
import type { ChartBlock, Point, RunMonitor, SignalSpec, ViewBlock } from "../../../runMonitor/types";
import { useBoxSize } from "../useBoxSize";
import { chartOf, type ChartData, type Series } from "./series";
import { ChartMenu } from "./ChartMenu";
import { midSentence } from "./Stats";

/** Path segments for one line; a missing value (NaN on the wire) breaks it. */
function pathOf(points: Point[], x: (value: number) => number, y: (value: number) => number): string {
  let path = "";
  let pen = "M";
  for (const [px, py] of points) {
    if (py === null) { pen = "M"; continue; }
    path += `${pen}${x(px).toFixed(1)} ${y(py).toFixed(1)}`;
    pen = "L";
  }
  return path;
}

/** A value with its signal's unit, as a readout or a description says it. */
function withUnit(spec: SignalSpec | undefined, value: number | null): string {
  const shown = fmtSignal(spec, value);
  return spec?.unit && spec.unit !== "%" && value !== null ? `${shown} ${spec.unit}` : shown;
}

/** The latest value of each line and how it compares with the parent, for a screen reader. */
function describe(monitor: RunMonitor, series: Series[], live: boolean): string {
  return series.filter((line) => !line.ghost).map((line) => {
    const spec = monitor.view.signals[line.name];
    const value = `${line.label} ${fmtSignal(spec, monitor.latest[line.name])}`;
    const vs = versusOf(monitor, line.name, live);
    if (!vs) return value;
    if (vs.flat) return `${value}, ${m.run_level_with_parent()}`;
    const delta = fmtMagnitude(spec, vs.mag);
    return `${value}, ${vs.good ? m.run_vs_better({ delta }) : m.run_vs_worse({ delta })} ${m.run_than_parent()}`;
  }).join("; ");
}

/** What a chart of a table's rows shows, line by line, for a screen reader: each category's value, or how many points span what. */
function describeRows(monitor: RunMonitor, { series, categories, notes }: ChartData): string {
  const lines = series.filter((line) => !line.ghost).flatMap((line) => {
    const spec = monitor.view.signals[line.y];
    const known = line.points.filter((point): point is [number, number] => point[1] !== null);
    if (known.length === 0) return [];
    if (categories) return [`${line.label}: ${known.map(([index, value]) => `${categories[index]} ${withUnit(spec, value)}`).join(", ")}`];
    const values = known.map(([, value]) => value);
    const count = new Intl.NumberFormat(getLocale()).format(known.length);
    return [known.length === 1
      ? m.run_chart_described_one({ label: line.label, count, value: withUnit(spec, values[0]) })
      : m.run_chart_described_other({ label: line.label, count, lo: withUnit(spec, Math.min(...values)), hi: withUnit(spec, Math.max(...values)) })];
  });
  return [lines.join("; "), ...notes].filter(Boolean).join(". ");
}

/** The stretch of a line between `lo` and `hi`, ending where it crosses either edge. */
function clip(points: Point[], lo: number, hi: number): Point[] {
  const at = (a: Point, b: Point, edge: number): Point => [edge, a[1] === null || b[1] === null ? null : a[1] + ((b[1] - a[1]) * (edge - a[0])) / (b[0] - a[0])];
  const out: Point[] = [];
  points.forEach((point, index) => {
    const before = points[index - 1];
    if (before && before[0] < lo && point[0] > lo) out.push(at(before, point, lo));
    if (point[0] >= lo && point[0] <= hi) out.push(point);
    if (before && before[0] < hi && point[0] > hi) out.push(at(before, point, hi));
  });
  return out;
}

/** Round values for an axis between `lo` and `hi`, about `count` of them. */
function ticks(lo: number, hi: number, count: number): number[] {
  const rough = (hi - lo || 1) / count;
  const magnitude = 10 ** Math.floor(Math.log10(rough));
  const q = rough / magnitude;
  const step = (q < 1.5 ? 1 : q < 3 ? 2 : q < 7 ? 5 : 10) * magnitude;
  const out: number[] = [];
  for (let value = Math.ceil(lo / step - 1e-9) * step; value <= hi + step * 1e-6; value += step) out.push(Number(value.toFixed(8)));
  return out;
}

/** Ticks on a log axis: 1, 2 and 5 of each power of ten while they fit, then powers of ten, then every few; round linear ticks across less than a doubling. */
function logTicks(lo: number, hi: number, count: number): number[] {
  const within = (multiples: number[], stride: number) => {
    const out: number[] = [];
    for (let power = Math.floor(Math.log10(lo)); power <= Math.ceil(Math.log10(hi)); power += stride) {
      for (const multiple of multiples) {
        const value = Number((multiple * 10 ** power).toPrecision(12));
        if (value >= lo * (1 - 1e-9) && value <= hi * (1 + 1e-9)) out.push(value);
      }
    }
    return out;
  };
  const fine = within([1, 2, 5], 1);
  if (fine.length < 2) return ticks(lo, hi, count);
  if (fine.length <= count * 2) return fine;
  const decades = Math.log10(hi / lo);
  return within([1], Math.max(1, Math.ceil(decades / (count * 2))));
}

/** Where `value` falls between `lo` and `hi`, from 0 to 1, along a linear or a log scale. */
const along = (value: number, lo: number, hi: number, log: boolean) =>
  log ? (Math.log(value) - Math.log(lo)) / (Math.log(hi) - Math.log(lo) || 1) : (value - lo) / (hi - lo || 1);

/** The range a chart of rows spans, padded so no point sits on an edge. */
function span(values: number[], log: boolean): [number, number] {
  const lo = Math.min(...values);
  const hi = Math.max(...values);
  if (log) {
    const ratio = lo === hi ? 2 : (hi / lo) ** 0.05;
    return [lo / ratio, hi * ratio];
  }
  const pad = (hi - lo) * 0.05 || Math.abs(hi) * 0.1 || 0.5;
  return [lo - pad, hi + pad];
}

/** The point of `points` nearest `x`, by binary search over a series ordered by x. */
function nearest(points: Point[], x: number): Point | undefined {
  let lo = 0;
  let hi = points.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (points[mid][0] < x) lo = mid + 1;
    else hi = mid;
  }
  const after = points[lo];
  const before = points[lo - 1];
  return before && after && Math.abs(before[0] - x) <= Math.abs(after[0] - x) ? before : after;
}

const extent = (values: number[]): [number, number] => [Math.min(...values), Math.max(...values)];

/** An axis in a line: its name, the span it covers and whether it is log. */
function rangeLine(name: string | null | undefined, lo: number, hi: number, label: (value: number) => string, log: boolean): string {
  const span = lo === hi ? label(lo) : `${label(lo)} → ${label(hi)}`;
  return [name && autoDir(name), span, log ? m.run_chart_log() : null].filter(Boolean).join(" ");
}

/** Wide enough apart to mark each point, as a run that reports once a round or an eval is. */
const DOT_SPACING = 14;

/** How near a landmark, in pixels along each axis, the pointer reads it out instead of the step beneath it. */
const SNAP = 12;

/** About how wide a character of an axis label is, to thin labels that would collide. */
const CHAR_WIDTH = 6.5;

/** A readout beside the pointer, in the top layer so no card around the chart clips it; `outside` floats it under the chart's box, over the box only where the screen has no room below, rather than inside it. */
function Readout({ at, width, outside, onDismiss, children }: { at: number; width: number; outside: boolean; onDismiss: () => void; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const readout = ref.current!;
    if (!readout.matches(":popover-open")) readout.showPopover();
    window.addEventListener("scroll", onDismiss, true);
    return () => window.removeEventListener("scroll", onDismiss, true);
  }, [onDismiss]);
  useLayoutEffect(() => {
    const readout = ref.current!;
    const box = readout.parentElement!.getBoundingClientRect();
    const size = readout.getBoundingClientRect();
    const left = at > width / 2 ? box.left + at - size.width - 10 : box.left + at + 10;
    const under = box.bottom + 6;
    readout.style.left = `${Math.max(8, Math.min(left, window.innerWidth - size.width - 8))}px`;
    readout.style.top = `${outside ? (under + size.height <= window.innerHeight - 8 ? under : Math.max(8, box.top - size.height - 6)) : box.top + 8}px`;
  });
  return (
    <div
      ref={ref}
      popover="manual"
      aria-hidden="true"
      className="pointer-events-none fixed inset-auto m-0 flex min-w-36 max-w-[calc(100vw-1rem)] flex-col gap-1 overflow-visible rounded-md border border-border bg-background px-2.5 py-2 text-xs text-ink-subtext shadow-popover"
    >
      {children}
    </div>
  );
}

/**
 * A chart block. In a strip it is the marks and one line under them naming each axis and the range it spans; on a page
 * it has axes and a dot per point when they are far enough apart. Both give a readout under the pointer, or at the
 * point an arrow key has reached. Over steps, each line ends in its latest value; from a table's rows, the axes name
 * their measures and anything left out of the picture is said under it.
 */
export function Chart({ monitor, block, title, size, live, height = 160 }: { monitor: RunMonitor; block: ChartBlock & Pick<ViewBlock, "phase">; title: string; size: "strip" | "page"; live: boolean; height?: number }) {
  const [ref, box] = useBoxSize<HTMLDivElement>();
  const [pointer, setPointer] = useState<{ at: number; y: number | null; keyboard: boolean } | null>(null);
  const dismiss = useCallback(() => setPointer(null), []);
  const hover = pointer?.at ?? null;
  const data = chartOf(monitor, block);
  const { categories, notes, rows } = data;
  const table = rows !== null;
  const page = size === "page";
  const width = box.width;
  const boxHeight = page ? height : box.height;
  const { signals } = monitor.view;
  const xLog = !categories && block.xScale === "log";
  const yLog = block.yScale === "log";
  const mark = categories ? "bars" : block.mark === "points" ? "points" : "line";

  const current = monitor.phases[monitor.progress?.phase ?? monitor.stageOrder.index ?? -1];
  const phase = block.phase === undefined ? current : monitor.phases.find((one) => one.name === block.phase);
  const ours = block.phase === undefined || phase === current;
  /** The steps the chart's stage counts; the run's total is the current stage's, never another's. */
  const total = ours ? monitor.x.max : monitor.x.name === "step" ? phase?.of ?? null : null;
  const unstarted = !ours && phase !== undefined && phase.startedAt === null && !phase.skipped;
  const known = (points: Point[]) => points.filter((point): point is [number, number] => point[1] !== null && Number.isFinite(point[1]));
  const reported = known(data.series.flatMap((line) => line.points));

  let x0: number;
  let x1: number;
  if (categories) [x0, x1] = [-0.5, categories.length - 0.5];
  else if (table) [x0, x1] = reported.length > 0 ? span(reported.map((point) => point[0]), xLog) : [xLog ? 1 : 0, xLog ? 10 : 1];
  else {
    // Over steps the axis spans the run's own total, or what it has reached; the parent's line is cut at its edge.
    const own = known(data.series.filter((line) => !line.ghost).flatMap((line) => line.points)).map((point) => point[0]);
    const theirs = known(data.series.filter((line) => line.ghost).flatMap((line) => line.points)).map((point) => point[0]);
    const reach = total !== null ? Math.max(total, ...own) : own.length > 0 ? Math.max(1, ...own) : null;
    const spanned = block.xRange === "parent" || own.length === 0 ? [...own, ...theirs] : own;
    if (xLog) [x0, x1] = spanned.length > 0 ? span(spanned, true) : [1, 10];
    else [x0, x1] = [Math.min(0, ...spanned), block.xRange === "parent" || reach === null ? Math.max(reach ?? 1, ...theirs) : reach];
  }
  const samples = table ? data.series : data.series.map((line) => ({
    ...line,
    points: line.points.filter(([at]) => at >= x0 && at <= x1),
  }));
  /** A noisy signal's own curve is drawn smoothed, its reports faint beneath. */
  const smoothOf = (line: Series) => table || mark !== "line" || line.ghost ? undefined : signals[line.y]?.smooth;
  const series = table || mark !== "line" ? samples : data.series.map((line) => {
    const smooth = smoothOf(line);
    let level: number | null = null;
    const points = smooth ? line.points.map(([at, value]): Point => {
      if (value === null) return [at, null];
      level = level === null ? value : level + smooth * (value - level);
      return [at, level];
    }) : line.points;
    return { ...line, points: clip(points, x0, x1) };
  });
  const drawn = known(series.flatMap((line) => line.points));
  const visible = samples === series ? drawn : known(samples.flatMap((line) => line.points));
  const nothing = unstarted ? (live ? m.run_block_stage_not_started({ stage: autoDir(phase.name) }) : m.run_block_stage_never_started({ stage: autoDir(phase.name) }))
    : rows === 0 ? (live ? m.run_block_no_rows() : m.run_block_no_rows_ended()) : live ? m.run_block_no_points() : m.run_block_no_points_ended();
  /** Nothing of this run's own to draw; a table's chart may still show the parent's under the words that say so. */
  const blank = table ? !series.some((line) => !line.ghost && line.points.some((point) => point[1] !== null)) : drawn.length === 0;
  const empty = blank && notes.length > 0 && rows !== 0 ? notes.join(" ") : nothing;
  const shownNotes = blank ? [] : notes;
  const owned = series.filter((line) => !line.ghost);
  const lead = owned[0];
  /** What an axis reads by: the first signal its lines declare, unless two declare different units. */
  const axisSpec = (column: (line: Series) => string) => {
    const specs = series.flatMap((line) => !line.ghost && signals[column(line)] ? [signals[column(line)]] : []);
    return new Set(specs.map((spec) => spec.unit)).size <= 1 ? specs[0] : undefined;
  };
  const xSpec = table ? axisSpec((line) => line.x) : undefined;
  const ySpec = table ? axisSpec((line) => line.y) : undefined;
  const yCaption = [ySpec?.unit !== "%" ? ySpec?.unit : null, yLog ? m.run_chart_log() : null].filter(Boolean).join(", ");
  const number = new Intl.NumberFormat(getLocale(), { maximumFractionDigits: 4 });
  const compact = new Intl.NumberFormat(getLocale(), { notation: "compact", maximumFractionDigits: 1 });
  const elapsed = !table && (monitor.x.name === "elapsed" || (monitor.x.name === "step" && /^(s|sec|secs|second|seconds)$/i.test(phase?.unit ?? "")));
  const coordinate = elapsed ? m.run_chart_elapsed() : monitor.x.name;
  /** A tick on a table's axis reads as its signal does, scaled and in percent, at the size of the tick. */
  const tick = (spec: SignalSpec | undefined, value: number) => Math.abs(value * (spec?.scale ?? 1)) >= 10_000
    ? compact.format(value * (spec?.scale ?? 1))
    : fmtSignal(spec && { label: spec.label, unit: spec.unit, scale: spec.scale, as: spec.as }, value);
  const xLabel = (value: number) => table ? tick(xSpec, value) : elapsed ? fmtDuration(value * 1000) : Math.abs(value) >= 10_000 ? compact.format(value) : number.format(value);
  const yLabel = (value: number) => table ? tick(ySpec, value) : Math.abs(value) >= 10_000 ? compact.format(value) : fmtMeasure(value);
  const xName = (line: Series) => signals[line.x]?.label ?? line.x;
  const yNames = new Set(owned.map((line) => signals[line.y]?.label ?? line.y));
  /** The one measure a table's chart plots, which a readout names beside a value when its line is labelled by something else. */
  const yName = table && yNames.size === 1 ? [...yNames][0] : null;
  const ys = (visible === drawn ? drawn : [...drawn, ...visible]).map((point) => point[1]);
  const [xLo, xHi] = table && drawn.length > 0 ? extent(drawn.map((point) => point[0])) : [x0, x1];
  const xRange = !page && !categories && drawn.length > 0 ? rangeLine(table ? lead && xName(lead) : coordinate, xLo, xHi, xLabel, xLog) : "";
  const yRange = !page && drawn.length > 0 ? rangeLine(yName, Math.min(...ys), Math.max(...ys), yLabel, yLog) : "";
  /** A strip with room for its axes in a line under the plot, the two ranges at full length. */
  const rangeWidth = (xRange + yRange).replace(/[\u2066-\u2069]/g, "").length * CHAR_WIDTH + 12;
  const ranged = !page && boxHeight >= 44 && !blank && rangeWidth <= width;
  const margin = page ? { start: 38, end: categories ? 6 : 48, top: yCaption ? 22 : 8, bottom: 22 } : { start: 5, end: 5, top: 5, bottom: ranged ? 17 : 5 };
  const innerWidth = Math.max(1, width - margin.start - margin.end);
  const innerHeight = Math.max(1, boxHeight - margin.top - margin.bottom);
  let y0 = Math.min(...ys);
  let y1 = Math.max(...ys);
  if (yLog) [y0, y1] = drawn.length > 0 ? span(ys, true) : [1, 10];
  else if (categories) {
    y0 = Math.min(0, y0);
    y1 = Math.max(0, y1);
    const pad = (y1 - y0) * 0.1 || 0.5;
    if (y1 > 0) y1 += pad;
    if (y0 < 0) y0 -= pad;
  } else {
    const pad = (y1 - y0) * 0.1 || Math.abs(y1) * 0.1 || 0.5;
    y0 -= pad;
    y1 += pad;
  }
  const x = (value: number) => margin.start + along(value, x0, x1, xLog) * innerWidth;
  const y = (value: number) => margin.top + (1 - along(value, y0, y1, yLog)) * innerHeight;
  const base = y(yLog ? y0 : Math.max(y0, Math.min(0, y1)));
  const last = lead && [...lead.points].reverse().find((point) => point[1] !== null);
  const label = table || categories ? describeRows(monitor, data) : [describe(monitor, series, live), ...notes].filter(Boolean).join(". ");

  const slot = innerWidth / Math.max(1, categories?.length ?? 1);
  const groupWidth = slot * 0.7;
  const seriesWidth = groupWidth / Math.max(1, owned.length);
  const barsBySeries = new Map((categories ? series : []).map((line) => {
    const groups = new Map<number, [number, number][]>();
    for (const point of known(line.points)) {
      const at = point[0];
      const group = groups.get(at);
      if (group) group.push(point);
      else groups.set(at, [point]);
    }
    return [line, groups];
  }));
  /** Each category gives a series its own band, divided among its reported rows. */
  const bars = (strip: boolean) => [...owned, ...series.filter((line) => line.ghost)].map((line) => {
    const index = line.ghost ? owned.findIndex((one) => `parent ${one.name}` === line.name) : owned.indexOf(line);
    return (
      <g key={line.name}>
        {[...(barsBySeries.get(line) ?? [])].flatMap(([at, values]) => {
          const start = x(at) - groupWidth / 2 + index * seriesWidth;
          const width = seriesWidth / values.length;
          const gap = Math.min(strip ? 0.5 : 1, width / 4);
          const overhang = values.length === 1 ? Math.min(3, (slot - groupWidth) / 2) : 0;
          return values.map(([, value], row) => {
            const key = `${at}:${row}`;
            const left = start + row * width + gap;
            const top = y(value);
            if (line.ghost) return <line key={key} x1={left - overhang} x2={left + width - 2 * gap + overhang} y1={top} y2={top} stroke={line.color} strokeWidth="1.5" strokeDasharray="3 2" />;
            return <rect key={key} x={left} width={width - 2 * gap} y={Math.min(top, base)} height={Math.max(1, Math.abs(base - top))} rx="1.5" fill={line.color} />;
          });
        })}
      </g>
    );
  });
  const points = (line: Series, radius: number) => line.points.map(([px, py], index) => py !== null && (
    line.ghost
      ? <circle key={index} cx={x(px)} cy={y(py)} r={radius - 1} fill="none" stroke={line.color} strokeWidth="1.5" strokeDasharray="2 1.5" />
      : <circle key={index} cx={x(px)} cy={y(py)} r={radius} fill={line.color} stroke="var(--base)" strokeWidth={radius > 3 ? 2 : 1} />
  ));

  /** Each own line's first report and its best one, where the line has no parent to read against. */
  const landmarks = mark === "line" && !table && !series.some((line) => line.ghost) ? samples.flatMap((line) => {
    const values = known(line.points);
    const first = values[0];
    if (!first) return [];
    const better = signals[line.y]?.better;
    const best = better && values.reduce((best, point) => (better === "lower" ? point[1] < best[1] : point[1] > best[1]) ? point : best, first);
    const label = autoDir(midSentence(line.label));
    const start = { line, point: first, best: false, name: m.run_chart_start({ label }) };
    if (!best) return [start];
    const top = { line, point: best, best: true, name: better === "lower" ? m.run_chart_lowest({ label }) : m.run_chart_highest({ label }) };
    return best === first ? [top] : [start, top];
  }) : [];
  const marks = <>
    {!table && total !== null && <line x1={x(total)} x2={x(total)} y1={margin.top} y2={margin.top + innerHeight} stroke="var(--border)" strokeDasharray="2 3" />}
    {landmarks.map(({ line, point, best }) => <circle key={`${line.name}:${best}`} cx={x(point[0])} cy={y(point[1])} r={best ? 3.5 : 3} fill={best ? line.color : "var(--surface)"} stroke={line.color} strokeWidth="1.5" />)}
  </>;

  const readout = hover === null || drawn.length === 0 ? null : (() => {
    const at = categories ? Math.round(x0 + ((hover - margin.start) / innerWidth) * (x1 - x0)) : null;
    const position = (hover - margin.start) / innerWidth;
    const height = pointer?.y ?? null;
    const reach = (one: (typeof landmarks)[number]) => [Math.abs(x(one.point[0]) - hover), Math.abs(y(one.point[1]) - height!)];
    const near = height === null ? [] : landmarks.filter((one) => reach(one).every((offset) => offset <= SNAP));
    const landmark = near.sort((a, b) => Math.hypot(...reach(a)) - Math.hypot(...reach(b)))[0];
    const target = landmark?.point[0] ?? at ?? (xLog ? Math.exp(Math.log(x0) + position * (Math.log(x1) - Math.log(x0))) : x0 + position * (x1 - x0));
    const hits = samples.flatMap((line, index) => {
      const point = landmark?.line === line ? landmark.point : at === null ? nearest(line.points, target) : barsBySeries.get(line)?.get(at)?.[0];
      const within = line.points.length > 0 && target >= line.points[0][0] && target <= line.points[line.points.length - 1][0];
      if (!point || point[1] === null || !(at !== null || within || (pointer?.keyboard && !line.ghost) || Math.abs(x(point[0]) - hover) <= 40)) return [];
      const levels = series[index].points;
      const leadingEdge = levels[0]?.[0] < line.points[0][0] ? 1 : 0;
      const level = smoothOf(line) ? levels[line.points.indexOf(point) + leadingEdge]?.[1] ?? null : null;
      return [{ line, point, level }];
    });
    if (hits.length === 0) return null;
    const mine = hits.filter((hit) => !hit.line.ghost);
    const anchor = hits.find((hit) => hit.point === landmark?.point)
      ?? (mine.length > 0 ? mine : hits).reduce((best, hit) => Math.abs(x(hit.point[0]) - hover) < Math.abs(x(best.point[0]) - hover) ? hit : best);
    /** The landmarks at the readout's step, on lines that report there. */
    const named = landmarks.filter((one) => one.point[0] === anchor.point[0] && hits.some((hit) => hit.point === one.point));
    return { hits, anchor, named };
  })();
  const readoutTitle = ({ anchor, named }: NonNullable<typeof readout>) => {
    if (categories) return categories[anchor.point[0]];
    if (table) return `${xName(anchor.line)} ${withUnit(signals[anchor.line.x], anchor.point[0])}`;
    const where = `${autoDir(coordinate)} ${ltr(xLabel(anchor.point[0]))}`;
    if (named.length === 0) return where;
    return m.run_chart_at({ value: new Intl.ListFormat(getLocale(), { type: "conjunction" }).format(named.map((one) => one.name)), x: where });
  };
  const readoutValue = (line: Series, point: Point, anchor: Point) => {
    const format = (value: Point[1]) => table ? withUnit(signals[line.y], value) : fmtSignal(signals[line.y], value);
    const group = barsBySeries.get(line)?.get(point[0]);
    if (group) return new Intl.ListFormat(getLocale(), { type: "unit" }).format(group.map(([, value]) => format(value)));
    const value = format(point[1]);
    if (point[0] === anchor[0]) return value;
    return m.run_chart_at({ value, x: table ? withUnit(signals[line.x], point[0]) : xLabel(point[0]) });
  };

  const readoutRows = readout?.hits.map(({ line, point, level }) => ({
    line,
    name: !line.ghost ? line.label : series.filter((one) => one.ghost).length > 1 ? m.run_chart_parent_line({ label: line.label }) : m.run_chart_parent(),
    value: readoutValue(line, point, readout.anchor.point),
    smoothed: level === null ? null : m.run_chart_smoothed({ value: fmtSignal(signals[line.y], level) }),
    measure: yName !== null && line.label !== yName ? yName : null,
  }));
  const spoken = readout && readoutRows && `${readoutTitle(readout)}: ${readoutRows.map((row) => `${row.name} ${row.measure ? `${row.measure} ` : ""}${row.value}${row.smoothed ? `, ${row.smoothed}` : ""}`).join(", ")}`;

  const own = known(samples.filter((line) => !line.ghost).flatMap((line) => line.points));
  const stops = [...new Set((own.length > 0 ? own : visible).map((point) => point[0]))].sort((a, b) => a - b);
  const interact = {
    tabIndex: 0,
    role: "group",
    "aria-label": `${autoDir(title)}. ${m.run_chart_scrub()}`,
    onMouseMove: (event: MouseEvent<HTMLDivElement>) => {
      const box = event.currentTarget.getBoundingClientRect();
      const position = event.clientX - box.left;
      setPointer(position >= margin.start && position <= margin.start + innerWidth ? { at: position, y: event.clientY - box.top, keyboard: false } : null);
    },
    onMouseLeave: () => setPointer((current) => current?.keyboard ? current : null),
    onFocus: (event: FocusEvent<HTMLDivElement>) => {
      if (pointer === null && stops.length > 0 && event.currentTarget.matches(":focus-visible")) setPointer({ at: x(stops[stops.length - 1]), y: null, keyboard: true });
    },
    onBlur: dismiss,
    onKeyDown: (event: KeyboardEvent<HTMLDivElement>) => {
      if (event.key === "Escape" && pointer) {
        event.preventDefault();
        event.stopPropagation();
        setPointer(null);
        return;
      }
      const here = readout ? stops.indexOf(readout.anchor.point[0]) : stops.length;
      const next = { ArrowLeft: here - 1, ArrowRight: here + 1, Home: 0, End: stops.length - 1 }[event.key];
      if (next === undefined || stops.length === 0) return;
      event.preventDefault();
      setPointer({ at: x(stops[Math.max(0, Math.min(stops.length - 1, next))]), y: null, keyboard: true });
    },
  };
  const focusRing = (
    <span
      aria-hidden="true"
      className="pointer-events-none absolute rounded-sm group-focus-visible:outline-2 group-focus-visible:outline-solid group-focus-visible:outline-focus group-focus-visible:-outline-offset-2"
      style={{ left: margin.start, top: margin.top, width: innerWidth, height: innerHeight }}
    />
  );

  /** The cursor stands at the step the readout reads, never between reports. */
  const cursorAt = readout && x(readout.anchor.point[0]);
  const cursor = readout && <>
    <line x1={cursorAt!} x2={cursorAt!} y1={margin.top} y2={margin.top + innerHeight} stroke="var(--border)" />
    {mark !== "bars" && readout.hits.map(({ line, point, level }) => (
      <g key={line.name}>
        {level !== null && <circle cx={x(point[0])} cy={y(point[1]!)} r="3" fill={line.color} stroke="var(--base)" strokeWidth="1.5" />}
        <circle cx={x(point[0])} cy={y(level ?? point[1]!)} r="4" fill={line.color} stroke="var(--base)" strokeWidth="2" />
      </g>
    ))}
  </>;
  const readoutView = readout && readoutRows && (
    <Readout at={cursorAt!} width={width} outside={!page} onDismiss={dismiss}>
      <b className="font-medium text-text">{readoutTitle(readout)}</b>
      {readoutRows.map(({ line, name, value, smoothed, measure }) => (
        <span key={line.name} className="flex flex-col">
          <span className="flex items-center gap-1.5">
            <svg width="12" height="2" className="shrink-0"><line x1="0" x2="12" y1="1" y2="1" stroke={line.color} strokeWidth="2" strokeDasharray={line.ghost ? "3 2" : undefined} /></svg>
            {name}
            <b className="ms-auto ps-3 text-end font-medium tabular-nums text-text">{measure && <span className="font-normal text-ink-subtext">{measure} </span>}{value}</b>
          </span>
          {smoothed && <span className="text-end tabular-nums">{smoothed}</span>}
        </span>
      ))}
    </Readout>
  );
  const status = <span role="status" className="sr-only">{pointer?.keyboard && spoken ? spoken : ""}</span>;
  const withMenu = (trigger: ReactElement) => (
    <ChartMenu monitor={monitor} block={block} limits={data.log} anchor={ref} onOpen={dismiss} trigger={trigger} />
  );

  if (!page) {
    return withMenu(
      <div dir="ltr" className="group relative h-full min-h-6 w-full outline-none" {...interact}>
        {width > 0 && focusRing}
        {drawn.length > 0 && (
          <svg width={width} height={boxHeight} role="img" aria-label={label} className="absolute inset-0 overflow-visible">
            {width > 0 && ranged && <line x1={margin.start} x2={margin.start + innerWidth} y1={margin.top + innerHeight} y2={margin.top + innerHeight} stroke="var(--border-variant)" />}
            {width > 0 && mark === "bars" && bars(true)}
            {width > 0 && mark === "points" && series.map((line) => <g key={line.name}>{points(line, 2.5)}</g>)}
            {width > 0 && mark === "line" && series.map((line, index) => line.points.length > 0 && (
              <g key={line.name}>
                {smoothOf(line) && <path d={pathOf(samples[index].points, x, y)} fill="none" stroke={line.color} strokeWidth="1" opacity="0.3" strokeLinejoin="round" />}
                <path
                  d={pathOf(line.points, x, y)}
                  fill="none"
                  stroke={line.color}
                  strokeWidth={line.ghost ? 1.5 : 1.75}
                  strokeDasharray={line.ghost ? "4 3" : undefined}
                  strokeLinejoin="round"
                  strokeLinecap="round"
                />
              </g>
            ))}
            {width > 0 && marks}
            {width > 0 && !table && mark === "line" && lead && last && last[1] !== null && <circle cx={x(last[0])} cy={y(last[1])} r="3" fill={lead.color} stroke="var(--base)" strokeWidth="1.5" />}
            {width > 0 && cursor}
          </svg>
        )}
        {width > 0 && ranged && (
          <div aria-hidden="true" className="pointer-events-none absolute inset-x-0 bottom-0 flex justify-between gap-3 text-xs leading-3 text-ink-subtext">
            <span className="truncate">{xRange}</span>
            <span className="truncate">{yRange}</span>
          </div>
        )}
        {width > 0 && blank && <span className="text-xs text-ink-subtext">{empty}</span>}
        {readoutView}
        {status}
      </div>,
    );
  }

  const xCaption = table ? [xSpec?.unit ?? (lead ? xName(lead) : null), xLog ? m.run_chart_log() : null].filter(Boolean).join(", ") : xLog ? `${coordinate}, ${m.run_chart_log()}` : coordinate;
  const dotted = (line: Series) => !line.ghost && !smoothOf(line) && line.points.length > 0 && line.points.length * DOT_SPACING <= innerWidth;
  const captionStart = margin.start + innerWidth + margin.end - 2 - xCaption.length * CHAR_WIDTH;
  /** Over steps the ticks are as they always were; elsewhere a tick whose label would run into another or the caption goes. */
  const spaced = (values: number[]) => {
    if (!table && !xLog && !elapsed) return values;
    let reach = -Infinity;
    return values.filter((value) => {
      const half = (xLabel(value).length * CHAR_WIDTH) / 2;
      if (x(value) - half < reach + 6 || x(value) + half > captionStart - 6) return false;
      reach = x(value) + half;
      return true;
    });
  };
  /** An axis with nothing on it and no total to span reads no steps. */
  const xTicks = categories || (drawn.length === 0 && (table || total === null)) ? [] : spaced(xLog ? logTicks(x0, x1, width < 420 ? 3 : 5) : ticks(x0, x1, width < 420 ? 3 : 5));
  /** A table's line keeps its full ink under its points, which are its data, not samples along it. */
  const dim = (line: Series) => !table && dotted(line);
  const yTicks = yLog ? logTicks(y0, y1, 4) : ticks(y0, y1, 4);

  const ends = table ? [] : series.flatMap((line) => {
    const end = [...line.points].reverse().find((point) => point[1] !== null);
    return end && end[1] !== null ? [{ line, x: x(end[0]), y: y(end[1]), value: end[1] }] : [];
  }).sort((a, b) => a.y - b.y);
  let lastLabel = -Infinity;
  const endLabels = ends.flatMap((end) => {
    if (end.y - lastLabel < 13) return [];
    lastLabel = end.y;
    return [end];
  });

  const chart = withMenu(
    <div dir="ltr" className="group relative w-full outline-none" style={{ height: boxHeight }} {...interact}>
      {width > 0 && focusRing}
      <svg width={width} height={boxHeight} role="img" aria-label={blank ? empty : label} className="block overflow-visible text-xs">
          {drawn.length > 0 && yTicks.map((value) => (
            <g key={value}>
              <line x1={margin.start} x2={margin.start + innerWidth} y1={y(value)} y2={y(value)} stroke="var(--border-variant)" />
              <text x={margin.start - 6} y={y(value) + 3.5} textAnchor="end" fill="var(--subtext)">{yLabel(value)}</text>
            </g>
          ))}
          {drawn.length > 0 && yCaption && <text x={margin.start - 6} y={10} textAnchor="start" fill="var(--subtext)">{yCaption}</text>}
          <line x1={margin.start} x2={margin.start + innerWidth} y1={margin.top + innerHeight} y2={margin.top + innerHeight} stroke="var(--border)" />
          {xTicks.map((value) => <text key={value} x={x(value)} y={boxHeight - 6} textAnchor="middle" fill="var(--subtext)">{xLabel(value)}</text>)}
          {drawn.length > 0 && categories?.map((word, index) => (
            <foreignObject key={index} x={x(index) - slot / 2} y={boxHeight - 18} width={slot} height={18}>
              <div dir="auto" className="truncate px-0.5 text-center text-xs text-subtext">{word}</div>
            </foreignObject>
          ))}
          {!categories && <text x={margin.start + innerWidth + margin.end - 2} y={boxHeight - 6} textAnchor="end" fill="var(--subtext)">{xCaption}</text>}
          {blank && <text x={margin.start + innerWidth / 2} y={margin.top + innerHeight / 2} textAnchor="middle" fill="var(--subtext)">{empty}</text>}
          {mark === "bars" && bars(false)}
          {mark === "points" && series.map((line) => <g key={line.name}>{points(line, 3.5)}</g>)}
          {mark === "line" && series.map((line, index) => line.points.length > 0 && (
            <g key={line.name}>
              {smoothOf(line) && <path d={pathOf(samples[index].points, x, y)} fill="none" stroke={line.color} strokeWidth="1.25" opacity="0.3" strokeLinejoin="round" />}
              <path
                d={pathOf(line.points, x, y)}
                fill="none"
                stroke={line.color}
                strokeWidth={line.ghost ? 1.5 : 2}
                strokeDasharray={line.ghost ? "4 3" : undefined}
                strokeLinejoin="round"
                strokeLinecap="round"
                opacity={dim(line) ? 0.45 : 1}
              />
              {dotted(line) && line.points.map(([px, py], index) => py !== null && <circle key={index} cx={x(px)} cy={y(py)} r={table ? 3 : 4} fill={line.color} stroke="var(--base)" strokeWidth={table ? 1.5 : 2} />)}
            </g>
          ))}
          {marks}
          {cursor}
          {endLabels.map((end) => (
            <text key={end.line.name} x={Math.min(end.x + 7, margin.start + innerWidth + 7)} y={end.y + 4} fill={end.line.ghost ? "var(--subtext)" : "var(--text)"} fontWeight={end.line.ghost ? 400 : 500}>
              {end.line.ghost ? m.run_chart_parent_end() : fmtSignal(signals[end.line.y], end.value)}
            </text>
          ))}
      </svg>
      {readoutView}
      {status}
    </div>,
  );
  if (shownNotes.length === 0) return chart;
  return (
    <>
      {chart}
      <p className="m-0 mt-1.5 text-xs text-ink-subtext">{shownNotes.join(" ")}</p>
    </>
  );
}
