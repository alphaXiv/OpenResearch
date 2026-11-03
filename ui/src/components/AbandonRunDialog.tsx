import { useWorkspace } from "../workspace";
import { Dialog } from "@base-ui/react/dialog";
import { useState } from "react";
import { type Run } from "../api";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { EscapeScope } from "./EscapeScope";
import { Button } from "./ui";

export function AbandonRunDialog({ run, onClose }: { run: Pick<Run, "id" | "host">; onClose: () => void }) {
  const { abandonRun } = useWorkspace().api;

  const [abandoning, setAbandoning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function confirm() {
    setAbandoning(true);
    setError(null);
    try {
      await abandonRun(run.id);
      onClose();
    } catch (err) {
      setAbandoning(false);
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  return <EscapeScope onEscape={() => { if (!abandoning) onClose(); }}>
    <Dialog.Root open disablePointerDismissal={abandoning} onOpenChange={(open, details) => {
      if (open) return;
      if (abandoning) {
        details.event.preventDefault();
        details.cancel();
      } else onClose();
    }}>
      <Dialog.Portal>
        <Dialog.Backdrop className="fixed inset-0 z-200 bg-modal-backdrop" />
        <Dialog.Viewport className="fixed inset-0 z-200 flex items-center justify-center p-5">
          <Dialog.Popup
            className="w-120 max-w-full rounded-xl border border-border bg-background p-6 shadow-modal"
            aria-labelledby="abandon-run-title"
            aria-describedby="abandon-run-body"
          >
            <h2 id="abandon-run-title" className="m-0 text-xl font-medium text-text">{m.run_abandon_title()}</h2>
            <p id="abandon-run-body" className="mt-3 mb-0 text-sm text-text">
              {run.host ? m.run_abandon_body({ host: ltr(run.host) }) : m.run_abandon_body_no_host()}
            </p>
            {error && <p role="alert" className="mt-3 mb-0 text-sm text-accent-red">{error}</p>}
            <div className="mt-6 flex justify-end gap-2.5">
              <Button disabled={abandoning} onClick={onClose}>{m.settings_page_cancel()}</Button>
              <Button variant="danger" disabled={abandoning} onClick={() => void confirm()}>
                {abandoning ? m.run_abandoning() : m.run_abandon_confirm()}
              </Button>
            </div>
          </Dialog.Popup>
        </Dialog.Viewport>
      </Dialog.Portal>
    </Dialog.Root>
  </EscapeScope>;
}
