import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { RefreshCw, X } from "lucide-react";
import { useEffect, useState } from "react";
import { getUpdateStatus, type UpdateStatus } from "../api";
import { onUpdateStatus } from "../events";
import { IconButton } from "./ui";

export interface UpdateState {
  status: UpdateStatus | null;
  /** The initial fetch failed — distinct from "still loading". */
  error: string | null;
  /** Adopt a status returned by a mutating call, so the card reflects the write
   *  immediately instead of waiting for the next SSE sample. */
  apply: (status: UpdateStatus) => void;
}

/** Live update status: the initial fetch plus every `update.status` SSE frame.
 *  Exported because the Updates settings card renders the same state. */
export function useUpdateStatus(): UpdateState {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let fromEvent = false;
    const stop = onUpdateStatus((next) => {
      fromEvent = true;
      setStatus(next);
    });
    getUpdateStatus()
      // An SSE frame can land first; it is never staler than this fetch.
      .then((next) => !fromEvent && setStatus(next))
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));
    return stop;
  }, []);
  // `setStatus` is referentially stable, so this needs no memoization.
  return { status, error, apply: setStatus };
}

/** Shown once the updater has already installed a newer version: the app the
 *  user is looking at is the old one until it restarts.
 *
 *  Deliberately not shown for a merely *available* update — that is the
 *  updater's job, and a banner for something already in hand is noise. */
export function UpdateBanner() {
  const { status } = useUpdateStatus();
  const [dismissed, setDismissed] = useState<string | null>(null);

  // `installedVersion`, not `latest`: a release can land between the install and
  // the restart, and the banner must name what is actually on disk.
  const version = status?.restartRequired ? status.installedVersion : null;
  if (!version || dismissed === version) return null;

  return (
    <div
      className="update-banner flex items-center gap-2 shrink-0 py-1.5 px-3.5 text-sm text-text bg-surface border-b border-b-border"
      role="status"
    >
      <RefreshCw size={13} className="shrink-0 text-subtext" />
      <span className="min-w-0">
        {m.update_banner_complete({ version: ltr(version) })}
      </span>
      <IconButton
        type="button"
        size="small"
        className="ms-auto"
        aria-label={m.update_banner_dismiss()}
        onClick={() => setDismissed(version)}
      >
        <X size={13} />
      </IconButton>
    </div>
  );
}
