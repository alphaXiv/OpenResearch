import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowUp } from "lucide-react";
import { memo, useEffect, useRef, useState } from "react";
import { autoDir, fmtNumber, ltr } from "../../../i18n";
import { m } from "../../../paraglide/messages.js";
import { getLocale } from "../../../paraglide/runtime.js";
import { fmtMagnitude, fmtSignal, labelOf, versusOf } from "../../../runMonitor/format";
import type { RunMonitor, SignalValue, Signals } from "../../../runMonitor/types";
import { cn } from "../../ui/cn";
import { Delta } from "../Delta";

type Row = Record<string, SignalValue>;
type TableBlock = { table: string; columns: string[]; last?: number; vs?: string };
/**
 * A row as the table draws it: `parent` is the parent's value of the compared column, `pending` a row the run hasn't
 * reached, and `id` the object whose identity keys it.
 */
type Line = { id: object; row: Row; parent: SignalValue; pending: boolean };

/** Rows a page shows when the view doesn't say. */
const NEWEST = 10;

/** Rows keep their identity while events append and slide them, so each keeps one key and redraws only when new. */
const rowKeys = new WeakMap<object, number>();
let nextKey = 0;
const keyOf = (row: object) => {
  let key = rowKeys.get(row);
  if (key === undefined) rowKeys.set(row, key = nextKey++);
  return key;
};

function cellText(signals: Signals, column: string, value: SignalValue | undefined) {
  const word = labelOf(signals[column], value);
  if (word !== undefined) return autoDir(word);
  return typeof value === "string" && !["NaN", "Infinity", "-Infinity"].includes(value) ? autoDir(value) : ltr(fmtSignal(signals[column], value));
}

const count = (value: number) => ltr(fmtNumber(value));

/** With `vs: "parent"`, every row the parent has, by its first column, with this run's value beside the parent's. */
function compared(rows: Row[], parentRows: Row[], key: string, column: string): Line[] {
  const own = new Map(rows.map((row) => [row[key], row]));
  const lines: Line[] = parentRows.map((parent) => {
    const row = own.get(parent[key]);
    return { id: row ?? parent, row: row ?? { [key]: parent[key] }, parent: parent[column] ?? null, pending: !row };
  });
  const seen = new Set(parentRows.map((parent) => parent[key]));
  return [...lines, ...rows.filter((row) => !seen.has(row[key])).map((row) => ({ id: row, row, parent: null, pending: false }))];
}

/** How many of `rows` came after `newest`, matched by value, since a reread brings the same rows as new objects. */
function arrivedAfter(rows: Row[], newest: Row | undefined): number {
  if (!newest) return rows.length;
  const held = JSON.stringify(newest);
  for (let index = rows.length - 1; index >= 0; index--) {
    if (rows[index] === newest || JSON.stringify(rows[index]) === held) return rows.length - 1 - index;
  }
  return rows.length;
}

type Columns = { columns: string[]; signals: Signals; vs: boolean };

const PageRow = memo(function PageRow({ row, parent, pending, index, columns, signals, vs, measure }: Line & Columns & { index?: number; measure?: (row: HTMLTableRowElement | null) => void }) {
  const compare = columns[1];
  const value = row[compare];
  const change = vs && typeof value === "number" && typeof parent === "number" ? versusOf(signals[compare], value - parent) : null;
  return (
    <tr ref={measure} className="border-t border-border-variant" data-index={index} aria-rowindex={index === undefined ? undefined : index + 2}>
      {columns.map((column, at) => (
        <td key={column} className={cn("py-2 [overflow-wrap:anywhere]", at ? "ps-3 text-end tabular-nums" : "text-start", pending ? "text-ink-subtext" : "text-text")}>
          {cellText(signals, column, row[column])}
        </td>
      ))}
      {vs && <td className="py-2 ps-3 text-end tabular-nums text-ink-subtext">{cellText(signals, compare, parent)}</td>}
      {vs && (
        <td className="py-2 ps-3 text-end">
          {change && (change.flat
            ? <span className="text-ink-subtext">{m.run_level_with_parent()}</span>
            : <Delta delta={{ down: change.d < 0, good: change.good, flat: false, magnitude: ltr(fmtMagnitude(signals[compare], change.mag)) }} said className="justify-end" />)}
        </td>
      )}
    </tr>
  );
});

function Head({ columns, signals, vs }: Columns) {
  const label = (column: string) => signals[column]?.label ?? column;
  return (
    <thead className="sticky top-0 z-[1] bg-background">
      <tr className="text-xs text-ink-subtext" aria-rowindex={1}>
        {columns.map((column, index) => <th key={column} scope="col" className={cn("pb-1.5 font-normal", index ? "ps-3 text-end" : "text-start")}>{label(column)}</th>)}
        {vs && <th scope="col" className="pb-1.5 ps-3 text-end font-normal">{m.run_chart_parent()}</th>}
        {vs && <th scope="col" className="pb-1.5 ps-3 text-end font-normal">{m.run_table_change()}</th>}
      </tr>
    </thead>
  );
}

/** Height a row is guessed at before it is measured. */
const ROW_HEIGHT = 37;

type AllRowsProps = Columns & { lines: Line[]; rows: Row[]; title: string };

/**
 * Every row, in a box of its own. While the reader is away from the newest row nothing in the box moves: arrivals are
 * held and counted in a control that returns to them, in a box drawn afresh at the top.
 */
function AllRows(props: AllRowsProps) {
  const [returns, setReturns] = useState(0);
  return <RowBox key={returns} {...props} returned={returns > 0} onNewest={() => setReturns(returns + 1)} />;
}

function RowBox({ lines, rows, title, returned, onNewest, ...columns }: AllRowsProps & { returned: boolean; onNewest: () => void }) {
  const box = useRef<HTMLDivElement>(null);
  const [held, setHeld] = useState<{ lines: Line[]; newest: Row | undefined } | null>(null);
  const shown = held?.lines ?? lines;
  const arrived = held ? arrivedAfter(rows, held.newest) : 0;

  const virtualizer = useVirtualizer({
    useFlushSync: false,
    count: shown.length,
    getScrollElement: () => box.current,
    getItemKey: (index) => keyOf(shown[index].id),
    estimateSize: () => ROW_HEIGHT,
    initialRect: { width: 0, height: 384 },
    overscan: 8,
  });
  const items = virtualizer.getVirtualItems();
  const before = items.length > 0 ? items[0].start : 0;
  const after = items.length > 0 ? virtualizer.getTotalSize() - items[items.length - 1].end : 0;

  const onScroll = () => {
    const away = (box.current?.scrollTop ?? 0) > 1;
    if (away && !held) setHeld({ lines, newest: rows.at(-1) });
    if (!away && held) setHeld(null);
  };
  useEffect(() => {
    if (returned) box.current?.focus();
  }, []);

  return (
    <div className="relative">
      <div
        ref={box}
        role="region"
        aria-label={m.run_table_every_row({ title: autoDir(title) })}
        tabIndex={0}
        onScroll={onScroll}
        className="max-h-96 overflow-y-auto rounded-sm [overflow-anchor:none] focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus"
      >
        <table className="w-full table-fixed border-collapse text-sm" aria-rowcount={shown.length + 1}>
          <Head {...columns} />
          <tbody>
            {before > 0 && <tr aria-hidden="true" style={{ height: before }} />}
            {items.map((item) => <PageRow key={item.key} {...shown[item.index]} {...columns} index={item.index} measure={virtualizer.measureElement} />)}
            {after > 0 && <tr aria-hidden="true" style={{ height: after }} />}
          </tbody>
        </table>
      </div>
      {arrived > 0 && (
        <button
          type="button"
          onClick={onNewest}
          className="absolute start-1/2 top-8 z-[2] inline-flex -translate-x-1/2 items-center gap-1 rounded-full border border-border bg-background px-2.5 py-1 text-xs text-text shadow-popover hover:bg-surface focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus rtl:translate-x-1/2"
        >
          <ArrowUp size={12} aria-hidden="true" />
          {(new Intl.PluralRules(getLocale()).select(arrived) === "one" ? m.run_table_new_rows_one : m.run_table_new_rows_other)({ count: count(arrived) })}
        </button>
      )}
    </div>
  );
}

/**
 * A table of the rows a run reported. A strip shows the newest two and the first three columns. A page shows the
 * newest `last`, newest first, and all of them on request; against the parent, the parent's rows in its order.
 */
export function Table({ monitor, block, size, live, title }: { monitor: RunMonitor; block: TableBlock; size: "strip" | "page"; live: boolean; title: string }) {
  const [all, setAll] = useState(false);
  const { signals } = monitor.view;
  const rows = monitor.rows[block.table] ?? [];
  const parentRows = block.vs === "parent" ? monitor.parent?.rows?.[block.table] ?? null : null;
  const vs = parentRows !== null;
  const lines: Line[] = vs ? compared(rows, parentRows, block.columns[0], block.columns[1])
    : rows.map((row) => ({ id: row, row, parent: null, pending: false })).reverse();

  if (lines.length === 0) return <p className="m-0 text-xs text-ink-subtext">{live ? m.run_block_no_rows() : m.run_block_no_rows_ended()}</p>;

  if (size === "strip") {
    const columns = block.columns.slice(0, 3);
    return (
      <table className="w-full table-fixed border-collapse text-xs">
        <thead>
          <tr className="text-ink-subtext">
            {columns.map((column, index) => <th key={column} scope="col" className={cn("truncate pb-1 font-normal", index ? "ps-2 text-end" : "text-start")}>{signals[column]?.label ?? column}</th>)}
            {vs && <th scope="col" className="pb-1 ps-2 text-end font-normal">{m.run_chart_parent()}</th>}
          </tr>
        </thead>
        <tbody>
          {lines.slice(0, 2).map((line) => (
            <tr key={keyOf(line.id)}>
              {columns.map((column, index) => (
                <td key={column} className={cn("truncate py-0.5", index ? "ps-2 text-end tabular-nums" : "text-start", line.pending ? "text-ink-subtext" : "text-text")}>{cellText(signals, column, line.row[column])}</td>
              ))}
              {vs && <td className="truncate py-0.5 ps-2 text-end tabular-nums text-ink-subtext">{cellText(signals, columns[1], line.parent)}</td>}
            </tr>
          ))}
        </tbody>
      </table>
    );
  }

  const newest = Math.max(1, block.last ?? NEWEST);
  const columns = { columns: block.columns, signals, vs };
  return (
    <div>
      {all ? <AllRows lines={lines} rows={rows} title={title} {...columns} /> : (
        <table className="w-full table-fixed border-collapse text-sm">
          <Head {...columns} />
          <tbody>
            {lines.slice(0, newest).map((line) => <PageRow key={keyOf(line.id)} {...line} {...columns} />)}
          </tbody>
        </table>
      )}
      {lines.length > newest && (
        <button
          type="button"
          aria-expanded={all}
          onClick={() => setAll(!all)}
          className="tool-target mt-2 rounded-sm text-xs text-ink-subtext hover:text-text focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus"
        >
          {all ? m.run_table_show_newest({ count: count(newest) }) : m.run_table_show_all({ count: count(lines.length) })}
        </button>
      )}
    </div>
  );
}
