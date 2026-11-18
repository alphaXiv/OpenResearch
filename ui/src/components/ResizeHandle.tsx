import { useLayoutEffect, useState } from "react";
import { m } from "../paraglide/messages.js";

const KEYBOARD_STEP = 24;

/**
 * A grip on the top edge of the nearest positioned ancestor; dragging up grows
 * it. `height` is the height the user chose, or null for the natural one.
 */
export function ResizeHandle({
  label,
  height,
  min,
  max = Number.POSITIVE_INFINITY,
  measure,
  onChange,
}: {
  label: string;
  height: number | null;
  min: number;
  max?: number;
  /** The height the box has right now. */
  measure: () => number;
  onChange: (height: number | null) => void;
}) {
  /** What the box measured when the grip last took focus or was reset, for a natural height. */
  const [measured, setMeasured] = useState<number | null>(null);
  useLayoutEffect(() => {
    if (height === null) setMeasured(measure());
  }, [height]);
  const resizeBy = (start: number, delta: number) => onChange(Math.round(Math.min(Math.max(start + delta, min), max)));

  return (
    <div
      role="separator"
      aria-orientation="horizontal"
      aria-label={label}
      aria-valuemin={min}
      aria-valuemax={Number.isFinite(max) ? Math.round(max) : undefined}
      aria-valuenow={Math.round(Math.min(height ?? measured ?? min, max))}
      title={m.chat_resize_composer_hint()}
      tabIndex={0}
      className="group/resize absolute inset-x-0 -top-1.5 z-3 flex h-3 cursor-row-resize touch-none items-center justify-center outline-none"
      onPointerDown={(event) => {
        event.preventDefault();
        event.currentTarget.setPointerCapture(event.pointerId);
        const startY = event.clientY;
        const start = measure();
        const move = (moveEvent: PointerEvent) => resizeBy(start, startY - moveEvent.clientY);
        const stop = () => {
          window.removeEventListener("pointermove", move);
          window.removeEventListener("pointerup", stop);
          window.removeEventListener("pointercancel", stop);
        };
        window.addEventListener("pointermove", move);
        window.addEventListener("pointerup", stop);
        window.addEventListener("pointercancel", stop);
      }}
      onFocus={() => setMeasured(measure())}
      onDoubleClick={() => onChange(null)}
      onKeyDown={(event) => {
        if (event.key === "ArrowUp" || event.key === "ArrowDown") {
          event.preventDefault();
          const current = measure();
          const grow = event.key === "ArrowUp";
          resizeBy(grow || height === null ? current : Math.min(height, current), grow ? KEYBOARD_STEP : -KEYBOARD_STEP);
        } else if (event.key === "Home") {
          event.preventDefault();
          onChange(null);
        }
      }}
    >
      <span className="h-1 w-10 rounded-full bg-muted opacity-0 transition-opacity group-hover/resize:opacity-100 group-focus-visible/resize:opacity-100 group-active/resize:opacity-100" />
    </div>
  );
}
