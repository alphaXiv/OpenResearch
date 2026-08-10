import { ChevronDown, CornerDownLeft, ScrollText } from "lucide-react";
import { useEffect, useRef, useState } from "react";

const PROMPT_ACTIONS_CLASS_NAME = [
  "prompt-actions [display:flex] [flex-wrap:wrap] [&_.btn-primary]:[display:inline-flex]",
  "[&_.btn-primary]:[align-items:center] [&_.btn-primary]:[gap:6px] [&_.btn-primary]:[padding:6px_13px]",
  "[&_.btn-primary]:[font-family:inherit] [&_.btn-primary]:[font-size:var(--fs-sm)]",
  "[&_.btn-primary]:[font-weight:var(--fw-semibold)] [&_.btn-primary]:[border-radius:var(--radius-sm)]",
  "[&_.btn-primary]:[cursor:pointer] [&_.btn-primary]:[transition:background_80ms_ease,_border-color_80ms_ease]",
  "[&_.btn-ghost]:[display:inline-flex] [&_.btn-ghost]:[align-items:center] [&_.btn-ghost]:[gap:6px]",
  "[&_.btn-ghost]:[padding:6px_13px] [&_.btn-ghost]:[font-family:inherit] [&_.btn-ghost]:[font-size:var(--fs-sm)]",
  "[&_.btn-ghost]:[font-weight:var(--fw-semibold)] [&_.btn-ghost]:[border-radius:var(--radius-sm)]",
  "[&_.btn-ghost]:[cursor:pointer] [&_.btn-ghost]:[transition:background_80ms_ease,_border-color_80ms_ease]",
  "[&_.btn-ghost]:[border-color:var(--border)] [&_button:disabled]:[opacity:0.5]",
  "[&_button:disabled]:[cursor:default] plan-strip-actions [gap:6px_8px] [justify-content:flex-end]",
  "[&_.btn-primary]:[background:transparent] [&_.btn-primary]:[border:1px_solid_var(--text)]",
  "[&_.btn-primary]:[color:var(--text)] [&_.btn-ghost]:[background:transparent]",
  "[&_.btn-ghost]:[border:1px_solid_var(--text)] [&_.btn-ghost]:[color:var(--text)]",
  "[&_.btn-primary:hover:not(:disabled)]:[background:var(--surface-2,_rgb(0_0_0_/_5%))]",
  "[&_.btn-primary:hover:not(:disabled)]:[border-color:var(--text)]",
  "[&_.btn-primary:hover:not(:disabled)]:[color:var(--text)] [&_.btn-primary:hover:not(:disabled)]:[opacity:1]",
  "[&_.btn-ghost:hover:not(:disabled)]:[background:var(--surface-2,_rgb(0_0_0_/_5%))]",
  "[&_.btn-ghost:hover:not(:disabled)]:[border-color:var(--text)]",
  "[&_.btn-ghost:hover:not(:disabled)]:[color:var(--text)] [&_.btn-ghost:hover:not(:disabled)]:[opacity:1]",
  "[&_.plan-strip-primary]:[background:var(--text)] [&_.plan-strip-primary]:[border-color:var(--text)]",
  "[&_.plan-strip-primary]:[color:var(--base)]",
  "[&_.plan-strip-primary:hover:not(:disabled)]:[background:color-mix(in_oklab,_var(--text)_85%,_var(--base))]",
  "[&_.plan-strip-primary:hover:not(:disabled)]:[border-color:var(--text)]",
  "[&_.plan-strip-primary:hover:not(:disabled)]:[color:var(--base)]",
  "[&_.plan-strip-caret]:[border-top-left-radius:0] [&_.plan-strip-caret]:[border-bottom-left-radius:0]",
  "[&_.plan-strip-caret]:[padding:0_6px] [&_.plan-strip-caret]:[display:flex]",
  "[&_.plan-strip-caret]:[align-items:center]",
  "[&_.plan-strip-caret]:[border-left:1px_solid_color-mix(in_oklab,_var(--base)_35%,_var(--text))]",
].join(" ");

/** Docked strip above the composer while a plan awaits the user's decision.
 * It owns the plan actions (the inline card renders compact, buttonless) so
 * the approval controls never scroll away with the transcript. Disappears when
 * the prompt resolves (the server re-emits the message with `resolved`).
 *
 * Claude-desktop parity — the actions mean:
 *  - Reject: plain rejection, no feedback; the model stops and waits.
 *  - Revise…: swaps the strip into its own inline textarea ("What should
 *    change? (optional)") with Back/Revise buttons — self-contained, not a
 *    detour through the main composer.
 *  - Accept and auto mode (primary): approve + resume under Auto — the
 *    default accept action. The caret menu holds Accept and bypass all
 *    (skip every gate, not just Auto's). No plain "accept-edits" tier here —
 *    the app has no story for partial (edits-only) approval.
 *  - Open plan: link in the title row → the right-pane plan tab. */
export function PlanStrip({
  synthesized,
  agentLabel,
  onView,
  onApprove,
  onReject,
  onRevise,
}: {
  /** Card synthesized from the turn's final text (no ExitPlanMode call). */
  synthesized: boolean;
  /** The harness's display name for the strip copy (e.g. "Claude Code",
   * "Codex"); falls back to a generic label when the harness is unknown. */
  agentLabel: string;
  onView: () => void;
  onApprove: (resumeMode: "auto" | "bypass") => void;
  onReject: () => void;
  /** Revision feedback; always non-empty (a blank submit sends a generic
   * "please revise" — note presence is what distinguishes revise from
   * reject on the wire). */
  onRevise: (note: string) => void;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement | null>(null);
  const [revising, setRevising] = useState(false);
  const [note, setNote] = useState("");
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (!menuOpen) return;
    const close = (e: PointerEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) setMenuOpen(false);
    };
    window.addEventListener("pointerdown", close);
    return () => window.removeEventListener("pointerdown", close);
  }, [menuOpen]);

  useEffect(() => {
    if (revising) textareaRef.current?.focus();
  }, [revising]);

  const submitRevision = () => {
    // A blank submit still means "revise": the wire distinguishes reject
    // (no note) from revise (note), so an empty field gets a generic nudge
    // rather than accidentally reading as a hard rejection. (The backend
    // wraps it as "Keep refining the plan: <note>", so word it to read well
    // there.)
    onRevise(note.trim() || "no specific feedback — use your judgment");
    setNote("");
    setRevising(false);
  };

  return (
    <div className="plan-strip [position:relative] [width:100%] [margin:0_0_10px] [padding:11px_13px] [display:flex] [flex-direction:column] [align-items:stretch] [gap:10px] [border:1px_solid_var(--border)] [border-left:3px_solid_var(--accent-blue)] [border-radius:var(--radius-md)] [background:var(--surface)] [box-shadow:0_2px_10px_rgb(0_0_0_/_6%)]">
      <div className="plan-strip-info [display:flex] [align-items:baseline] [gap:8px] [min-width:0]">
        <ScrollText size={14} className="plan-strip-icon [color:var(--accent-blue)] [flex-shrink:0] [align-self:center]" />
        <span className="plan-strip-title [font-size:var(--fs-md)] [font-weight:var(--fw-semibold)] [white-space:nowrap]">
          {synthesized ? `${agentLabel} is ready to proceed` : `${agentLabel} proposed a plan`}
        </span>
        <button className="plan-strip-open [margin-left:auto] [padding:0] [border:none] [background:none] [color:var(--accent-blue)] [font-size:var(--fs-md)] [cursor:pointer] [white-space:nowrap] [flex-shrink:0] [&:hover]:[text-decoration:underline]" onClick={onView}>
          Open plan
        </button>
      </div>
      {revising ? (
        <>
          <textarea
            ref={textareaRef}
            className="plan-strip-revise-input [width:100%] [resize:none] [border:1px_solid_var(--border)] [border-radius:var(--radius-md)] [padding:9px_11px] [font-size:var(--fs-md)] [font-family:inherit] [background:var(--base)] [color:var(--text)] [&:focus]:[border-color:var(--accent-blue)]"
            placeholder="What should change? (optional)"
            rows={2}
            value={note}
            onChange={(e) => setNote(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.preventDefault();
                setNote("");
                setRevising(false);
              } else if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                submitRevision();
              }
            }}
          />
          <div className={PROMPT_ACTIONS_CLASS_NAME}>
            <button
              className="btn-ghost"
              onClick={() => {
                setNote("");
                setRevising(false);
              }}
            >
              Back
            </button>
            <span className="plan-strip-spacer [flex:1]" />
            <button className="btn-primary plan-strip-primary" onClick={submitRevision}>
              Revise
              <CornerDownLeft size={13} />
            </button>
          </div>
        </>
      ) : (
        <div className={PROMPT_ACTIONS_CLASS_NAME}>
          <button className="btn-ghost" onClick={onReject}>
            Reject
          </button>
          <button className="btn-ghost" onClick={() => setRevising(true)}>
            Revise…
          </button>
          <span className="plan-strip-spacer [flex:1]" />
          <div className="plan-strip-approve [position:relative] [display:flex] [&_.btn-primary:first-child]:[border-top-right-radius:0] [&_.btn-primary:first-child]:[border-bottom-right-radius:0]" ref={menuRef}>
            <button className="btn-primary plan-strip-primary" onClick={() => onApprove("auto")}>
              Accept and auto mode
            </button>
            <button
              className="btn-primary plan-strip-primary plan-strip-caret"
              aria-label="More approval options"
              onClick={() => setMenuOpen((o) => !o)}
            >
              <ChevronDown size={13} />
            </button>
            {menuOpen && (
              <div className="plan-strip-menu [position:absolute] [right:0] [bottom:calc(100%_+_4px)] [display:flex] [flex-direction:column] [min-width:190px] [padding:4px] [border:1px_solid_var(--border)] [border-radius:var(--radius-md)] [background:var(--surface)] [box-shadow:0_6px_20px_rgb(0_0_0_/_12%)] [z-index:6] [&_button]:[text-align:left] [&_button]:[padding:7px_9px] [&_button]:[border:none] [&_button]:[border-radius:var(--radius-sm)] [&_button]:[background:transparent] [&_button]:[color:var(--text)] [&_button]:[font-size:var(--fs-md)] [&_button]:[cursor:pointer] [&_button:hover]:[background:var(--surface-2,_rgb(0_0_0_/_5%))]">
                <button
                  onClick={() => {
                    setMenuOpen(false);
                    onApprove("bypass");
                  }}
                >
                  Accept and bypass all
                </button>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
