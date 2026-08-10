import { useCallback, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";
import {
  type Bounds,
  type TourAnchor,
  useMeasure,
  usePopoverPosition,
  useTourBounds,
} from "./tourGeometry";
import { GHOST_BUTTON_CLASS_NAME, ICON_BUTTON_CLASS_NAME, PRIMARY_BUTTON_CLASS_NAME } from "../styleClasses";

/** Breathing room between a target's edges and the spotlight cutout. */
const BOX_PADDING = 8;
/** Gap between the spotlight and the tour card. */
const CARD_DISTANCE = 20;

interface TourStep {
  /** data-onboarding ids to spotlight; null = centered card over a full dim. */
  focus: string[] | null;
  /** Which side of the target the card sits on; null = centered. */
  anchor: TourAnchor | null;
  title: string;
  description: string;
}

const STEPS: TourStep[] = [
  {
    focus: null,
    anchor: null,
    title: "Welcome to OpenResearch",
    description:
      "OpenResearch is your home for autoresearch. Spawn and coordinate research agents " +
      "in one workspace.",
  },
  {
    focus: ["composer"],
    anchor: "above",
    title: "Talk to your research agent",
    description:
      "Prompt your research agents to replicate a paper, create a baseline experiment, " +
      "run an eval, or investigate any research question. Type / for skills like " +
      "/reproduce-paper.",
  },
  {
    focus: ["model-picker"],
    anchor: "above",
    title: "Pick your model",
    description:
      "Choose a model from any harness you've connected: Claude Code, Codex, or OpenCode. " +
      "New sessions start with whatever you pick here, and each session keeps its harness.",
  },
  {
    focus: ["nav-artifacts"],
    anchor: "right",
    title: "Project artifacts",
    description:
      "The agent writes its reports, figures, and other outputs here, and anything you drop " +
      "in is visible to it too. Check Artifacts after a run to see what came back.",
  },
  {
    focus: ["nav-compute"],
    anchor: "right",
    title: "Configure compute",
    description:
      "This is where compute is configured. Point runs at this machine, Modal, SSH boxes, " +
      "Kubernetes, or Slurm. Set it up once and agents pick the right hardware per run.",
  },
  {
    focus: ["experiments"],
    anchor: "left",
    title: "Follow every experiment",
    description:
      "Runs land here as a tree of experiments. Branch variants off a baseline, compare " +
      "results, and open any run's terminal or code changes in a tab.",
  },
  {
    focus: ["new-session"],
    anchor: "right",
    title: "Start a session",
    description:
      "Each session is its own agent working in its own worktree, so you can run several " +
      "agents in parallel. Ask for your first experiment whenever you're ready.",
  },
];

/**
 * The onboarding tour: a dimming overlay with a spotlight cut around the
 * focused element, plus an anchored card describing it. CSS transitions morph
 * the spotlight between steps. Targets are located by `data-onboarding`
 * attributes; a missing target degrades to a full dim with a centered card.
 */
export function Tour({ onClose }: { onClose: () => Promise<void> }) {
  const [index, setIndex] = useState(0);
  const [closing, setClosing] = useState(false);
  const [closeError, setCloseError] = useState<string | null>(null);
  const step = STEPS[index];
  const bounds = useTourBounds(step.focus ?? []);
  const requestClose = useCallback(() => {
    if (closing) return;
    setClosing(true);
    setCloseError(null);
    void onClose()
      .catch((error) => {
        setCloseError(error instanceof Error ? error.message : String(error));
      })
      .finally(() => setClosing(false));
  }, [closing, onClose]);

  // Own Escape in the capture phase so it can never reach ChatPanel's
  // document-level listener, which would interrupt a running agent turn.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      requestClose();
    }
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [requestClose]);

  const box = bounds
    ? {
        left: bounds.x - BOX_PADDING,
        top: bounds.y - BOX_PADDING,
        width: bounds.width + BOX_PADDING * 2,
        height: bounds.height + BOX_PADDING * 2,
      }
    : null;

  return createPortal(
    <div className="tour-overlay [position:fixed] [inset:0] [z-index:200]">
      {box ? (
        <>
          {/* Dim everything except the spotlight via an oversized box-shadow. */}
          <div className="tour-spotlight [position:absolute] [border-radius:var(--radius-lg)] [box-shadow:0_0_0_9999px_rgba(0,_0,_0,_0.55)] [transition:all_300ms_ease]" style={box} />
          <div className="tour-ring [position:absolute] [border-radius:var(--radius-lg)] [border:2px_solid_var(--primary)] [transition:all_300ms_ease]" style={box} />
        </>
      ) : (
        <div className="tour-dim [position:absolute] [inset:0] [background:rgba(0,_0,_0,_0.55)]" />
      )}
      <TourCard
        step={step}
        bounds={bounds}
        index={index}
        onBack={() => setIndex((i) => Math.max(0, i - 1))}
        onNext={() => (index + 1 >= STEPS.length ? requestClose() : setIndex(index + 1))}
        onClose={requestClose}
        closing={closing}
        closeError={closeError}
      />
    </div>,
    document.body,
  );
}

function TourCard({
  step,
  bounds,
  index,
  onBack,
  onNext,
  onClose,
  closing,
  closeError,
}: {
  step: TourStep;
  bounds: Bounds;
  index: number;
  onBack: () => void;
  onNext: () => void;
  onClose: () => void;
  closing: boolean;
  closeError: string | null;
}) {
  const measure = useMeasure();
  const popover = usePopoverPosition(
    bounds && step.anchor
      ? {
          x: bounds.x - BOX_PADDING,
          y: bounds.y - BOX_PADDING,
          width: bounds.width + BOX_PADDING * 2,
          height: bounds.height + BOX_PADDING * 2,
          anchor: step.anchor,
          distance: CARD_DISTANCE,
        }
      : null,
    measure,
  );

  // Only trust the computed position once the card has real dimensions and
  // the viewport size is known; until then, center it.
  const positioned = step.anchor != null && bounds != null && popover.x > 0;
  const last = index + 1 === STEPS.length;

  return (
    <div
      ref={measure.ref}
      className={`tour-card [position:absolute] [width:390px] [max-width:calc(100vw_-_32px)] [background:var(--base)] [border:1px_solid_var(--border)] [border-radius:var(--radius-xl)] [box-shadow:0_24px_60px_rgba(0,_0,_0,_0.22)] [padding:21px_23px_0] [transition:left_200ms_ease,_top_200ms_ease] [&.centered]:[left:50%] [&.centered]:[top:50%] [&.centered]:[transform:translate(-50%,_-50%)] [&.centered]:[transition:none] [&_h3]:[margin:0_26px_7px_0] [&_h3]:[font-size:var(--fs-lg)] [&_h3]:[font-weight:var(--fw-semibold)] [&_p]:[margin:0] [&_p]:[font-size:var(--fs-base)] [&_p]:[line-height:1.55] [&_p]:[color:var(--text)] [&_.tour-close]:[position:absolute] [&_.tour-close]:[top:12px] [&_.tour-close]:[right:12px] ${positioned ? "" : "centered"}`}
      style={positioned ? { left: popover.x, top: popover.y } : undefined}
    >
      {positioned && step.anchor && (
        <Arrow anchor={step.anchor} adjustment={popover.arrowAdjustment} />
      )}
      <button className={`${ICON_BUTTON_CLASS_NAME} tour-close`} title="Skip tour" onClick={onClose} disabled={closing}>
        <X size={15} />
      </button>
      <h3>{step.title}</h3>
      <p>{step.description}</p>
      {closeError && <div className="error">Couldn't save tour progress. Try again.</div>}
      <div className="tour-footer [display:flex] [align-items:center] [border-top:1px_solid_var(--border-variant)] [margin-top:16px] [padding:11px_0]">
        <div className="tour-footer-side [flex:1] [display:flex] [&.end]:[justify-content:flex-end]">
          {index > 0 && (
            <button className={GHOST_BUTTON_CLASS_NAME} onClick={onBack}>
              Back
            </button>
          )}
        </div>
        <span className="tour-count [font-size:var(--fs-sm)] [color:var(--muted)] [font-variant-numeric:tabular-nums]">
          {index + 1} / {STEPS.length}
        </span>
        <div className="tour-footer-side [flex:1] [display:flex] [&.end]:[justify-content:flex-end] end">
          <button className={PRIMARY_BUTTON_CLASS_NAME} onClick={onNext} disabled={closing}>
            {closing ? "Saving…" : last ? "Done" : "Next"}
          </button>
        </div>
      </div>
    </div>
  );
}

/**
 * A rotated-square arrow on the card edge nearest the spotlight. `adjustment`
 * is how far viewport clamping displaced the card along its cross axis; the
 * arrow shifts by the same amount to keep pointing at the target.
 */
function Arrow({ anchor, adjustment }: { anchor: TourAnchor; adjustment: number }) {
  const cross =
    anchor === "above" || anchor === "below" ? `${adjustment}px 0` : `0 ${adjustment}px`;
  return <div className={`tour-arrow [position:absolute] [width:12px] [height:12px] [background:var(--base)] [rotate:45deg] [pointer-events:none] [&.above]:[bottom:-6.5px] [&.above]:[left:calc(50%_-_6px)] [&.above]:[border-right:1px_solid_var(--border)] [&.above]:[border-bottom:1px_solid_var(--border)] [&.below]:[top:-6.5px] [&.below]:[left:calc(50%_-_6px)] [&.below]:[border-left:1px_solid_var(--border)] [&.below]:[border-top:1px_solid_var(--border)] [&.left]:[right:-6.5px] [&.left]:[top:calc(50%_-_6px)] [&.left]:[border-top:1px_solid_var(--border)] [&.left]:[border-right:1px_solid_var(--border)] [&.right]:[left:-6.5px] [&.right]:[top:calc(50%_-_6px)] [&.right]:[border-bottom:1px_solid_var(--border)] [&.right]:[border-left:1px_solid_var(--border)] ${anchor}`} style={{ translate: cross }} />;
}
