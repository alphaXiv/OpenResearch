import type { ReactNode } from "react";

/** Determinate progress bar. The caption row renders only when `label` or
 * `caption` is given (the context meter draws a bare bar; the data-dir move
 * card passes a byte caption). `fillColor` overrides the default accent. */
export function ProgressBar({
  value,
  max,
  label,
  caption,
  fillColor,
}: {
  value: number;
  max: number;
  /** Left caption; defaults to the computed percent. */
  label?: string;
  /** Right caption (e.g. a byte or token detail). */
  caption?: ReactNode;
  fillColor?: string;
}) {
  const pct = max > 0 ? Math.min(100, Math.round((value / max) * 100)) : 0;
  return (
    <div className="progress mt-3 mx-0 mb-1" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
      <div className="progress-track h-2 rounded-full bg-surface border border-border overflow-hidden">
        <div className="progress-fill h-full bg-accent rounded-full transition-[width] duration-200 ease-standard" style={{ width: `${pct}%`, background: fillColor }} />
      </div>
      {(label !== undefined || caption !== undefined) && (
        <div className="progress-caption flex justify-between mt-1.5 text-sm text-muted">
          <span>{label ?? `${pct}%`}</span>
          {caption}
        </div>
      )}
    </div>
  );
}
