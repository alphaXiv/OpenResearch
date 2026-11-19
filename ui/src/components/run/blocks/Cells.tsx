import { fmtNumber, ltr } from "../../../i18n";
import { m } from "../../../paraglide/messages.js";
import { getLocale } from "../../../paraglide/runtime.js";
import type { SignalValue } from "../../../runMonitor/types";
import { useBoxSize } from "../useBoxSize";

type Outcome = "correct" | "wrong" | "error" | "unknown";

function Mark({ outcome, x, y, size }: { outcome: Outcome; x: number; y: number; size: number }) {
  switch (outcome) {
    case "correct":
      return <circle cx={x + size / 2} cy={y + size / 2} r={size / 2} fill="var(--mark-blue)" />;
    case "wrong":
      return <rect x={x} y={y} width={size} height={size} rx="1" fill="var(--subtext)" />;
    case "error":
      return <path d={`M${x + size / 2} ${y} L${x + size} ${y + size} H${x} Z`} fill="var(--mark-amber)" />;
    case "unknown":
      return <rect x={x + 0.5} y={y + 0.5} width={size - 1} height={size - 1} rx="1" fill="none" stroke="var(--subtext)" />;
  }
}

export function Cells({ rows, size, live }: { rows: Record<string, SignalValue>[]; size: "strip" | "page"; live: boolean }) {
  const [ref, { width }] = useBoxSize<HTMLDivElement>();
  if (rows.length === 0) {
    return <div ref={ref} className="w-full"><p className="m-0 text-xs text-ink-subtext">{live ? m.run_cells_none() : m.run_cells_none_ended()}</p></div>;
  }

  const tally = { correct: 0, wrong: 0, error: 0, unknown: 0 };
  const outcomes = rows.slice(-256).map((row): Outcome => {
    const value = row.outcome;
    const outcome = value === "correct" || value === "wrong" || value === "error" ? value : "unknown";
    tally[outcome]++;
    return outcome;
  });
  const plurals = new Intl.PluralRules(getLocale());
  const legend = ([
    ["correct", m.run_cells_correct_one, m.run_cells_correct_other],
    ["wrong", m.run_cells_wrong_one, m.run_cells_wrong_other],
    ["error", m.run_cells_errors_one, m.run_cells_errors_other],
    ["unknown", m.run_cells_unknown_one, m.run_cells_unknown_other],
  ] as const).flatMap(([outcome, one, other]) => {
    const n = tally[outcome];
    return n ? [{ outcome, text: (plurals.select(n) === "one" ? one : other)({ count: ltr(fmtNumber(n)) }) }] : [];
  });

  const cell = size === "page" ? 12 : 8;
  const columns = Math.max(1, Math.min(outcomes.length, Math.floor(width / cell)));
  const scope = m.run_cells_recent({ count: ltr(fmtNumber(outcomes.length)) });
  const counts = new Intl.ListFormat(getLocale(), { type: "unit" }).format(legend.map((part) => part.text));
  return (
    <div ref={ref} className="w-full">
      <div dir="ltr">
        <svg width={columns * cell} height={Math.ceil(outcomes.length / columns) * cell} role="img" aria-label={`${scope}. ${counts}`} className="block">
          {outcomes.map((outcome, index) => <Mark key={index} outcome={outcome} x={(index % columns) * cell} y={Math.floor(index / columns) * cell} size={cell - 2} />)}
        </svg>
      </div>
      {size === "page" && (
        <div aria-hidden="true" className="mt-2.5 text-xs text-ink-subtext">
          <p className="m-0 mb-1.5">{scope}</p>
          <div className="flex flex-wrap gap-x-4 gap-y-1">
            {legend.map((part) => (
              <span key={part.outcome} className="inline-flex items-center gap-1.5">
                <svg width="10" height="10" className="shrink-0"><Mark outcome={part.outcome} x={0} y={0} size={10} /></svg>
                {part.text}
              </span>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
