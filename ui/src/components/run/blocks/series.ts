import { m } from "../../../paraglide/messages.js";
import { getLocale } from "../../../paraglide/runtime.js";
import { fmtSignal } from "../../../runMonitor/format";
import type { ChartBlock, Point, RunMonitor, SignalValue } from "../../../runMonitor/types";

const COLORS = ["var(--mark-blue)", "var(--accent-orange)", "var(--accent-purple)", "var(--accent-teal)", "var(--accent-pink)", "var(--accent-amber)", "var(--accent-indigo)", "var(--accent-green)"];

/** The most lines a chart draws; `by` can name more, and the rest are counted. */
const MOST = 8;

export interface Series {
  name: string;
  label: string;
  color: string;
  points: Point[];
  ghost: boolean;
  /** The signals or columns this line plots, for their units and digits. */
  x: string;
  y: string;
}

export interface ChartData {
  series: Series[];
  /** A bars chart's x values in order, worded; each point's x is its index here. */
  categories: string[] | null;
  /** What a reader can't see in the picture and why, worded. */
  notes: string[];
  /** How many rows the table a chart draws from has; null when it draws signals over steps. */
  rows: number | null;
}

type Row = Record<string, SignalValue>;
/** A line before its cells are read; `by` is the value of the `by` column its rows share. */
type Shape = { name: string; label: string; x: string; y: string; by?: SignalValue };
type Line = Omit<Series, "points"> & { pairs: [SignalValue, SignalValue][] };

/** A cell as a number: a numeric string counts, a word or NaN does not. */
function numberOf(value: SignalValue | undefined): number | null {
  const number = typeof value === "number" ? value : typeof value === "string" && value.trim() !== "" ? Number(value) : NaN;
  return Number.isFinite(number) ? number : null;
}

const plural = (count: number, one: (inputs: { count: string }) => string, other: (inputs: { count: string }) => string) =>
  (new Intl.PluralRules(getLocale()).select(count) === "one" ? one : other)({ count: new Intl.NumberFormat(getLocale()).format(count) });

/** Each line a table chart draws: explicit series, one per value of `by` and y, or one per y. */
function shapesOf(monitor: RunMonitor, block: ChartBlock, rows: Row[], notes: string[]): Shape[] {
  const { signals } = monitor.view;
  const label = (name: string) => signals[name]?.label ?? name;
  const x = block.x ?? "step";
  if (block.series?.length) return block.series.map((one) => ({ name: `${one.x} ${one.y}`, label: one.label ?? label(one.y), x: one.x, y: one.y }));
  if (!block.by) return block.chart.map((y) => ({ name: y, label: label(y), x, y }));

  const by = block.by;
  const word = (value: SignalValue) => typeof value === "number"
    ? m.run_chart_by_value({ label: label(by), value: fmtSignal(signals[by], value) })
    : fmtSignal(signals[by], value);
  const shapes = [...new Set(rows.map((row) => row[by] ?? null))].flatMap((value) => block.chart.map((y) => ({
    name: `${String(value)} ${y}`,
    label: block.chart.length > 1 ? m.run_chart_by_measure({ value: word(value), label: label(y) }) : word(value),
    x,
    y,
    by: value,
  })));
  if (shapes.length > MOST) notes.push(m.run_chart_series_capped({ shown: String(MOST), count: String(shapes.length) }));
  return shapes.slice(0, MOST);
}

/** The cells each line reads from a table's rows; a `by` line reads only its own rows. */
const pairsOf = (shape: Shape, rows: Row[], by: string | undefined): [SignalValue, SignalValue][] =>
  rows.filter((row) => by === undefined || !("by" in shape) || (row[by] ?? null) === shape.by).map((row) => [row[shape.x] ?? null, row[shape.y] ?? null]);

/** Each category of a bars chart once, numbers in order and words as they first came, with their words. */
function categoriesOf(lines: Line[], monitor: RunMonitor) {
  const values = lines.flatMap((line) => line.pairs.map(([x]) => x)).filter((x) => x !== null);
  const unique = [...new Map(values.map((x) => [String(x), x])).values()];
  if (unique.every((x) => numberOf(x) !== null)) unique.sort((a, b) => numberOf(a)! - numberOf(b)!);
  const spec = monitor.view.signals[lines.find((line) => !line.ghost)?.x ?? ""];
  return { keys: unique.map(String), words: unique.map((value) => fmtSignal(spec, value)) };
}

/** What a chart block draws: its lines, scaled and marked as the view asks, and what was left out of them. */
export function chartOf(monitor: RunMonitor, block: ChartBlock): ChartData {
  const { signals } = monitor.view;
  const notes: string[] = [];
  /** Columns no row gives as a number, already said; their cells aren't counted again. */
  const broken = new Set<string>();
  const bars = block.mark === "bars";
  let lines: Line[];
  let count: number | null = null;

  if (block.from === undefined) {
    lines = block.chart.map((name, index) => {
      const smooth = signals[name]?.smooth;
      let level: number | null = null;
      const pairs = (monitor.series[name] ?? []).map(([x, y]): [number, number | null] => {
        if (!smooth || y === null) return [x, y];
        level = level === null ? y : level + smooth * (y - level);
        return [x, level];
      });
      return { name, label: signals[name]?.label ?? name, color: COLORS[index % COLORS.length], pairs, ghost: false, x: monitor.x.name, y: name };
    });
    const first = block.chart[0];
    const parent = block.ghost === "parent" ? monitor.parent?.series[first] : undefined;
    if (parent) lines.unshift({ name: "parent", label: "", color: "var(--subtext)", pairs: parent, ghost: true, x: monitor.x.name, y: first });
  } else {
    const from = block.from;
    const rows = monitor.rows[from] ?? [];
    count = rows.length;
    const shapes = shapesOf(monitor, block, rows, notes);
    lines = shapes.map((shape, index) => ({ name: shape.name, label: shape.label, x: shape.x, y: shape.y, color: COLORS[index % COLORS.length], ghost: false, pairs: pairsOf(shape, rows, block.by) }));
    const parentRows = block.ghost === "parent" ? monitor.parent?.rows?.[from] ?? [] : [];
    if (parentRows.length > 0) {
      lines.unshift(...shapes.map((shape) => ({ name: `parent ${shape.name}`, label: "", x: shape.x, y: shape.y, color: "var(--subtext)", ghost: true, pairs: pairsOf(shape, parentRows, block.by) })));
    }
    for (const column of new Set(shapes.flatMap((shape) => [shape.x, shape.y]))) {
      const cells = rows.filter((row) => column in row);
      if (rows.length > 0 && cells.length === 0) notes.push(m.run_chart_no_column({ table: from, column }));
      else if (cells.length > 0 && !(bars && shapes.some((shape) => shape.x === column)) && cells.every((row) => numberOf(row[column]) === null)) notes.push(m.run_chart_not_number({ table: from, column }));
      else continue;
      broken.add(column);
    }
  }

  const categories = bars ? categoriesOf(lines, monitor) : null;
  const counts = block.from !== undefined;
  let leftOut = 0;
  let offScale = 0;
  const series = lines.map(({ pairs, ...line }): Series => {
    const own = !line.ghost;
    const points: Point[] = [];
    for (const [rawX, rawY] of pairs) {
      const index = categories && rawX !== null ? categories.keys.indexOf(String(rawX)) : -1;
      const x = categories ? (index < 0 ? null : index) : numberOf(rawX);
      let y = numberOf(rawY);
      if (x === null) {
        if (counts && own && !broken.has(line.x)) leftOut++;
        continue;
      }
      if (!categories && block.xScale === "log" && x <= 0) {
        if (own) offScale++;
        continue;
      }
      if (y === null && counts && own && !broken.has(line.y)) leftOut++;
      if (y !== null && block.yScale === "log" && y <= 0) {
        if (own) offScale++;
        y = null;
      }
      points.push([x, y]);
    }
    if (counts) points.sort((a, b) => a[0] - b[0]);
    return { ...line, points };
  });
  if (leftOut > 0) notes.push(plural(leftOut, m.run_chart_left_out_one, m.run_chart_left_out_other));
  if (offScale > 0) notes.push(plural(offScale, m.run_chart_off_log_one, m.run_chart_off_log_other));
  return { series, categories: categories?.words ?? null, notes, rows: count };
}

/** A chart block's lines, smoothed as the view asks, with the parent's dashed behind. */
export const seriesOf = (monitor: RunMonitor, block: ChartBlock) => chartOf(monitor, block).series;
