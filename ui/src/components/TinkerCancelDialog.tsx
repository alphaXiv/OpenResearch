import { useEffect, useRef, useState } from "react";
import { backendKind, type Run } from "../api";
import { BUTTON_CLASS_NAME, PRIMARY_BUTTON_CLASS_NAME } from "../styleClasses";

const TINKER_CONSOLE_URL = "https://tinker-console.thinkingmachines.ai/";

export function isTinkerRun(run: Run): boolean {
  return backendKind(run.backend) === "tinker_job";
}

export function tinkerConsoleUrl(run: Run): string {
  const url = run.backend?.url;
  if (typeof url === "string") {
    try {
      const parsed = new URL(url);
      if (parsed.protocol === "https:" && parsed.hostname === "tinker-console.thinkingmachines.ai") {
        return parsed.href;
      }
    } catch {
      // Fall through to the canonical console.
    }
  }
  return TINKER_CONSOLE_URL;
}

export function TinkerCancelDialog({
  run,
  onCancel,
  onClose,
}: {
  run: Run;
  onCancel: (runId: string) => Promise<void>;
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  onCloseRef.current = onClose;

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const focusable = () =>
      [...dialog.querySelectorAll<HTMLElement>('button:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])')];
    (focusable()[0] ?? dialog).focus();
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onCloseRef.current();
        return;
      }
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
    document.addEventListener("keydown", handleKeyDown, true);
    return () => {
      document.removeEventListener("keydown", handleKeyDown, true);
      previousFocus?.focus();
    };
  }, []);

  async function stop(openConsole: boolean) {
    setPending(true);
    setError(null);
    const cancellation = onCancel(run.id);
    if (openConsole) window.open(tinkerConsoleUrl(run), "_blank", "noopener,noreferrer");
    try {
      await cancellation;
      onClose();
    } catch (cause) {
      setPending(false);
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  return (
    <div
      className="modal-backdrop fixed inset-0 bg-[rgba(29,_27,_26,_0.42)] flex items-center justify-center p-5 overflow-y-auto z-100"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="modal w-125 max-w-full bg-background border border-border rounded-xl shadow-[0_24px_60px_rgba(0,_0,_0,_0.22)] p-6"
        role="dialog"
        aria-modal="true"
        aria-labelledby="tinker-cancel-title"
        aria-describedby="tinker-cancel-description"
        tabIndex={-1}
      >
        <h2 id="tinker-cancel-title" className="mt-0 mb-3 text-xl">Stop Tinker controller?</h2>
        <div id="tinker-cancel-description" className="flex flex-col gap-2 text-md leading-normal text-subtext">
          <p className="m-0">
            Stopping the local controller prevents new requests. Operations already accepted by
            Tinker may continue until you cancel them in Tinker or they expire.
          </p>
          {error && <p className="m-0 text-accent-red" role="alert">Stop failed: {error}</p>}
        </div>
        <div className="mt-5 flex flex-wrap justify-end gap-2">
          <button className={BUTTON_CLASS_NAME} disabled={pending} onClick={onClose}>Keep running</button>
          <button className={BUTTON_CLASS_NAME} disabled={pending} onClick={() => void stop(false)}>
            Stop controller only
          </button>
          <button className={PRIMARY_BUTTON_CLASS_NAME} disabled={pending} onClick={() => void stop(true)}>
            {pending ? "Stopping…" : "Stop controller & open Tinker"}
          </button>
        </div>
      </div>
    </div>
  );
}
