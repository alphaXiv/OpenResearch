import { TONE_BG, type RunModel } from "../../runMonitor/model";
import { Tooltip } from "../ui";
import { cn } from "../ui/cn";

/**
 * The bar, always an underline: stages are segments, and each look the watcher took is a tick standing on it,
 * amber where it flagged something. With `hover`, a tick shows what was said at that look. A page draws it as a `bar`
 * in the flow, under its stages.
 */
export function Rail({ model, kind, hover = false }: { model: RunModel; kind: "edge" | "thick" | "bar"; hover?: boolean }) {
  const { bar, ticks } = model;
  const fill = bar.frozen ? "bg-subtext" : TONE_BG[model.tone];
  const mark = (flag: boolean) => <i className={flag ? "h-1.5 w-0.5 bg-mark-amber" : "h-1 w-px bg-subtext"} />;
  return (
    <div className={kind === "bar" ? "pointer-events-none" : "pointer-events-none absolute inset-x-0 bottom-0"}>
      {ticks.length > 0 && (
        <div className="relative h-1.5" aria-hidden="true">
          {ticks.map((tick) => (
            <span key={tick.id} className={cn("absolute bottom-0 flex -translate-x-1/2 rtl:translate-x-1/2", hover && "pointer-events-auto")} style={{ insetInlineStart: `${tick.at * 100}%` }}>
              {hover
                ? <Tooltip interactive content={tick.text}><span className="flex h-3 w-[9px] items-end justify-center">{mark(tick.flag)}</span></Tooltip>
                : mark(tick.flag)}
            </span>
          ))}
        </div>
      )}
      <div
        className={cn("flex gap-px", kind === "bar" ? "h-1.5 overflow-hidden rounded-full" : kind === "thick" ? "h-1" : "h-[3px]")}
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={bar.whole === null ? undefined : Math.round(bar.whole * 100)}
        aria-valuetext={bar.label}
      >
        {bar.segments.map((segment, index) => (
          <span key={index} className="h-full bg-panel" style={{ width: `${segment.weight * 100}%` }}>
            {segment.fill !== null && <i className={cn("block h-full", fill)} style={{ width: `${segment.fill * 100}%` }} />}
          </span>
        ))}
      </div>
    </div>
  );
}
