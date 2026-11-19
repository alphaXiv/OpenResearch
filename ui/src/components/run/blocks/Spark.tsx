import type { Point } from "../../../runMonitor/types";

/** A signal's recent course, with its latest value as a dot; nothing until two points exist. */
export function Spark({ points, width = 64, height = 20 }: { points: Point[]; width?: number; height?: number }) {
  const values = points.flatMap(([, y]) => y !== null && Number.isFinite(y) ? [y] : []);
  if (values.length < 2) return null;
  const count = Math.min(48, values.length);
  const picked = Array.from({ length: count }, (_, i) => values[Math.round((i * (values.length - 1)) / (count - 1))]);
  const lo = Math.min(...picked);
  const hi = Math.max(...picked);
  const xy = picked.map((value, i) => [2 + (i * (width - 6)) / (count - 1), hi > lo ? 3 + (height - 6) * (1 - (value - lo) / (hi - lo)) : height / 2]);
  const [endX, endY] = xy[count - 1];
  return (
    <svg width={width} height={height} aria-hidden="true" className="shrink-0 text-subtext">
      <polyline points={xy.map(([x, y]) => `${x.toFixed(1)},${y.toFixed(1)}`).join(" ")} fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" strokeLinecap="round" />
      <circle cx={endX} cy={endY} r="2.5" fill="currentColor" />
    </svg>
  );
}
