import { useWorkspace } from "../workspace";
import { CircleStop, Plug, RefreshCw } from "lucide-react";
import { useState } from "react";
import { type Run } from "../api";
import { hostForRun } from "../engineHosts";
import { fmtClock, ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { runAbandonable, runActive, runApiProvider, runConnectHost, runNotice, type RunNotice } from "../runDisplay";
import { statusLabel } from "./StatusBadge";
import { Button } from "./ui";
import { cn } from "./ui/cn";

type NoticeRun = Parameters<typeof runNotice>[0] & Pick<Run, "id" | "host">;

const TONES = {
  warning: "text-accent-amber",
  caution: "text-accent-orange",
  quiet: "text-subtext",
};

function copy(notice: RunNotice, nextRetryMs: number | null, provider: string | null): { lead: string; detail: string | null; tone: keyof typeof TONES } {
  const authLead = provider === "k8s_job" ? m.run_notice_cluster_auth() : provider ? m.run_notice_provider_auth() : null;
  const host = (name: string | null) => ltr(name ?? m.run_notice_the_host());
  switch (notice.kind) {
    case "monitoring":
      return { lead: notice.error, detail: null, tone: "warning" };
    case "unreachable": {
      const lead = m.run_notice_unreachable({ host: host(notice.host), time: fmtClock(notice.since) });
      const retry = nextRetryMs === null ? "" : ` ${m.run_notice_retry_at({ time: fmtClock(nextRetryMs) })}`;
      return { lead: lead + retry, detail: notice.cause, tone: "warning" };
    }
    case "needs_auth":
      return { lead: authLead ?? m.run_notice_needs_auth({ host: host(notice.host) }), detail: notice.cause, tone: "warning" };
    case "cancelling":
      return { lead: m.status_cancelling(), detail: null, tone: "caution" };
    case "cancel_pending":
      return notice.contact === "unreachable"
        ? { lead: m.status_cancellation_pending(), detail: `${m.run_notice_cancel_pending({ host: host(notice.host), time: fmtClock(notice.since) })} ${m.run_notice_cancel_pending_later()}`, tone: "warning" }
        : { lead: m.status_cancellation_pending(), detail: authLead ?? m.run_notice_cancel_pending_auth({ host: host(notice.host) }), tone: "warning" };
    case "unconfirmed_launch":
      return { lead: m.run_notice_unconfirmed_launch(), detail: notice.detail, tone: "caution" };
    case "cancel_pending_launch":
      return { lead: m.status_cancellation_pending(), detail: m.run_notice_cancel_pending_launch(), tone: "caution" };
    case "cancel_unconfirmed":
      return { lead: m.status_cancelling(), detail: notice.detail, tone: "caution" };
    case "not_fully_stopped":
      return { lead: statusLabel(notice.kind, notice), detail: null, tone: "caution" };
    case "cancelled_unconfirmed": {
      const detail = notice.unreadable.length
        ? m.run_notice_cancel_unreadable({ pids: ltr(notice.unreadable.join(", ")) })
        : m.run_notice_cancel_uncovered();
      return { lead: m.status_cancelled_unconfirmed(), detail, tone: "caution" };
    }
    case "unreconciled":
      return { lead: m.run_notice_unreconciled(), detail: notice.detail, tone: "caution" };
    case "log_pending":
      return { lead: m.run_notice_log_pending({ host: host(notice.host) }), detail: null, tone: "quiet" };
    case "log_failed":
      return { lead: m.run_notice_log_failed({ host: host(notice.host) }), detail: notice.cause, tone: "warning" };
    case "notice_failed":
      return { lead: m.run_notice_post_failed({ error: notice.error }), detail: null, tone: "warning" };
    case "abandoned":
      return {
        lead: notice.host ? m.run_notice_abandoned({ host: ltr(notice.host) }) : m.run_notice_abandoned_no_host(),
        detail: null,
        tone: "quiet",
      };
  }
}

/** What a run's status alone can't say: lost contact, a pending cancel, or an abandoned job's pending kill. */
export function RunContactNotice({ run, className }: { run: NoticeRun; className?: string }) {
  const { abandonRun, cancelRun, cleanupRun, retryEngineHost } = useWorkspace().api;
  const { showConfirmDialog } = useWorkspace().dialogs;
  const { requestHostConnect } = useWorkspace().events;
  const { setEngineHost, useEngineHosts } = useWorkspace().queries;

  const hosts = useEngineHosts();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const notice = runNotice(run);
  if (!notice) return null;

  const engineHost = hostForRun(hosts, run);
  const nextRetryMs = engineHost?.status === "backoff" ? engineHost.nextRetryMs : null;
  const { lead, detail, tone } = copy(notice, nextRetryMs, runApiProvider(run));
  const rentalBilling = run.backend?.kind === "openresearch_job"
    && runActive(run.status)
    && (run.contact?.type === "needs_auth" || run.contact?.type === "unreachable");
  const killPending = notice.kind === "not_fully_stopped";
  const connectHost = runActive(run.status) || killPending
    ? runConnectHost(run) ?? (killPending && engineHost?.status === "needs_auth" ? run.host : null)
    : null;
  const waitsOnUnreachable = notice.kind === "unreachable" || (notice.kind === "cancel_pending" && notice.contact === "unreachable") || (killPending && engineHost?.status === "backoff");
  const retryHost = waitsOnUnreachable && engineHost?.status === "backoff" && engineHost.class === "key" ? engineHost.alias : null;
  const stopAgain = notice.kind === "cancel_unconfirmed";
  const abandonable = runAbandonable(run);

  async function act(action: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  // Message and actions stack at every width: beside each other, a narrow drawer squeezes the text into a column.
  return (
    <div className={cn("run-contact-notice flex flex-col gap-1.5 text-sm", className)}>
      <div role="status" className="min-w-0">
        <p className={cn("m-0 wrap-anywhere", notice.kind !== "monitoring" && "font-medium", TONES[tone])}>{lead}</p>
        {rentalBilling && <p className="m-0 mt-0.5 text-xs text-subtext wrap-anywhere">{m.run_notice_rental_billing()}</p>}
        {detail && <p className="m-0 mt-0.5 text-xs text-subtext wrap-anywhere">{detail}</p>}
        {error && <p role="alert" className="m-0 mt-0.5 text-xs text-accent-red wrap-anywhere">{error}</p>}
      </div>
      {(connectHost || retryHost || rentalBilling || stopAgain || killPending || abandonable) && (
        <div className="run-contact-actions flex flex-wrap items-center gap-1">
          {connectHost && (
            <Button size="small" variant="primary" onClick={() => requestHostConnect(connectHost)}>
              <Plug size={14} />{m.settings_connect()}
            </Button>
          )}
          {retryHost && (
            <Button size="small" variant="ghost" disabled={busy} onClick={() => void act(async () => setEngineHost(await retryEngineHost(retryHost)))}>
              <RefreshCw size={14} />{m.app_retry()}
            </Button>
          )}
          {rentalBilling && (
            <Button size="small" variant="ghost" disabled={busy} onClick={() => void act(() => cancelRun(run.id))}>
              <CircleStop size={14} />{m.run_stop_box()}
            </Button>
          )}
          {stopAgain && (
            <Button size="small" variant="ghost" disabled={busy} onClick={() => void act(() => cancelRun(run.id))}>
              <CircleStop size={14} />{m.run_stop_again()}
            </Button>
          )}
          {killPending && (
            <Button size="small" variant="ghost" disabled={busy} onClick={() => void act(() => cleanupRun(run.id))}>
              <CircleStop size={14} />{m.run_stop_now()}
            </Button>
          )}
          {abandonable && (
            <Button size="small" variant="ghost" className="text-accent-red" onClick={() => showConfirmDialog({
              title: m.run_abandon_title(),
              description: <>{run.contact?.type === "unreachable" && <>{m.run_abandon_unreachable()} </>}{m.run_abandon_body()}</>,
              destructive: true,
              confirmLabel: m.run_abandon_confirm(),
              cancelLabel: m.run_abandon_keep(),
              onCancel: "close",
              onConfirm: async () => { await abandonRun(run.id); },
            })}>
              {m.run_abandon()}
            </Button>
          )}
        </div>
      )}
    </div>
  );
}
