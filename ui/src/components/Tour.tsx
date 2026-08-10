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
    <div className="tour-overlay fixed inset-0 z-200">
      {box ? (
        <>
          {/* Dim everything except the spotlight via an oversized box-shadow. */}
          <div className="tour-spotlight absolute rounded-lg shadow-[0_0_0_9999px_rgba(0,_0,_0,_0.55)] transition-all duration-300 ease-standard" style={box} />
          <div className="tour-ring absolute rounded-lg border-2 border-primary transition-all duration-300 ease-standard" style={box} />
        </>
      ) : (
        <div className="tour-dim absolute inset-0 bg-[rgba(0,_0,_0,_0.55)]" />
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
      className={`tour-card absolute w-97.5 max-w-[calc(100vw_-_32px)] bg-background border border-border rounded-xl shadow-[0_24px_60px_rgba(0,_0,_0,_0.22)] pt-[21px] px-[23px] pb-0 transition-[left,top] duration-200 ease-standard [&.centered]:left-1/2 [&.centered]:top-1/2 [&.centered]:-translate-x-1/2 [&.centered]:-translate-y-1/2 [&.centered]:transition-none [&_h3]:mt-0 [&_h3]:mr-6.5 [&_h3]:mb-[7px] [&_h3]:ml-0 [&_h3]:text-lg [&_h3]:font-semibold [&_p]:m-0 [&_p]:text-base [&_p]:leading-[1.55] [&_p]:text-text [&_.tour-close]:absolute [&_.tour-close]:top-3 [&_.tour-close]:right-3 ${positioned ? "" : "centered"}`}
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
      <div className="tour-footer flex items-center border-t border-t-border-variant mt-4 py-[11px] px-0">
        <div className="tour-footer-side flex-1 flex [&.end]:justify-end">
          {index > 0 && (
            <button className={GHOST_BUTTON_CLASS_NAME} onClick={onBack}>
              Back
            </button>
          )}
        </div>
        <span className="tour-count text-sm text-muted tabular-nums">
          {index + 1} / {STEPS.length}
        </span>
        <div className="tour-footer-side flex-1 flex [&.end]:justify-end end">
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
  return <div className={`tour-arrow absolute w-3 h-3 bg-background rotate-45 pointer-events-none [&.above]:-bottom-[6.5px] [&.above]:left-[calc(50%_-_6px)] [&.above]:border-r [&.above]:border-r-border [&.above]:border-b [&.above]:border-b-border [&.below]:-top-[6.5px] [&.below]:left-[calc(50%_-_6px)] [&.below]:border-l [&.below]:border-l-border [&.below]:border-t [&.below]:border-t-border [&.left]:-right-[6.5px] [&.left]:top-[calc(50%_-_6px)] [&.left]:border-t [&.left]:border-t-border [&.left]:border-r [&.left]:border-r-border [&.right]:-left-[6.5px] [&.right]:top-[calc(50%_-_6px)] [&.right]:border-b [&.right]:border-b-border [&.right]:border-l [&.right]:border-l-border ${anchor}`} style={{ translate: cross }} />;
}
