import { autoDir, fmtClock, fmtNumber, ltr } from "../i18n";
import { getLocale } from "../paraglide/runtime.js";
import type { RunDisplay } from "../runDisplay";
import { m } from "../paraglide/messages.js";
import { StatusIndicator, type StatusTone } from "./ui";
import { cn } from "./ui/cn";

export interface StatusStyle {
  tone: StatusTone;
  live: boolean;
}

const STATUS_STYLES: Record<string, StatusStyle> = {
  done: { tone: "success", live: false },
  failed: { tone: "danger", live: false },
  submitting: { tone: "info", live: true },
  running: { tone: "info", live: true },
  starting: { tone: "warning", live: true },
  cancelling: { tone: "caution", live: true },
  cancellation_pending: { tone: "warning", live: false },
  not_fully_stopped: { tone: "caution", live: false },
  cancelled: { tone: "caution", live: false },
  cancelled_unconfirmed: { tone: "caution", live: false },
  unreachable: { tone: "warning", live: false },
  needs_auth: { tone: "warning", live: false },
  unconfirmed: { tone: "caution", live: false },
  lost: { tone: "neutral", live: false },
  abandoned: { tone: "neutral", live: false },
  editing: { tone: "accent", live: true },
  idle: { tone: "neutral", live: false },
};

export function statusStyle(status: string): StatusStyle {
  return STATUS_STYLES[status] ?? STATUS_STYLES.idle;
}

const STATUS_LABELS: Record<string, () => string> = {
  done: m.status_done,
  failed: m.status_failed,
  submitting: m.status_submitting,
  running: m.status_running,
  starting: m.status_starting,
  cancelling: m.status_cancelling,
  cancellation_pending: m.status_cancellation_pending,
  cancelled: m.status_cancelled,
  not_fully_stopped: m.status_not_fully_stopped,
  cancelled_unconfirmed: m.status_cancelled_unconfirmed,
  unreachable: m.status_unreachable,
  needs_auth: m.status_needs_auth,
  unconfirmed: m.status_unconfirmed,
  lost: m.status_lost,
  abandoned: m.status_abandoned,
  editing: m.status_editing,
  idle: m.status_idle,
};

export function statusLabel(s: string, stopping?: RunDisplay["stopping"]): string {
  if (stopping) {
    const plural = new Intl.PluralRules(getLocale()).select(stopping.count);
    const messages = {
      zero: m.status_stopping_zero,
      one: m.status_stopping_one,
      two: m.status_stopping_two,
      few: m.status_stopping_few,
      many: m.status_stopping_many,
      other: m.status_stopping_other,
    };
    return messages[plural]({ count: ltr(fmtNumber(stopping.count)), host: ltr(stopping.host) });
  }
  const localized = STATUS_LABELS[s];
  if (localized) return localized();
  return s.charAt(0).toUpperCase() + s.slice(1);
}

/** Spread a `runDisplay(run)` into it; `stale` and `since` mark a status that was last seen, not live. */
export function StatusBadge({ status, stopping, waiting, stale = false, since = null, className }: { status: string; stopping?: RunDisplay["stopping"]; waiting?: string; stale?: boolean; since?: number | null; className?: string }) {
  const style = statusStyle(status);
  const text = statusLabel(status);
  const full = statusLabel(status, stopping);
  const note = waiting ? autoDir(waiting) : stale && since !== null ? m.status_last_seen({ time: fmtClock(since) }) : null;
  return (
    <StatusIndicator tone={style.tone} live={style.live} stale={stale} className={className}>
      <span className={cn("min-w-0 truncate", waiting && "max-w-80")} title={note ? `${full} · ${note}` : full}>
        {text}
        {waiting && <span className="font-normal text-subtext"> · {autoDir(waiting)}</span>}
      </span>
    </StatusIndicator>
  );
}
