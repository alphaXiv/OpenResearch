import { fmtTokens, type ContextUsage } from "../api";
import { usePopover } from "./ModelPicker";
import { ProgressBar } from "./ProgressBar";

/** Amber ≥80%, red ≥95% — mirrors Claude Desktop's context meter. */
function tone(pct: number): string {
  if (pct >= 95) return "var(--accent-red)";
  if (pct >= 80) return "var(--accent-amber)";
  return "var(--accent)";
}

const RING_R = 6.5;
const RING_C = 2 * Math.PI * RING_R;

/** Composer meter: how much of the model's context window this session has
 * used, drawn as a small progress ring (token-count text when the window is
 * unknown). Hidden until the harness first reports usage; the popover holds
 * the breakdown. */
export function ContextMeter({ usage }: { usage?: ContextUsage }) {
  if (!usage || usage.usedTokens <= 0) return null;
  return <VisibleContextMeter usage={usage} />;
}

function VisibleContextMeter({ usage }: { usage: ContextUsage }) {
  const { open, setOpen, ref } = usePopover();
  const { usedTokens, contextWindow } = usage;
  const pct =
    contextWindow && contextWindow > 0
      ? Math.min(100, Math.round((usedTokens / contextWindow) * 100))
      : null;
  const fill = pct === null ? "var(--accent)" : tone(pct);

  return (
    <div className="option-picker [position:relative] [display:inline-flex]" ref={ref}>
      <button
        type="button"
        className="composer-bare [display:inline-flex] [align-items:center] [gap:3px] [font-size:var(--fs-md)] [color:var(--text)] [padding:5px_4px] [border-radius:var(--radius-sm)] [&:hover]:[color:var(--text)] [&.context-ring]:[display:inline-flex] [&.context-ring]:[align-items:center] [&.context-ring]:[margin-right:8px] context-ring"
        title="Context window used"
        onClick={() => setOpen((v) => !v)}
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
      </button>
      {open && (
        <div className="option-menu [position:absolute] [bottom:calc(100%_+_8px)] [left:0] [max-height:380px] [display:flex] [flex-direction:column] [background:var(--base)] [border:1px_solid_var(--border)] [border-radius:var(--radius-lg)] [box-shadow:0_12px_32px_rgba(0,_0,_0,_0.18)] [z-index:50] [overflow:hidden] [min-width:190px] [&.align-right]:[left:auto] [&.align-right]:[right:0] [&.drop-down]:[bottom:auto] [&.drop-down]:[top:calc(100%_+_4px)] [&.session-menu]:[left:auto] [&.session-menu]:[right:6px] [&.session-menu]:[top:calc(100%_-_2px)] [&.session-menu]:[min-width:140px] align-right context-meter-menu [width:280px] [padding:10px_12px_12px] [&_.progress]:[margin:8px_0_0] [&_.progress-track]:[height:5px] [&_.progress-track]:[border:none] [&_.progress-track]:[background:var(--border)]">
          <div className="context-meter-head [display:flex] [justify-content:space-between] [align-items:baseline] [gap:12px] [font-size:var(--fs-sm)] [color:var(--muted)]">
            <span>Context window</span>
            <span className="context-meter-value [color:var(--text)] [font-variant-numeric:tabular-nums]">
              {pct === null
                ? `${fmtTokens(usedTokens)} tokens`
                : `${fmtTokens(usedTokens)} / ${fmtTokens(contextWindow!)} (${pct}%)`}
            </span>
          </div>
          {pct !== null && (
            <ProgressBar value={usedTokens} max={contextWindow!} fillColor={fill} />
          )}
        </div>
      )}
    </div>
  );
}
