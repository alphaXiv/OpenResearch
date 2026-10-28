
import { m } from "../paraglide/messages.js";
import { StatusIndicator, type StatusTone } from "./ui";

export interface StatusStyle {
  tone: StatusTone;
  live: boolean;
}

export const STATUS_STYLES: Record<string, StatusStyle> = {
  done: { tone: "success", live: false },
  failed: { tone: "danger", live: false },
  running: { tone: "info", live: true },
  starting: { tone: "warning", live: true },
  cancelling: { tone: "caution", live: true },
  cancelled: { tone: "caution", live: false },
  lost: { tone: "neutral", live: false },
  editing: { tone: "accent", live: true },
  idle: { tone: "neutral", live: false },
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
  lost: m.status_lost,
  editing: m.status_editing,
  idle: m.status_idle,
};

export function statusLabel(s: string): string {
  const localized = STATUS_LABELS[s];
  if (localized) return localized();
  return s.charAt(0).toUpperCase() + s.slice(1);
}

export function StatusBadge({ status, label, className }: { status: string; label?: string; className?: string }) {
  const style = statusStyle(status);
  const text = label ?? statusLabel(status);
  return (
    <StatusIndicator tone={style.tone} live={style.live} className={className}>
      <span className="truncate" title={text}>{text}</span>
    </StatusIndicator>
  );
}
