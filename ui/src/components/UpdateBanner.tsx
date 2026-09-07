import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { RefreshCw, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { getUpdateStatus, restartApp, type UpdateStatus } from "../api";
import { onUpdateStatus } from "../events";
import { Button, IconButton } from "./ui";

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
export function useUpdateStatus(enabled = true): UpdateState {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!enabled) return;
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
  }, [enabled]);
  // `setStatus` is referentially stable, so this needs no memoization.
  return { status, error, apply: setStatus };
}

/** How long to wait for the relaunched server before giving up on the reload. */
const RESTART_TIMEOUT_MS = 60_000;
const RESTART_POLL_MS = 500;

export interface RestartState {
  restarting: boolean;
  error: string | null;
  restart: () => void;
}

/** Ask the server to relaunch, then reload once a different server process
 *  answers — the page itself is the old build until it reloads. The old server answers the POST and then drops every connection,
 *  so the polling errors in between are expected and swallowed. */
export function useRestartApp(status: UpdateStatus | null): RestartState {
  const [restarting, setRestarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const unmounted = useRef(false);
  useEffect(() => {
    unmounted.current = false;
    return () => {
      unmounted.current = true;
    };
  }, []);

  const previous = status?.restartRequired ? status.instance : null;
  const restart = () => {
    if (!previous || restarting) return;
    setRestarting(true);
    setError(null);
    void (async () => {
      try {
        await restartApp();
        const deadline = Date.now() + RESTART_TIMEOUT_MS;
        while (Date.now() < deadline) {
          await new Promise((resolve) => setTimeout(resolve, RESTART_POLL_MS));
          const next = await getUpdateStatus().catch(() => null);
          if (next && next.instance !== previous) {
            window.location.reload();
            return;
          }
        }
        throw new Error(m.update_banner_restart_timed_out());
      } catch (e) {
        if (unmounted.current) return;
        setError(e instanceof Error ? e.message : String(e));
        setRestarting(false);
      }
    })();
  };
  return { restarting, error, restart };
}

/** Shown once the updater has already installed a newer version: the app the
 *  user is looking at is the old one until it restarts.
 *
 *  Deliberately not shown for a merely *available* update — that is the
 *  updater's job, and a banner for something already in hand is noise. */
export function UpdateBanner({ status }: { status: UpdateStatus | null }) {
  const [dismissed, setDismissed] = useState<string | null>(null);

  // `installedVersion`, not `latest`: a release can land between the install and
  // the restart, and the banner must name what is actually on disk.
  const version = status?.restartRequired ? status.installedVersion : null;
  const { restarting, error, restart } = useRestartApp(status);
  if (!version || dismissed === version) return null;

  return (
    <div
      className="update-banner flex items-center gap-2 shrink-0 py-1.5 px-3.5 text-sm text-text bg-surface border-b border-b-border"
      role="status"
    >
      <RefreshCw size={13} className={`shrink-0 text-subtext${restarting ? " animate-spin" : ""}`} />
      <span className="min-w-0">
        {error
          ? m.update_banner_restart_failed({ error })
          : m.update_banner_complete({ version: ltr(version) })}
      </span>
      {status?.canRestart && (
        <Button type="button" size="small" disabled={restarting} onClick={restart}>
          {restarting ? m.update_banner_restarting() : m.update_banner_restart()}
        </Button>
      )}
      <IconButton
        type="button"
        size="small"
        className="ms-auto"
        aria-label={m.update_banner_dismiss()}
        disabled={restarting}
        onClick={() => setDismissed(version)}
      >
        <X size={13} />
      </IconButton>
    </div>
  );
}
