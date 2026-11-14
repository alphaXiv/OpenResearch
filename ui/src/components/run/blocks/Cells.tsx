import { fmtNumber, ltr } from "../../../i18n";
import { m } from "../../../paraglide/messages.js";
import { getLocale } from "../../../paraglide/runtime.js";
import type { Cells as CellsData } from "../../../runMonitor/types";
import { useBoxSize } from "../useBoxSize";

type Outcome = 0 | 1 | 2 | null | "todo";

/**
 * One item's square at `x, y`, `size` wide. Correct, wrong and an error fill it; not known is its outline and still to
 * go a dot in its middle, so the three greys differ by shape as well as colour.
 */
function Mark({ outcome, x, y, size }: { outcome: Outcome; x: number; y: number; size: number }) {
  if (outcome === null) return <rect x={x + 0.5} y={y + 0.5} width={size - 1} height={size - 1} rx="1" fill="none" stroke="var(--subtext)" />;
  if (outcome === "todo") {
    const dot = Math.max(1.5, size / 3);
    return <rect x={x + (size - dot) / 2} y={y + (size - dot) / 2} width={dot} height={dot} rx={dot / 4} fill="var(--subtext)" />;
  }
  const fill = outcome === 1 ? "var(--mark-blue)" : outcome === 2 ? "var(--mark-amber)" : "var(--subtext)";
  return <rect x={x} y={y} width={size} height={size} rx="1.5" fill={fill} />;
}

const plural = (count: number, one: (inputs: { count: string }) => string, other: (inputs: { count: string }) => string) =>
  (new Intl.PluralRules(getLocale()).select(count) === "one" ? one : other)({ count: ltr(fmtNumber(count)) });

/**
 * One square per item in the order they finished: correct, wrong, an error, not known, or still to go. A page draws
 * every item; a strip the newest rows that fit.
 */
export function Cells({ cells, size, live }: { cells: CellsData | undefined; size: "strip" | "page"; live: boolean }) {
  const [ref, { width }] = useBoxSize<HTMLDivElement>();
  const finished = cells ? cells.from + cells.outcomes.length : 0;
  const all = cells ? Math.max(cells.of ?? finished, finished) : 0;
  if (!cells || all === 0) return <p className="m-0 text-xs text-ink-subtext">{live ? m.run_cells_none() : m.run_cells_none_ended()}</p>;

  const page = size === "page";
  const outcome = (index: number): Outcome => index < cells.from ? null : index < finished ? cells.outcomes[index - cells.from] : "todo";
  const cell = page ? Math.max(5, Math.floor(width / 60)) : 5;
  const columns = Math.max(1, Math.floor(width / cell));
  const rows = page ? Math.ceil(all / columns) : 5;
  const count = Math.min(all, columns * rows);
  // A strip ends on the row after the newest item, so the items still to go show beyond it.
  const first = page ? 0 : Math.max(0, Math.min(finished, all) - count + columns);

  const tally = { correct: 0, wrong: 0, errors: 0, unknown: cells.from };
  for (const value of cells.outcomes) {
    if (value === 1) tally.correct++;
    else if (value === 0) tally.wrong++;
    else if (value === 2) tally.errors++;
    else tally.unknown++;
  }
  const toGo = cells.of === null ? 0 : Math.max(0, cells.of - finished);
  const parts = ([
    [1, tally.correct, m.run_cells_correct_one, m.run_cells_correct_other],
    [0, tally.wrong, m.run_cells_wrong_one, m.run_cells_wrong_other],
    [2, tally.errors, m.run_cells_errors_one, m.run_cells_errors_other],
    [null, tally.unknown, m.run_cells_unknown_one, m.run_cells_unknown_other],
    ["todo", toGo, m.run_cells_to_go_one, m.run_cells_to_go_other],
  ] as const).flatMap(([outcome, n, one, other]) => n > 0 ? [{ outcome: outcome as Outcome, text: plural(n, one, other) }] : []);

  return (
    <div ref={ref} className="w-full">
      <div dir="ltr">
        <svg width={columns * cell} height={Math.ceil(count / columns) * cell} role="img" aria-label={parts.map((part) => part.text).join(", ")} className="block">
          {Array.from({ length: count }, (_, i) => (
            <Mark key={first + i} outcome={outcome(first + i)} x={(i % columns) * cell} y={Math.floor(i / columns) * cell} size={cell - 1} />
          ))}
        </svg>
      </div>
      {page && (
        <div aria-hidden="true" className="mt-2.5 flex flex-wrap gap-x-4 gap-y-1 text-xs text-ink-subtext">
          {parts.map((part) => (
            <span key={String(part.outcome)} className="inline-flex items-center gap-1.5">
              <svg width="10" height="10" className="shrink-0"><Mark outcome={part.outcome} x={0} y={0} size={10} /></svg>
              {part.text}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
