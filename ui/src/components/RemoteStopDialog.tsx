import { useRef } from "react";
import { createPortal } from "react-dom";
import type { RemoteStopPreview } from "../api";
import { fmtNumber, ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { Button } from "./ui";
import { useDialogFocus } from "./useDialogFocus";

export function RemoteStopDialog({
  host,
  preview,
  currentClientAttached,
  stopping,
  onClose,
  onConfirm,
}: {
  host: string;
  preview: RemoteStopPreview;
  currentClientAttached: boolean;
  stopping: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const otherClients = Math.max(0, preview.attachmentCount - (currentClientAttached ? 1 : 0));
  const impacts: string[] = [];

  if (preview.activeTurnCount > 0) {
    impacts.push(preview.activeTurnCount === 1
      ? m.remote_stop_one_turn()
      : m.remote_stop_turns({ count: fmtNumber(preview.activeTurnCount) }));
  }
  if (preview.pendingPermissionCount > 0) {
    impacts.push(preview.pendingPermissionCount === 1
      ? m.remote_stop_one_approval()
      : m.remote_stop_approvals({ count: fmtNumber(preview.pendingPermissionCount) }));
  }
  if (otherClients > 0) {
    impacts.push(otherClients === 1
      ? m.remote_stop_one_other_client()
      : m.remote_stop_other_clients({ count: fmtNumber(otherClients) }));
  }
  if (preview.queuedMessageCount > 0) {
    impacts.push(preview.queuedMessageCount === 1
      ? m.remote_stop_one_queued_message()
      : m.remote_stop_queued_messages({ count: fmtNumber(preview.queuedMessageCount) }));
  }
  if (preview.activeRunCount > 0) {
    impacts.push(preview.activeRunCount === 1
      ? m.remote_stop_one_experiment()
      : m.remote_stop_experiments({ count: fmtNumber(preview.activeRunCount) }));
  }

  useDialogFocus(dialogRef, onClose);

  return createPortal(
    <div
      className="fixed inset-0 z-200 flex items-center justify-center bg-modal-backdrop p-5"
      onClick={(event) => {
        if (!stopping && event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="w-120 max-w-full rounded-xl border border-border bg-background p-6 shadow-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="remote-stop-dialog-title"
        aria-describedby={impacts.length > 0 ? "remote-stop-dialog-impact" : undefined}
        tabIndex={-1}
      >
        <h2 id="remote-stop-dialog-title" className="m-0 text-xl font-medium text-text">
          {m.remote_stop_confirm_title({ host: ltr(host) })}
        </h2>
        {impacts.length > 0 && (
          <div id="remote-stop-dialog-impact" className="mt-4 text-sm text-text">
            <p className="m-0 font-medium">{m.remote_stop_impact_intro()}</p>
            <ul className="mt-2 mb-0 space-y-1 ps-5">
              {impacts.map((impact) => <li key={impact}>{impact}</li>)}
            </ul>
          </div>
        )}
        <div className="mt-6 flex justify-end gap-2.5">
          <Button disabled={stopping} onClick={onClose}>{m.settings_page_cancel()}</Button>
          <Button variant="danger" disabled={stopping} onClick={onConfirm}>
            {stopping ? m.remote_stopping_host() : m.remote_stop_confirm_action()}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
