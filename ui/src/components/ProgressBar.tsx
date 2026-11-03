import type { ReactNode } from "react";

export function ProgressBar({
  value,
  max = 100,
  label,
  caption,
  fillColor,
}: {
  value?: number;
  max?: number;
  /** Left caption; defaults to the computed percent. */
  label?: string;
  /** Right caption (e.g. a byte or token detail). */
  caption?: ReactNode;
  fillColor?: string;
}) {
  const pct = value === undefined
    ? undefined
    : max > 0 ? Math.min(100, Math.round((value / max) * 100)) : 0;
  const animation = pct === undefined
    ? "motion-safe:animate-[progress_1.2s_ease-in-out_infinite_alternate]"
    : "transition-[width] duration-200 ease-standard";
  return (
    <div className="progress mt-3 mx-0 mb-1" role="progressbar" aria-label={label} aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
      <div className="progress-track h-2 rounded-full bg-panel border border-border overflow-hidden">
        <div
          className={`progress-fill h-full bg-subtext rounded-full ${animation}`}
          style={{ width: pct === undefined ? "35%" : `${pct}%`, background: fillColor }}
        />
      </div>
      {(label !== undefined || caption !== undefined) && (
        <div className="progress-caption flex justify-between mt-1.5 text-sm text-subtext">
          <span>{label ?? (pct === undefined ? null : `${pct}%`)}</span>
          {caption}
        </div>
      )}
    </div>
  );
}
