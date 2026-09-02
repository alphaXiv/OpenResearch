import { m } from "../paraglide/messages.js";
import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";
import { BrandMark } from "./Wordmark";
import { Button, IconButton } from "./ui";

export function DemoWelcomeModal({
  onClose,
  onCreateProject,
}: {
  onClose: () => Promise<void>;
  onCreateProject: () => Promise<void>;
}) {
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dialogRef = useRef<HTMLDivElement>(null);

  const run = useCallback(
    (action: () => Promise<void>) => {
      if (saving) return;
      setSaving(true);
      setError(null);
      void action()
        .catch(() => setError(m.tour_save_error()))
        .finally(() => setSaving(false));
    },
    [saving],
  );

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      run(onClose);
    };
    document.addEventListener("keydown", onKeyDown, true);
    return () => document.removeEventListener("keydown", onKeyDown, true);
  }, [onClose, run]);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const focusable = () =>
      [...dialog.querySelectorAll<HTMLElement>(
        'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])',
      )];
    (focusable()[0] ?? dialog).focus();

    const trapFocus = (event: KeyboardEvent) => {
      if (event.key !== "Tab") return;
      const controls = focusable();
      if (controls.length === 0) {
        event.preventDefault();
        dialog.focus();
        return;
      }
      const first = controls[0];
      const last = controls[controls.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };

    document.addEventListener("keydown", trapFocus, true);
    return () => {
      document.removeEventListener("keydown", trapFocus, true);
      previousFocus?.focus();
    };
  }, []);

  return createPortal(
    <div className="fixed inset-0 z-200 flex items-center justify-center bg-modal-backdrop p-5">
      <div
        ref={dialogRef}
        className="relative w-110 max-w-full rounded-xl border border-border bg-background p-6 shadow-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="demo-welcome-title"
        tabIndex={-1}
      >
        <IconButton
          className="absolute end-3.5 top-3.5"
          aria-label={m.tour_close()}
          onClick={() => run(onClose)}
          disabled={saving}
        >
          <X size={16} />
        </IconButton>
        <div className="mb-5 flex items-center gap-3 pe-8">
          <span className="block h-9 w-9 shrink-0 [&_svg]:block [&_svg]:h-full [&_svg]:w-full">
            <BrandMark />
          </span>
          <div>
            <div className="mb-0.5 text-xs font-medium tracking-[0.08em] text-primary uppercase">
              {m.tour_demo_project()}
            </div>
            <h2
              id="demo-welcome-title"
              className="m-0 text-2xl leading-tight tracking-[-0.02em]"
            >
              {m.tour_welcome_to_open_research()}
            </h2>
          </div>
        </div>
        <div className="text-base leading-relaxed text-text [&_p]:m-0 [&_p_+_p]:mt-3">
          <p dir="auto">
            {m.tour_this_is_a_demo_project_showing_how_open()}{" "}
            <a
              dir="ltr"
              href="https://github.com/karpathy/nanochat"
              target="_blank"
              rel="noreferrer"
              className="font-medium text-primary underline decoration-border-strong underline-offset-3 hover:decoration-primary"
            >
              {m.tour_nanochat()}
            </a>{m.tour_a_repo_for_training_a_mini_gpt_from()}
          </p>
          <p dir="auto">
            {m.tour_look_through_the_agent_conversations_experiments_runs_and()}
          </p>
        </div>
        {error && <p className="mt-3 mb-0 text-sm text-accent-red">{error}</p>}
        <div className="mt-6 flex flex-wrap items-center justify-end gap-2.5">
          <Button
            onClick={() => run(onCreateProject)}
            disabled={saving}
          >
            {m.tour_create_a_new_project()}
          </Button>
          <Button variant="primary"
            onClick={() => run(onClose)}
            disabled={saving}
          >
            {saving ? m.common_saving() : m.tour_explore_demo()}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
