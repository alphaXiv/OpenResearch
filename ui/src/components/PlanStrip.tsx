import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { ChevronDown, CornerDownLeft, ScrollText } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";
import { Button, MenuItem } from "./ui";

const PROMPT_ACTIONS_CLASS_NAME = "prompt-actions plan-strip-actions flex flex-wrap justify-end gap-x-2 gap-y-1.5";

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
 *  - Open plan: link in the title row → the end-pane plan tab. */
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
    <div className="plan-strip relative w-full mt-0 mx-0 mb-2.5 py-[11px] px-[13px] flex flex-col items-stretch gap-2.5 border border-border border-s-[3px] border-s-accent-blue rounded-md bg-surface shadow-plan">
      <div className="plan-strip-info flex items-baseline gap-2 min-w-0">
        <ScrollText size={14} className="plan-strip-icon text-accent-blue shrink-0 self-center" />
        <span dir="auto" className="plan-strip-title text-sm font-semibold whitespace-nowrap">
          {synthesized
            ? m.plan_strip_agent_ready({ agent: ltr(agentLabel) })
            : m.plan_strip_agent_proposed({ agent: ltr(agentLabel) })}
        </span>
        <button
          className="plan-strip-open ms-auto p-0 border-0 bg-none bg-transparent text-accent-blue text-sm cursor-pointer whitespace-nowrap shrink-0 [&:hover]:underline"
          {...tabOpenGestureHandlers<HTMLButtonElement>(onView)}
        >
          {m.plan_strip_open_plan()}
        </button>
      </div>
      {revising ? (
        <>
          <textarea
            dir="auto"
            ref={textareaRef}
            className="plan-strip-revise-input w-full resize-none border border-border rounded-md py-[9px] px-[11px] text-sm font-[inherit] bg-background text-text [&:focus]:border-accent-blue"
            placeholder={m.plan_strip_what_should_change_optional()}
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
            <Button
              size="small"
              onClick={() => {
                setNote("");
                setRevising(false);
              }}
            >
              {m.plan_strip_back()}
            </Button>
            <span className="plan-strip-spacer flex-1" />
            <Button size="small" variant="primary" onClick={submitRevision}>
              {m.plan_strip_revise()}
              <CornerDownLeft size={13} />
            </Button>
          </div>
        </>
      ) : (
        <div className={PROMPT_ACTIONS_CLASS_NAME}>
          <Button size="small" onClick={onReject}>
            {m.plan_strip_reject()}
          </Button>
          <Button size="small" onClick={() => setRevising(true)}>
            {m.plan_strip_revise_05bacc9()}
          </Button>
          <span className="plan-strip-spacer flex-1" />
          {showResumeModes ? (
            <div className="plan-strip-approve relative flex" ref={menuRef}>
              <Button size="small" variant="primary" className="rounded-e-none" onClick={() => onApprove("auto")}>
                {m.plan_strip_accept_and_auto_mode()}
              </Button>
              <Button
                size="small"
                variant="primary"
                className="rounded-s-none border-s-plan-caret px-1.5"
                aria-label={m.plan_strip_more_approval_options()}
                onClick={() => setMenuOpen((o) => !o)}
              >
                <ChevronDown size={13} />
              </Button>
              {menuOpen && (
                <div className="plan-strip-menu absolute end-0 bottom-[calc(100%_+_4px)] flex min-w-47.5 flex-col rounded-md border border-border bg-surface p-1 shadow-plan-menu z-6">
                  <MenuItem
                    onClick={() => {
                      setMenuOpen(false);
                      onApprove("bypassPermissions");
                    }}
                  >
                    {m.plan_strip_accept_and_bypass_all()}
                  </MenuItem>
                </div>
              )}
            </div>
          ) : (
            <Button size="small" variant="primary" onClick={() => onApprove()}>
              {m.plan_strip_accept_plan()}
            </Button>
          )}
        </div>
      )}
    </div>
  );
}
