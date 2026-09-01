import { m } from "../paraglide/messages.js";
import { CircleAlert } from "lucide-react";
import { useSyncExternalStore } from "react";
import { isConnected, onConnectionChange } from "../events";

/** Shown while the event stream is down: everything on screen is a snapshot
 *  from whenever the backend was last reachable.
 *
 *  A banner rather than a blocking overlay — the transcript, logs, and diffs
 *  already rendered stay worth reading — and not dismissible, since nothing
 *  else on the page distinguishes stale from live. */
export function OfflineBanner() {
  const connected = useSyncExternalStore(onConnectionChange, isConnected, isConnected);

  return (
    <>
      {/* Mounted whatever the state, because a live region inserted together
          with its text is missed by VoiceOver and NVDA. */}
      <span className="sr-only" role="status" aria-live="polite">
        {connected ? "" : m.offline_banner_disconnected()}
      </span>
      {/* The bar is hidden from the a11y tree because the span above carries the
          text; keep this subtree free of anything focusable. */}
      {!connected && (
        <div
          className="offline-banner flex items-center gap-2 shrink-0 py-1.5 px-3.5 text-sm text-text bg-accent-amber-subtle border-b border-b-accent-amber"
          aria-hidden
        >
          <CircleAlert size={13} className="shrink-0 text-accent-amber" />
          <span dir="auto" className="min-w-0">{m.offline_banner_disconnected()}</span>
        </div>
      )}
    </>
  );
}
