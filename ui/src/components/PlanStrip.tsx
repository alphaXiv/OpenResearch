import { ChevronDown, CornerDownLeft, ScrollText } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";

const PROMPT_ACTIONS_CLASS_NAME = [
  "prompt-actions flex flex-wrap [&_.btn-primary]:inline-flex",
  "[&_.btn-primary]:items-center [&_.btn-primary]:gap-1.5 [&_.btn-primary]:py-1.5 [&_.btn-primary]:px-[13px]",
  "[&_.btn-primary]:font-[inherit] [&_.btn-primary]:text-sm",
  "[&_.btn-primary]:font-semibold [&_.btn-primary]:rounded-sm",
  "[&_.btn-primary]:cursor-pointer [&_.btn-primary]:transition-[background,border-color] [&_.btn-primary]:duration-80 [&_.btn-primary]:ease-standard",
  "[&_.btn-ghost]:inline-flex [&_.btn-ghost]:items-center [&_.btn-ghost]:gap-1.5",
  "[&_.btn-ghost]:py-1.5 [&_.btn-ghost]:px-[13px] [&_.btn-ghost]:font-[inherit] [&_.btn-ghost]:text-sm",
  "[&_.btn-ghost]:font-semibold [&_.btn-ghost]:rounded-sm",
  "[&_.btn-ghost]:cursor-pointer [&_.btn-ghost]:transition-[background,border-color] [&_.btn-ghost]:duration-80 [&_.btn-ghost]:ease-standard",
  "[&_.btn-ghost]:border-border [&_button:disabled]:opacity-50",
  "[&_button:disabled]:cursor-default plan-strip-actions gap-y-1.5 gap-x-2 justify-end",
  "[&_.btn-primary]:bg-transparent [&_.btn-primary]:border [&_.btn-primary]:border-text",
  "[&_.btn-primary]:text-text [&_.btn-ghost]:bg-transparent",
  "[&_.btn-ghost]:border [&_.btn-ghost]:border-text [&_.btn-ghost]:text-text",
  "[&_.btn-primary:hover:not(:disabled)]:bg-[var(--surface-2,_rgb(0_0_0_/_5%))]",
  "[&_.btn-primary:hover:not(:disabled)]:border-text",
  "[&_.btn-primary:hover:not(:disabled)]:text-text [&_.btn-primary:hover:not(:disabled)]:opacity-100",
  "[&_.btn-ghost:hover:not(:disabled)]:bg-[var(--surface-2,_rgb(0_0_0_/_5%))]",
  "[&_.btn-ghost:hover:not(:disabled)]:border-text",
  "[&_.btn-ghost:hover:not(:disabled)]:text-text [&_.btn-ghost:hover:not(:disabled)]:opacity-100",
  "[&_.plan-strip-primary]:bg-text [&_.plan-strip-primary]:border-text",
  "[&_.plan-strip-primary]:text-background",
  "[&_.plan-strip-primary:hover:not(:disabled)]:bg-[color-mix(in_oklab,_var(--text)_85%,_var(--base))]",
  "[&_.plan-strip-primary:hover:not(:disabled)]:border-text",
  "[&_.plan-strip-primary:hover:not(:disabled)]:text-background",
  "[&_.plan-strip-caret]:rounded-tl-none [&_.plan-strip-caret]:rounded-bl-none",
  "[&_.plan-strip-caret]:py-0 [&_.plan-strip-caret]:px-1.5 [&_.plan-strip-caret]:flex",
  "[&_.plan-strip-caret]:items-center",
  "[&_.plan-strip-caret]:border-l [&_.plan-strip-caret]:border-l-[color-mix(in_oklab,_var(--base)_35%,_var(--text))]",
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
 *    (skip every gate, not just Auto's). No plain Accept edits tier here —
 *    the app has no story for partial (edits-only) approval.
 *  - Open plan: link in the title row → the right-pane plan tab. */
export function PlanStrip({
  synthesized,
  agentLabel,
  onView,
  onApprove,
  showResumeModes,
  onReject,
  onRevise,
}: {
  /** Card synthesized from the turn's final text (no ExitPlanMode call). */
  synthesized: boolean;
  /** The harness's display name for the strip copy (e.g. "Claude Code",
   * "Codex"); falls back to a generic label when the harness is unknown. */
  agentLabel: string;
  onView: (intent: TabOpenIntent) => void;
  onApprove: (resumeMode?: "auto" | "bypassPermissions") => void;
  /** Claude approval chooses its next permission mode; Codex preserves the
   * current permission choice and only leaves the independent Plan axis. */
  showResumeModes: boolean;
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
    <div className="plan-strip relative w-full mt-0 mx-0 mb-2.5 py-[11px] px-[13px] flex flex-col items-stretch gap-2.5 border border-border border-l-[3px] border-l-accent-blue rounded-md bg-surface shadow-[0_2px_10px_rgb(0_0_0_/_6%)]">
      <div className="plan-strip-info flex items-baseline gap-2 min-w-0">
        <ScrollText size={14} className="plan-strip-icon text-accent-blue shrink-0 self-center" />
        <span className="plan-strip-title text-md font-semibold whitespace-nowrap">
          {synthesized ? `${agentLabel} is ready to proceed` : `${agentLabel} proposed a plan`}
        </span>
        <button
          className="plan-strip-open ml-auto p-0 border-0 bg-none bg-transparent text-accent-blue text-md cursor-pointer whitespace-nowrap shrink-0 [&:hover]:underline"
          {...tabOpenGestureHandlers<HTMLButtonElement>(onView)}
        >
          Open plan
        </button>
      </div>
      {revising ? (
        <>
          <textarea
            ref={textareaRef}
            className="plan-strip-revise-input w-full resize-none border border-border rounded-md py-[9px] px-[11px] text-md font-[inherit] bg-background text-text [&:focus]:border-accent-blue"
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
            <span className="plan-strip-spacer flex-1" />
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
          <span className="plan-strip-spacer flex-1" />
          {showResumeModes ? (
            <div className="plan-strip-approve relative flex [&_.btn-primary:first-child]:rounded-tr-none [&_.btn-primary:first-child]:rounded-br-none" ref={menuRef}>
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
                <div className="plan-strip-menu absolute right-0 bottom-[calc(100%_+_4px)] flex flex-col min-w-47.5 p-1 border border-border rounded-md bg-surface shadow-[0_6px_20px_rgb(0_0_0_/_12%)] z-6 [&_button]:text-left [&_button]:py-[7px] [&_button]:px-[9px] [&_button]:border-0 [&_button]:rounded-sm [&_button]:bg-transparent [&_button]:text-text [&_button]:text-md [&_button]:cursor-pointer [&_button:hover]:bg-[var(--surface-2,_rgb(0_0_0_/_5%))]">
                  <button
                    onClick={() => {
                      setMenuOpen(false);
                      onApprove("bypassPermissions");
                    }}
                  >
                    Accept and bypass all
                  </button>
                </div>
              )}
            </div>
          ) : (
            <button className="btn-primary plan-strip-primary" onClick={() => onApprove()}>
              Accept plan
            </button>
          )}
        </div>
      )}
    </div>
  );
}
