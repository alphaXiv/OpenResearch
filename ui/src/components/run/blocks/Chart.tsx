import { useState, type MouseEvent } from "react";
import { fmtDuration } from "../../../api";
import { m } from "../../../paraglide/messages.js";
import { getLocale } from "../../../paraglide/runtime.js";
import { fmtMagnitude, fmtMeasure, fmtSignal } from "../../../runMonitor/format";
import { versusOf } from "../../../runMonitor/model";
import type { ChartBlock, Point, RunMonitor, SignalSpec } from "../../../runMonitor/types";
import { useBoxSize } from "../useBoxSize";
import { chartOf, type ChartData, type Series } from "./series";

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

/** Wide enough apart to mark each point, as a run that reports once a round or an eval is. */
const DOT_SPACING = 14;

/** About how wide a character of an axis label is, to thin labels that would collide. */
const CHAR_WIDTH = 6.5;

/**
 * A chart block. In a strip it is the marks alone, no axes, filling its box; on a page it has axes, a dot per point
 * when they are far enough apart, and a readout under the pointer. Over steps, each line ends in its latest value; from
 * a table's rows, the axes name their measures and anything left out of the picture is said under it.
 */
export function Chart({ monitor, block, size, live, height = 160 }: { monitor: RunMonitor; block: ChartBlock; size: "strip" | "page"; live: boolean; height?: number }) {
  const [ref, box] = useBoxSize<HTMLDivElement>();
  const [hover, setHover] = useState<number | null>(null);
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
    const reach = monitor.x.max !== null ? Math.max(monitor.x.max, ...own) : own.length > 0 ? Math.max(1, ...own) : null;
    const spanned = block.xRange === "parent" || own.length === 0 ? [...own, ...theirs] : own;
    if (xLog) [x0, x1] = spanned.length > 0 ? span(spanned, true) : [1, 10];
    else [x0, x1] = [Math.min(0, ...spanned), block.xRange === "parent" || reach === null ? Math.max(reach ?? 1, ...theirs) : reach];
  }
  const series = table || categories ? data.series : data.series.map((line) => ({ ...line, points: clip(line.points, x0, x1) }));
  const drawn = known(series.flatMap((line) => line.points));
  const nothing = rows === 0 ? (live ? m.run_block_no_rows() : m.run_block_no_rows_ended()) : live ? m.run_block_no_points() : m.run_block_no_points_ended();
  /** Nothing of this run's own to draw; a table's chart may still show the parent's under the words that say so. */
  const blank = table ? !series.some((line) => !line.ghost && line.points.some((point) => point[1] !== null)) : drawn.length === 0;
  const empty = blank && notes.length > 0 && rows !== 0 ? notes.join(" ") : nothing;
  const shownNotes = blank ? [] : notes;
  const lead = series.find((line) => !line.ghost);
  /** What an axis reads by: the first signal its lines declare, unless two declare different units. */
  const axisSpec = (column: (line: Series) => string) => {
    const specs = series.flatMap((line) => !line.ghost && signals[column(line)] ? [signals[column(line)]] : []);
    return new Set(specs.map((spec) => spec.unit)).size <= 1 ? specs[0] : undefined;
  };
  const xSpec = table ? axisSpec((line) => line.x) : undefined;
  const ySpec = table ? axisSpec((line) => line.y) : undefined;
  const yCaption = [ySpec?.unit !== "%" ? ySpec?.unit : null, yLog ? m.run_chart_log() : null].filter(Boolean).join(", ");
  const margin = page ? { start: 38, end: 48, top: yCaption ? 22 : 8, bottom: 22 } : { start: 1, end: 5, top: 5, bottom: 3 };
  const innerWidth = Math.max(1, width - margin.start - margin.end);
  const innerHeight = Math.max(1, boxHeight - margin.top - margin.bottom);
  const ys = drawn.map((point) => point[1]);
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

  const owned = series.filter((line) => !line.ghost);
  const slot = innerWidth / Math.max(1, categories?.length ?? 1);
  const barWidth = (slot * 0.7) / Math.max(1, owned.length);
  /** A bar of each own line per category, side by side; the parent's value as a dashed level over its bar, reaching past it. */
  const bars = (strip: boolean) => [...owned, ...series.filter((line) => line.ghost)].map((line) => {
    const index = line.ghost ? series.filter((one) => one.ghost).indexOf(line) : owned.indexOf(line);
    const left = (at: number) => x(at) - (slot * 0.7) / 2 + index * barWidth;
    return (
      <g key={line.name}>
        {line.points.map(([at, value]) => {
          if (value === null) return null;
          const top = y(value);
          if (line.ghost) return <line key={at} x1={left(at) - 3} x2={left(at) + barWidth + 3} y1={top} y2={top} stroke={line.color} strokeWidth="1.5" strokeDasharray="3 2" />;
          return <rect key={at} x={left(at) + (strip ? 0.5 : 1)} width={Math.max(1, barWidth - (strip ? 1 : 2))} y={Math.min(top, base)} height={Math.max(1, Math.abs(base - top))} rx="1.5" fill={line.color} />;
        })}
      </g>
    );
  });
  const points = (line: Series, radius: number) => line.points.map(([px, py]) => py !== null && (
    line.ghost
      ? <circle key={px} cx={x(px)} cy={y(py)} r={radius - 1} fill="none" stroke={line.color} strokeWidth="1.5" strokeDasharray="2 1.5" />
      : <circle key={px} cx={x(px)} cy={y(py)} r={radius} fill={line.color} stroke="var(--base)" strokeWidth={radius > 3 ? 2 : 1} />
  ));

  if (!page) {
    return (
      <div ref={ref} dir="ltr" className="relative h-full min-h-6 w-full">
        {drawn.length > 0 && (
          <svg width={width} height={boxHeight} role="img" aria-label={label} className="absolute inset-0 overflow-visible">
            {width > 0 && mark === "bars" && bars(true)}
            {width > 0 && mark === "points" && series.map((line) => <g key={line.name}>{points(line, 2.5)}</g>)}
            {width > 0 && mark === "line" && series.map((line) => line.points.length > 0 && (
              <path
                key={line.name}
                d={pathOf(line.points, x, y)}
                fill="none"
                stroke={line.color}
                strokeWidth={line.ghost ? 1.5 : 1.75}
                strokeDasharray={line.ghost ? "4 3" : undefined}
                strokeLinejoin="round"
                strokeLinecap="round"
              />
            ))}
            {width > 0 && !table && mark === "line" && lead && last && last[1] !== null && <circle cx={x(last[0])} cy={y(last[1])} r="3" fill={lead.color} stroke="var(--base)" strokeWidth="1.5" />}
          </svg>
        )}
        {width > 0 && blank && <span className="text-xs text-ink-subtext">{empty}</span>}
      </div>
    );
  }

  const number = new Intl.NumberFormat(getLocale(), { maximumFractionDigits: 4 });
  const compact = new Intl.NumberFormat(getLocale(), { notation: "compact", maximumFractionDigits: 1 });
  const elapsed = !table && monitor.x.name === "elapsed";
  /** A tick on a table's axis reads as its signal does, scaled and in percent, at the size of the tick. */
  const tick = (spec: SignalSpec | undefined, value: number) => Math.abs(value * (spec?.scale ?? 1)) >= 10_000
    ? compact.format(value * (spec?.scale ?? 1))
    : fmtSignal(spec && { label: spec.label, unit: spec.unit, scale: spec.scale, as: spec.as }, value);
  const xLabel = (value: number) => table ? tick(xSpec, value) : elapsed ? fmtDuration(value * 1000) : Math.abs(value) >= 10_000 ? compact.format(value) : number.format(value);
  const yLabel = (value: number) => table ? tick(ySpec, value) : Math.abs(value) >= 10_000 ? compact.format(value) : fmtMeasure(value);
  const xName = (line: Series) => signals[line.x]?.label ?? line.x;
  const xCaption = table ? [xSpec?.unit ?? (lead ? xName(lead) : null), xLog ? m.run_chart_log() : null].filter(Boolean).join(", ") : xLog ? `${monitor.x.name}, ${m.run_chart_log()}` : monitor.x.name;
  const dotted = (line: Series) => !line.ghost && line.points.length > 0 && line.points.length * DOT_SPACING <= innerWidth;
  const captionStart = margin.start + innerWidth + margin.end - 2 - xCaption.length * CHAR_WIDTH;
  /** Over steps the ticks are as they always were; elsewhere a tick whose label would run into another or the caption goes. */
  const spaced = (values: number[]) => {
    if (!table && !xLog) return values;
    let reach = -Infinity;
    return values.filter((value) => {
      const half = (xLabel(value).length * CHAR_WIDTH) / 2;
      if (x(value) - half < reach + 6 || x(value) + half > captionStart - 6) return false;
      reach = x(value) + half;
      return true;
    });
  };
  const xTicks = categories ? [] : spaced(xLog ? logTicks(x0, x1, width < 420 ? 3 : 5) : ticks(x0, x1, width < 420 ? 3 : 5));
  /** A table's line keeps its full ink under its points, which are its data, not samples along it. */
  const dim = (line: Series) => !table && dotted(line);
  const yTicks = yLog ? logTicks(y0, y1, 4) : ticks(y0, y1, 4);
  const every = categories ? Math.max(1, Math.ceil((Math.max(...categories.map((word) => word.length)) * CHAR_WIDTH + 8) / slot)) : 1;

  const readout = hover === null || drawn.length === 0 ? null : (() => {
    const at = categories ? Math.round(x0 + ((hover - margin.start) / innerWidth) * (x1 - x0)) : null;
    const position = (hover - margin.start) / innerWidth;
    const target = at ?? (xLog ? Math.exp(Math.log(x0) + position * (Math.log(x1) - Math.log(x0))) : x0 + position * (x1 - x0));
    const hits = series.flatMap((line) => {
      const point = at === null ? nearest(line.points, target) : line.points.find(([px]) => px === at);
      return point && point[1] !== null && (at !== null || Math.abs(x(point[0]) - hover) <= 40) ? [{ line, point }] : [];
    });
    if (hits.length === 0) return null;
    const mine = hits.filter((hit) => !hit.line.ghost);
    const anchor = (mine.length > 0 ? mine : hits).reduce((best, hit) => Math.abs(x(hit.point[0]) - hover) < Math.abs(x(best.point[0]) - hover) ? hit : best);
    return { hits, anchor };
  })();
  const readoutTitle = (anchor: { line: Series; point: Point }) => {
    if (categories) return categories[anchor.point[0]];
    if (!table) return `${monitor.x.name} ${xLabel(anchor.point[0])}`;
    return `${xName(anchor.line)} ${withUnit(signals[anchor.line.x], anchor.point[0])}`;
  };
  const readoutValue = (line: Series, point: Point, anchor: Point) => {
    if (!table) return fmtSignal(signals[line.y], point[1]);
    const value = withUnit(signals[line.y], point[1]);
    return categories || point[0] === anchor[0] ? value : m.run_chart_at({ value, x: withUnit(signals[line.x], point[0]) });
  };

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

  const onMove = (event: MouseEvent<HTMLDivElement>) => {
    const position = event.clientX - event.currentTarget.getBoundingClientRect().left;
    setHover(position >= margin.start && position <= margin.start + innerWidth ? position : null);
  };

  const chart = (
    <div ref={ref} dir="ltr" className="relative w-full" style={{ height: boxHeight }} onMouseMove={onMove} onMouseLeave={() => setHover(null)}>
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
          {drawn.length > 0 && categories?.map((word, index) => index % every === 0 && (
            <text key={index} x={x(index)} y={boxHeight - 6} textAnchor="middle" fill="var(--subtext)">{word}</text>
          ))}
          {!categories && <text x={margin.start + innerWidth + margin.end - 2} y={boxHeight - 6} textAnchor="end" fill="var(--subtext)">{xCaption}</text>}
          {blank && <text x={margin.start + innerWidth / 2} y={margin.top + innerHeight / 2} textAnchor="middle" fill="var(--subtext)">{empty}</text>}
          {mark === "bars" && bars(false)}
          {mark === "points" && series.map((line) => <g key={line.name}>{points(line, 3.5)}</g>)}
          {mark === "line" && series.map((line) => line.points.length > 0 && (
            <g key={line.name}>
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
              {dotted(line) && line.points.map(([px, py]) => py !== null && <circle key={px} cx={x(px)} cy={y(py)} r={table ? 3 : 4} fill={line.color} stroke="var(--base)" strokeWidth={table ? 1.5 : 2} />)}
            </g>
          ))}
          {readout && <line x1={hover!} x2={hover!} y1={margin.top} y2={margin.top + innerHeight} stroke="var(--border)" />}
          {mark !== "bars" && readout?.hits.map(({ line, point }) => <circle key={line.name} cx={x(point[0])} cy={y(point[1]!)} r="4" fill={line.color} stroke="var(--base)" strokeWidth="2" />)}
          {endLabels.map((end) => (
            <text key={end.line.name} x={Math.min(end.x + 7, margin.start + innerWidth + 7)} y={end.y + 4} fill={end.line.ghost ? "var(--subtext)" : "var(--text)"} fontWeight={end.line.ghost ? 400 : 500}>
              {end.line.ghost ? m.run_chart_parent_end() : fmtSignal(signals[end.line.y], end.value)}
            </text>
          ))}
      </svg>
      {readout && (
        <div
          aria-hidden="true"
          className="pointer-events-none absolute top-2 z-10 flex min-w-36 flex-col gap-1 rounded-md border border-border bg-background px-2.5 py-2 text-xs text-ink-subtext shadow-popover"
          style={hover! > width / 2 ? { right: width - hover! + 10 } : { left: hover! + 10 }}
        >
          <b className="font-medium text-text">{readoutTitle(readout.anchor)}</b>
          {readout.hits.map(({ line, point }) => (
            <span key={line.name} className="flex items-center gap-1.5">
              <svg width="12" height="2" className="shrink-0"><line x1="0" x2="12" y1="1" y2="1" stroke={line.color} strokeWidth="2" strokeDasharray={line.ghost ? "3 2" : undefined} /></svg>
              {!line.ghost ? line.label : series.filter((one) => one.ghost).length > 1 ? m.run_chart_parent_line({ label: line.label }) : m.run_chart_parent()}
              <b className="ms-auto ps-3 font-medium tabular-nums text-text">{readoutValue(line, point, readout.anchor.point)}</b>
            </span>
          ))}
        </div>
      )}
    </div>
  );
  if (shownNotes.length === 0) return chart;
  return (
    <>
      {chart}
      <p className="m-0 mt-1.5 text-xs text-ink-subtext">{shownNotes.join(" ")}</p>
    </>
  );
}
