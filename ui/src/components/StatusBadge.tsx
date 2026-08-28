// Mirror of openresearch.sh StatusBadge: sentence-case label + colored dot,
// live statuses pulse. STATUS_STYLES is the single source of truth for status
// coloring across the table, graph and drawer.

import { STATUS_BADGE_CLASS_NAME } from "../styleClasses";
import { m } from "../paraglide/messages.js";

export interface StatusStyle {
  className: string;
  live: boolean;
}

export const STATUS_STYLES: Record<string, StatusStyle> = {
  done: { className: "st-done", live: false },
  failed: { className: "st-failed", live: false },
  running: { className: "st-running", live: true },
  starting: { className: "st-starting", live: true },
  cancelling: { className: "st-cancelling", live: true },
  cancelled: { className: "st-cancelled", live: false },
  editing: { className: "st-editing", live: true },
  idle: { className: "st-idle", live: false },
};

export function statusStyle(status: string): StatusStyle {
  return STATUS_STYLES[status] ?? STATUS_STYLES.idle;
}

const STATUS_LABELS: Record<string, () => string> = {
  done: m.status_done,
  failed: m.status_failed,
  running: m.status_running,
  starting: m.status_starting,
  cancelling: m.status_cancelling,
  cancelled: m.status_cancelled,
  editing: m.status_editing,
  idle: m.status_idle,
};

export function statusLabel(s: string): string {
  const localized = STATUS_LABELS[s];
  if (localized) return localized();
  return s.charAt(0).toUpperCase() + s.slice(1);
}

export function StatusBadge({ status, label }: { status: string; label?: string }) {
  const style = statusStyle(status);
  return (
    <span className={`${STATUS_BADGE_CLASS_NAME} ${style.className}${style.live ? " live" : ""}`}>
      <span className="dot" />
      {label ?? statusLabel(status)}
    </span>
  );
}
