import { useVirtualizer } from "@tanstack/react-virtual";
import {
  columnFilteringFeature,
  columnResizingFeature,
  columnSizingFeature,
  createFilteredRowModel,
  createSortedRowModel,
  globalFilteringFeature,
  rowSortingFeature,
  sortFn_alphanumeric,
  sortFn_basic,
  tableFeatures,
  useTable,
  type ColumnDef,
} from "@tanstack/react-table";
import { ArrowDown, ArrowUp, Search } from "lucide-react";
import { useDeferredValue, useLayoutEffect, useMemo, useRef, useState } from "react";
import { detectDelimiter, parseCsv } from "../csv";
import { m } from "../paraglide/messages.js";
import type { FileScrollPosition } from "./FileViewer";
import { Input } from "./ui";

const HEADER_HEIGHT = 32;
const ROW_HEIGHT = 28;
const MIN_COLUMN_WIDTH = 64;
const MAX_INITIAL_COLUMN_WIDTH = 360;
const CHAR_WIDTH = 8;
const NUMBER = /^[+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?$/i;

const features = tableFeatures({
  columnSizingFeature,
  columnResizingFeature,
  rowSortingFeature,
  sortedRowModel: createSortedRowModel(),
  sortFns: { alphanumeric: sortFn_alphanumeric, basic: sortFn_basic },
  columnFilteringFeature,
  globalFilteringFeature,
  filteredRowModel: createFilteredRowModel(),
});

type Row = string[];

/** Matches against the source text, so "1.50" is found as typed rather than as 1.5. */
const includesCell = (row: { original: Row }, columnId: string, query: string) =>
  row.original[Number(columnId)]?.toLowerCase().includes(query) ?? false;

function buildSchema(rows: Row[]) {
  const [head = [], ...body] = rows;
  const columnCount = rows.reduce((width, row) => Math.max(width, row.length), 0);

  const numeric = Array.from({ length: columnCount }, (_, i) => {
    const values = body.map((row) => row[i]?.trim()).filter(Boolean);
    return values.length > 0 && values.every((value) => NUMBER.test(value!));
  });
  const labels = Array.from({ length: columnCount }, (_, i) => head[i] || `#${i + 1}`);
  const widths = labels.map((label, i) => {
    const longest = body.reduce((max, row) => Math.max(max, row[i]?.length ?? 0), label.length);
    return Math.min(Math.max(Math.round(longest * CHAR_WIDTH) + 44, MIN_COLUMN_WIDTH), MAX_INITIAL_COLUMN_WIDTH);
  });

  const columns: ColumnDef<typeof features, Row>[] = labels.map((_, i) => ({
    id: String(i),
    accessorFn: numeric[i]
      ? (row) => {
          const value = row[i]?.trim();
          return value ? Number(value) : undefined;
        }
      : (row) => row[i] || undefined,
    size: widths[i],
    minSize: MIN_COLUMN_WIDTH,
    sortFn: numeric[i] ? "basic" : "alphanumeric",
    sortDescFirst: false,
    sortUndefined: "last",
  }));

  return { body, columns, labels, numeric };
}

export function CsvTable({
  text,
  path,
  truncated,
  scrollPosition,
  onScrollPositionChange,
}: {
  text: string;
  path: string;
  /** The text ends mid-file, so its last record may be cut off. */
  truncated: boolean;
  scrollPosition?: FileScrollPosition;
  onScrollPositionChange?: (position: FileScrollPosition) => void;
}) {
  const { body, columns, labels, numeric } = useMemo(() => {
    const rows = parseCsv(text, detectDelimiter(path, text));
    return buildSchema(truncated ? rows.slice(0, -1) : rows);
  }, [text, path, truncated]);

  const [query, setQuery] = useState("");
  const globalFilter = useDeferredValue(query.trim().toLowerCase());

  const table = useTable({
    features,
    columns,
    data: body,
    columnResizeMode: "onChange",
    globalFilterFn: includesCell,
    getColumnCanGlobalFilter: () => true,
    state: { globalFilter },
  });

  const rows = table.getRowModel().rows;
  const leafColumns = table.getAllLeafColumns();
  const gutterWidth = Math.max(String(body.length).length * 8 + 20, 44);
  const totalWidth = gutterWidth + table.getTotalSize();

  const scrollRef = useRef<HTMLDivElement>(null);
  const initialScroll = useRef(scrollPosition);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    getItemKey: (index) => rows[index].id,
    estimateSize: () => ROW_HEIGHT,
    scrollMargin: HEADER_HEIGHT,
    initialOffset: initialScroll.current?.top,
    overscan: 12,
  });

  useLayoutEffect(() => {
    const scroller = scrollRef.current;
    const position = initialScroll.current;
    if (!scroller || !position) return;
    scroller.scrollTop = position.top;
    scroller.scrollLeft = position.left;
  }, []);

  const fileName = path.split("/").pop() ?? path;
  const summary = globalFilter
    ? m.csv_table_summary_filtered({
        shown: rows.length.toLocaleString(),
        rows: body.length.toLocaleString(),
        columns: columns.length.toLocaleString(),
      })
    : m.csv_table_summary({
        rows: body.length.toLocaleString(),
        columns: columns.length.toLocaleString(),
      });

  return (
    <div className="csv-table flex flex-col h-full min-h-0">
      <div className="csv-table-toolbar flex shrink-0 items-center gap-3 px-4 py-1.5 border-b border-border-variant">
        <div className="relative w-full max-w-60">
          <Search size={14} className="pointer-events-none absolute start-2.5 top-1/2 -translate-y-1/2 text-muted" />
          <Input
            value={query}
            placeholder={m.csv_table_filter_placeholder()}
            aria-label={m.csv_table_filter_placeholder()}
            className="ps-8"
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape" && query) {
                event.stopPropagation();
                setQuery("");
              }
            }}
          />
        </div>
        <span className="ms-auto shrink-0 text-xs text-muted tabular-nums">{summary}</span>
      </div>
      {truncated && (
        <div className="file-view-note shrink-0 border-b border-b-border-variant py-2 px-4 text-sm text-muted">
          {m.file_viewer_file_truncated_showing_the_first_512_kb()}
        </div>
      )}
      <div
        ref={scrollRef}
        dir="ltr"
        role="region"
        aria-label={fileName}
        tabIndex={0}
        className="csv-table-scroll flex-1 min-h-0 overflow-auto outline-none"
        onScroll={(event) =>
          onScrollPositionChange?.({
            top: Math.max(0, event.currentTarget.scrollTop),
            left: Math.max(0, event.currentTarget.scrollLeft),
          })
        }
      >
        <div
          role="table"
          aria-label={fileName}
          aria-rowcount={rows.length + 1}
          aria-colcount={columns.length}
          className="relative min-w-full text-sm text-text"
          style={{ width: totalWidth }}
        >
          <div role="rowgroup" className="sticky top-0 z-10">
            <div role="row" className="flex bg-background border-b border-border" style={{ height: HEADER_HEIGHT }}>
              <div role="columnheader" className="sticky left-0 z-10 shrink-0 bg-background border-e border-border-variant" style={{ width: gutterWidth }} />
              {table.getHeaderGroups()[0]?.headers.map((header) => {
                const column = header.column;
                const index = Number(column.id);
                const sorted = column.getIsSorted();
                return (
                  <div
                    key={header.id}
                    role="columnheader"
                    aria-sort={sorted === "asc" ? "ascending" : sorted === "desc" ? "descending" : undefined}
                    className="group relative shrink-0"
                    style={{ width: column.getSize() }}
                  >
                    <button
                      type="button"
                      className={`flex h-full w-full items-center gap-1 px-3 text-xs font-medium text-subtext hover:text-text hover:bg-hover-faint cursor-pointer ${numeric[index] ? "flex-row-reverse" : ""}`}
                      onClick={column.getToggleSortingHandler()}
                    >
                      <span className="min-w-0 truncate">{labels[index]}</span>
                      {sorted === "desc" ? (
                        <ArrowDown size={12} className="shrink-0" />
                      ) : (
                        <ArrowUp size={12} className={`shrink-0 ${sorted ? "" : "opacity-0 group-hover:opacity-40"}`} />
                      )}
                    </button>
                    <div
                      aria-hidden="true"
                      className={`absolute right-0 top-0 h-full w-1.5 cursor-col-resize touch-none select-none hover:bg-resizer-hover ${column.getIsResizing() ? "bg-resizer-hover" : ""}`}
                      onMouseDown={header.getResizeHandler()}
                      onTouchStart={header.getResizeHandler()}
                      onDoubleClick={() => column.resetSize()}
                    />
                  </div>
                );
              })}
            </div>
          </div>
          <div role="rowgroup" className="relative" style={{ height: virtualizer.getTotalSize() }}>
            {virtualizer.getVirtualItems().map((item) => {
              const row = rows[item.index];
              return (
                <div
                  key={row.id}
                  role="row"
                  aria-rowindex={item.index + 2}
                  className={`absolute left-0 flex hover:bg-hover-muted ${item.index % 2 ? "bg-hover-faint" : ""}`}
                  style={{ top: item.start - HEADER_HEIGHT, height: ROW_HEIGHT, width: totalWidth }}
                >
                  <div role="rowheader" className="sticky left-0 z-[1] shrink-0 bg-background border-e border-border-variant pe-2 text-end text-xs leading-7 text-muted tabular-nums select-none" style={{ width: gutterWidth }}>
                    {Number(row.id) + 1}
                  </div>
                  {leafColumns.map((column) => {
                    const value = row.original[Number(column.id)] ?? "";
                    return (
                      <div
                        key={column.id}
                        role="cell"
                        title={value}
                        className={`shrink-0 truncate px-3 leading-7 ${numeric[Number(column.id)] ? "text-end tabular-nums" : ""}`}
                        style={{ width: column.getSize() }}
                      >
                        {value}
                      </div>
                    );
                  })}
                </div>
              );
            })}
          </div>
          {rows.length === 0 && (
            <div className="py-6 text-center text-sm text-muted">
              {body.length === 0 ? m.csv_table_no_rows() : m.csv_table_no_matches()}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
