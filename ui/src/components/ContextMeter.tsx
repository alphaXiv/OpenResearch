import { DirectionProvider } from "@base-ui/react/direction-provider";
import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { getLocale } from "../paraglide/runtime.js";
import { fmtTokens, type ContextUsage } from "../api";
import { Popover } from "@base-ui/react/popover";
import { ProgressBar } from "./ProgressBar";

/** Amber ≥80%, red ≥95% — mirrors Claude Desktop's context meter. */
function tone(pct: number): string {
  if (pct >= 95) return "var(--accent-red)";
  if (pct >= 80) return "var(--accent-amber)";
  return "var(--subtext)";
}

const RING_R = 6.5;
const RING_C = 2 * Math.PI * RING_R;

export function ContextMeter({ usage }: { usage?: ContextUsage }) {
  if (!usage || usage.usedTokens <= 0) return null;

  const { usedTokens, contextWindow } = usage;
  const pct =
    contextWindow && contextWindow > 0
      ? Math.min(100, Math.round((usedTokens / contextWindow) * 100))
      : null;
  const fill = pct === null ? "var(--subtext)" : tone(pct);
  const percent = pct === null
    ? ""
    : new Intl.NumberFormat(getLocale(), { style: "percent" }).format(pct / 100);

  return (
    <DirectionProvider direction={typeof document !== "undefined" && document.documentElement.dir === "rtl" ? "rtl" : "ltr"}>
      <Popover.Root>
        <div className="option-picker relative inline-flex shrink-0">
          <Popover.Trigger
            type="button"
            className={`${pct === null ? "inline-flex h-8 items-center rounded-md px-1 transition-[background,color] duration-150 ease-standard hover:bg-surface" : "inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-text transition-[background,color] duration-150 ease-standard hover:bg-surface"} composer-bare context-ring text-sm text-text`}
            title={m.context_meter_context_window_used()}
            aria-label={m.context_meter_context_window_used()}
          >
            {pct === null ? (
              fmtTokens(usedTokens)
            ) : (
              <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
                <circle
                  cx="8"
                  cy="8"
                  r={RING_R}
                  fill="none"
                  stroke="var(--border)"
                  strokeWidth="2.5"
                />
                <circle
                  cx="8"
                  cy="8"
                  r={RING_R}
                  fill="none"
                  stroke={fill}
                  strokeWidth="2.5"
                  strokeLinecap="round"
                  strokeDasharray={`${(RING_C * Math.max(pct, 2)) / 100} ${RING_C}`}
                  transform="rotate(-90 8 8)"
                />
              </svg>
            )}
          </Popover.Trigger>
          <Popover.Portal>
            <Popover.Positioner side="top" align="end" sideOffset={8} collisionPadding={8} className="z-50">
              <Popover.Popup aria-label={m.context_meter_context_window()} className="outline-none option-menu max-w-[calc(100vw-1rem)] max-h-95 flex flex-col bg-background border border-border rounded-lg shadow-menu overflow-hidden min-w-47.5 context-meter-menu w-70 pt-2.5 px-3 pb-3 [&_.progress]:mt-2 [&_.progress]:mx-0 [&_.progress]:mb-0 [&_.progress-track]:h-[5px] [&_.progress-track]:border-0 [&_.progress-track]:bg-border">
                <div className="context-meter-head flex justify-between items-baseline gap-3 text-sm text-muted">
                  <span>{m.context_meter_context_window()}</span>
                  <span className="context-meter-value text-text tabular-nums">
                    {pct === null
                      ? m.context_meter_tokens({ value: ltr(fmtTokens(usedTokens)) })
                      : m.context_meter_usage({ used: ltr(fmtTokens(usedTokens)), total: ltr(fmtTokens(contextWindow!)), percent: ltr(percent) })}
                  </span>
                </div>
                {pct !== null && (
                  <ProgressBar value={usedTokens} max={contextWindow!} fillColor={fill} />
                )}
              </Popover.Popup>
            </Popover.Positioner>
          </Popover.Portal>
        </div>
      </Popover.Root>
    </DirectionProvider>
  );
}
