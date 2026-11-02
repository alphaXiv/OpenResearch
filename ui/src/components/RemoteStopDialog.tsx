import type { DialogContentProps } from "../dialog";
import type { RemoteStopPreview } from "../api";
import { fmtNumber, ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { Button, DialogContent, DialogFooter, DialogTitle, Spinner } from "./ui";

export function RemoteStopDialog({
  host,
  preview,
  currentClientAttached,
  isDisabled,
  isPending,
  errorMessage,
  cancel,
  confirm,
}: DialogContentProps<void> & {
  host: string;
  preview: RemoteStopPreview;
  currentClientAttached: boolean;
}) {
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

  return <DialogContent
    aria-labelledby="remote-stop-dialog-title"
    aria-describedby={impacts.length > 0 ? "remote-stop-dialog-impact" : undefined}
  >
    <DialogTitle id="remote-stop-dialog-title">
      {m.remote_stop_confirm_title({ host: ltr(host) })}
    </DialogTitle>
    {impacts.length > 0 && (
      <div id="remote-stop-dialog-impact" className="mt-4 text-sm text-text">
        <p className="m-0 font-medium">{m.remote_stop_impact_intro()}</p>
        <ul className="mt-2 mb-0 space-y-1 ps-5">
          {impacts.map((impact) => <li key={impact}>{impact}</li>)}
        </ul>
      </div>
    )}
    {errorMessage !== null && <p role="alert" className="mt-3 mb-0 text-sm text-accent-red">{errorMessage}</p>}
    <DialogFooter>
      <Button disabled={isDisabled} onClick={cancel}>{m.settings_page_cancel()}</Button>
      <Button variant="danger" disabled={isDisabled} onClick={() => confirm()}>
        {isPending && <Spinner />}
        {isPending ? m.remote_stopping_host() : m.remote_stop_confirm_action()}
      </Button>
    </DialogFooter>
  </DialogContent>;
}
