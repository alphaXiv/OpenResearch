import { useQuery } from "@tanstack/react-query";
import { useWorkspace } from "../workspace";
import { mutAbandonRun, mutCancelRun, mutCleanupRun } from "../queries/projects";
import { mutRetryEngineHost } from "../queries/settings";
import { CircleStop, Plug, RefreshCw, type LucideIcon } from "lucide-react";
import { useState } from "react";
import { type Run } from "../api";
import { hostForRun } from "../engineHosts";
import { fmtClock, ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { runAbandonable, runActive, runApiProvider, runConnectHost, runNotice, type RunNotice } from "../runDisplay";
import { statusLabel } from "./StatusBadge";
import { Button } from "./ui";
import { cn } from "./ui/cn";
import { confirmStop } from "./run/confirmStop";

type NoticeRun = Parameters<typeof runNotice>[0] & Pick<Run, "id" | "projectId" | "experimentId" | "host">;

const TONES = {
  warning: "text-ink-amber",
  caution: "text-accent-orange",
  quiet: "text-subtext",
};

/** The words for every contact state, wherever a run shows; `figure` marks the one value a line draws in bold. */
export function runNoticeCopy(notice: RunNotice, nextRetryMs: number | null, provider: string | null, figure = (value: string) => value): { lead: string; detail: string | null; next?: string; tone: keyof typeof TONES } {
  const authLead = provider === "k8s_job" ? m.run_notice_cluster_auth() : provider ? m.run_notice_provider_auth() : null;
  const host = (name: string | null) => ltr(name ?? m.run_notice_the_host());
  switch (notice.kind) {
    case "monitoring":
      return { lead: notice.error, detail: null, tone: "warning" };
    case "unreachable": {
      const lead = m.run_notice_unreachable({ host: host(notice.host), time: figure(fmtClock(notice.since)) });
      const retry = nextRetryMs === null ? "" : ` ${m.run_notice_retry_at({ time: ltr(fmtClock(nextRetryMs)) })}`;
      return { lead: lead + retry, detail: notice.cause, tone: "warning" };
    }
    case "needs_auth":
      return { lead: authLead ?? m.run_notice_needs_auth({ host: figure(host(notice.host)) }), detail: notice.cause, tone: "warning" };
    case "cancelling":
      return { lead: m.status_cancelling(), detail: null, tone: "caution" };
    case "cancel_pending":
      return notice.contact === "unreachable"
        ? { lead: m.status_cancellation_pending(), detail: `${m.run_notice_cancel_pending({ host: host(notice.host), time: fmtClock(notice.since) })} ${m.run_notice_cancel_pending_later()}`, tone: "warning" }
        : { lead: m.status_cancellation_pending(), detail: authLead ?? m.run_notice_cancel_pending_auth({ host: host(notice.host) }), tone: "warning" };
    case "unconfirmed_launch":
      return { lead: m.run_notice_unconfirmed_launch(), detail: notice.detail, tone: "caution" };
    case "cancel_pending_launch":
      return notice.detail
        ? { lead: m.status_cancellation_pending(), detail: notice.detail, next: m.run_notice_cancel_pending_launch_unknown(), tone: "caution" }
        : { lead: m.status_cancellation_pending(), detail: m.run_notice_cancel_pending_launch(), tone: "caution" };
    case "cancel_unconfirmed":
      return { lead: m.status_cancellation_pending(), detail: notice.detail, tone: "caution" };
    case "cancel_refused":
      return { lead: m.status_cancellation_pending(), detail: notice.detail, next: m.run_notice_cancel_refused(), tone: "caution" };
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

/** One way forward from a contact notice. */
export interface ContactAction {
  key: string;
  label: string;
  icon?: LucideIcon;
  tone: "primary" | "ghost" | "danger";
  /** It asks for confirmation first. */
  dialog?: boolean;
  disabled?: boolean;
  run: () => void;
}

/** A run's contact notice in words, and what can be done about it; null when its status says it all. */
export function useRunContact(run: NoticeRun) {
  const workspace = useWorkspace();
  const { showConfirmDialog } = workspace.dialogs;
  const { requestHostConnect } = workspace.events;
  const { useEngineHosts, listExperimentsQuery } = workspace.queries;

  const hosts = useEngineHosts();
  const experiment = useQuery(listExperimentsQuery(run.projectId)).data?.find((row) => row.id === run.experimentId);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<{ notice: string; message: string } | null>(null);
  const notice = runNotice(run);
  if (!notice) return null;

  // A failed action speaks only for the notice it was taken on, not a later outage of the same kind.
  const noticeKey = `${notice.kind}:${"since" in notice ? notice.since : ""}`;
  const error = failure?.notice === noticeKey ? failure.message : null;

  const engineHost = hostForRun(hosts, run);
  const nextRetryMs = engineHost?.status === "backoff" ? engineHost.nextRetryMs : null;
  const copy = runNoticeCopy(notice, nextRetryMs, runApiProvider(run));
  const rentalBilling = run.backend?.kind === "openresearch_job"
    && runActive(run.status)
    && (run.contact?.type === "needs_auth" || run.contact?.type === "unreachable");
  const killPending = notice.kind === "not_fully_stopped";
  const connectHost = killPending
    ? runConnectHost(run) ?? (engineHost?.status === "needs_auth" ? run.host : null)
    : runActive(run.status) ? runConnectHost(run) : null;
  const waitsOnUnreachable = notice.kind === "unreachable" || (notice.kind === "cancel_pending" && notice.contact === "unreachable") || (killPending && engineHost?.status === "backoff");
  const retryHost = waitsOnUnreachable && engineHost?.status === "backoff" && engineHost.class === "key" ? engineHost.alias : null;
  const stopAgain = notice.kind === "cancel_unconfirmed";
  const abandonable = runAbandonable(run);

  const stop = () => confirmStop(
    workspace.dialogs,
    experiment?.title || experiment?.slug || run.experimentId,
    () => mutCancelRun.runAsHeadlessPromise(workspace, run, {}),
  );

  async function act(action: () => Promise<void>) {
    setBusy(true);
    setFailure(null);
    try {
      await action();
    } catch (err) {
      setFailure({ notice: noticeKey, message: err instanceof Error ? err.message : String(err) });
    } finally {
      setBusy(false);
    }
  }

  const abandon = () => showConfirmDialog({
    title: m.run_abandon_title(),
    description: <>{run.contact?.type === "unreachable" && <>{m.run_abandon_unreachable()} </>}{m.run_abandon_effect()}</>,
    destructive: true,
    confirmLabel: m.run_abandon_confirm(),
    cancelLabel: m.run_abandon_keep(),
    onCancel: "close",
    onConfirm: async () => { await mutAbandonRun.runAsHeadlessPromise(workspace, run.id, {}); },
  });
  const offered: (ContactAction | false | null | "")[] = [
    connectHost && { key: "connect", label: m.settings_connect(), icon: Plug, tone: "primary", run: () => requestHostConnect(connectHost) },
    retryHost && { key: "retry", label: m.app_retry(), icon: RefreshCw, tone: "ghost", disabled: busy, run: () => void act(async () => { await mutRetryEngineHost.runAsHeadlessPromise(workspace, retryHost, {}); }) },
    rentalBilling && { key: "stop-box", label: m.run_stop_box(), icon: CircleStop, tone: "ghost", dialog: true, disabled: busy, run: stop },
    stopAgain && { key: "stop-again", label: m.run_stop_again(), icon: CircleStop, tone: "ghost", dialog: true, disabled: busy, run: stop },
    killPending && { key: "stop-now", label: m.run_stop_now(), icon: CircleStop, tone: "ghost", disabled: busy, run: () => void act(() => mutCleanupRun.runAsHeadlessPromise(workspace, run.id, {})) },
    abandonable && { key: "abandon", label: m.run_abandon(), tone: "danger", dialog: true, run: abandon },
  ];
  const actions = offered.filter((action): action is ContactAction => Boolean(action));

  return { notice, ...copy, rentalBilling, error, actions };
}

/** The lines under a notice's lead: what a rented box still costs, the cause, and what happens next. */
export function RunContactDetails({ contact, cause = true }: { contact: NonNullable<ReturnType<typeof useRunContact>>; cause?: boolean }) {
  const { rentalBilling, detail, next } = contact;
  return <>
    {rentalBilling && <p className="m-0 mt-0.5 text-xs text-subtext wrap-anywhere">{m.run_notice_rental_billing()}</p>}
    {cause && detail && <p className="m-0 mt-0.5 text-xs text-subtext wrap-anywhere">{detail}</p>}
    {next && <p className="m-0 mt-0.5 text-xs text-subtext wrap-anywhere">{next}</p>}
  </>;
}

/** What a run's status alone can't say: lost contact, a pending cancel, or an abandoned job's pending kill. */
export function RunContactNotice({ run, className }: { run: NoticeRun; className?: string }) {
  const contact = useRunContact(run);
  if (!contact) return null;

  // Message and actions stack at every width: beside each other, a narrow drawer squeezes the text into a column.
  return (
    <div className={cn("run-contact-notice flex flex-col gap-1.5 text-sm", className)}>
      <div role="status" className="min-w-0">
        <p className={cn("m-0 wrap-anywhere", contact.notice.kind !== "monitoring" && "font-medium", TONES[contact.tone])}>{contact.lead}</p>
        <RunContactDetails contact={contact} />
        {contact.error && <p role="alert" className="m-0 mt-0.5 text-xs text-accent-red wrap-anywhere">{contact.error}</p>}
      </div>
      {contact.actions.length > 0 && (
        <div className="run-contact-actions flex flex-wrap items-center gap-1">
          {contact.actions.map(({ key, label, icon: Icon, tone, dialog, disabled, run: onClick }) => (
            <Button key={key} size="small" variant={tone === "primary" ? "primary" : "ghost"} className={tone === "danger" ? "text-primary" : undefined}
              disabled={disabled} aria-haspopup={dialog ? "dialog" : undefined} onClick={onClick}>
              {Icon && <Icon size={14} />}{label}
            </Button>
          ))}
        </div>
      )}
    </div>
  );
}
