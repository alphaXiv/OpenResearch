import type { Run, RunContact, RunStatus } from "./api";

export type RunDisplayStatus =
  | RunStatus
  | "cancelling"
  | "cancellation_pending"
  | "not_fully_stopped"
  | "cancelled_unconfirmed"
  | "unreachable"
  | "needs_auth"
  | "unconfirmed"
  | "abandoned";

type RunState = Pick<Run, "status"> & Partial<Pick<Run, "contact" | "cancelling" | "lostReason" | "host" | "cancelUnconfirmed" | "waitingReason" | "cancelError">> & { cancelRequested?: boolean };

export interface RunDisplay {
  status: RunDisplayStatus;
  stopping?: { count: number; host: string };
  /** Why the run hasn't started, in the backend's words. */
  waiting?: string;
  /** The status is what was last seen, not live knowledge. */
  stale: boolean;
  /** When contact was lost, for the "when last seen" tooltip. */
  since: number | null;
}

export type RunNotice =
  | { kind: "unreachable"; host: string | null; since: number; cause: string }
  | { kind: "needs_auth"; host: string | null; since: number; cause: string }
  | { kind: "unconfirmed_launch"; detail: string }
  | { kind: "cancel_pending"; host: string | null; since: number; contact: "unreachable" | "needs_auth" }
  | { kind: "cancel_pending_launch"; detail: string | null }
  | { kind: "cancel_unconfirmed"; detail: string }
  | { kind: "cancel_refused"; detail: string }
  | { kind: "abandoned"; host: string | null }
  | { kind: "cancelling" }
  | { kind: "cancelled_unconfirmed"; unreadable: number[] }
  | { kind: "not_fully_stopped"; count: number; host: string }
  | { kind: "unreconciled"; detail: string }
  | { kind: "log_pending"; host: string | null }
  | { kind: "log_failed"; host: string | null; cause: string | null }
  | { kind: "notice_failed"; error: string }
  | { kind: "monitoring"; error: string };

type ActiveRunStatus = "submitting" | "starting" | "running";

export function runActive(status: RunStatus): status is ActiveRunStatus {
  return status === "submitting" || status === "starting" || status === "running";
}

function runCancelling(run: RunState): boolean {
  return run.cancelling ?? (runActive(run.status) && run.cancelRequested === true);
}

export function runAbandoned(run: Pick<Run, "status"> & Partial<Pick<Run, "lostReason">>): boolean {
  return run.status === "lost" && run.lostReason === "abandoned";
}

/** The run's result text, except an abandoned run's: its notice already says so. */
export function runResult(run: Pick<Run, "status" | "resultMarkdown"> & Partial<Pick<Run, "lostReason">>): string | null {
  return runAbandoned(run) ? null : run.resultMarkdown || null;
}

export function runDisplay(run: RunState): RunDisplay {
  const contact = run.contact ?? null;
  const since = contact?.since ?? null;

  if (run.status === "cancelled") {
    if (contact?.type === "stopping") {
      return { status: "not_fully_stopped", stopping: { count: contact.count, host: run.host ?? "localhost" }, stale: true, since };
    }
    if (run.cancelUnconfirmed) {
      return { status: "cancelled_unconfirmed", stale: false, since: null };
    }
  }
  if (!runActive(run.status)) {
    return { status: runAbandoned(run) ? "abandoned" : run.status, stale: contact !== null, since };
  }
  if (runCancelling(run)) {
    const pending = run.status === "submitting" || Boolean(run.cancelError) || contact?.type === "unreachable" || contact?.type === "needs_auth" || contact?.type === "unreconciled";
    return { status: pending ? "cancellation_pending" : "cancelling", stale: contact !== null, since };
  }
  if (contact === null) return { status: run.status, ...(run.waitingReason ? { waiting: run.waitingReason } : {}), stale: false, since };

  if (contact.type === "needs_auth") return { status: "needs_auth", stale: true, since };
  const status = contact.type === "unreconciled" || contact.type === "stopping" ? "unconfirmed" : contact.type;
  return { status, stale: true, since };
}

/** What the run detail strip says about a run, or null when there is nothing to add. */
type CollectionFields = Partial<Pick<Run, "logCollection" | "logCollectionError" | "noticeError">>;

export function runNotice(run: Pick<Run, "status" | "backend"> & Partial<Pick<Run, "contact" | "cancelling" | "lostReason" | "host" | "cancelUnconfirmed" | "cancelError">> & CollectionFields): RunNotice | null {
  const contact: RunContact | null = run.contact ?? null;
  const host = run.host ?? null;

  if (run.status === "cancelled") {
    if (contact?.type === "stopping") return { kind: "not_fully_stopped", count: contact.count, host: host ?? "localhost" };
    if (run.cancelUnconfirmed) return { kind: "cancelled_unconfirmed", unreadable: run.cancelUnconfirmed };
  }
  if (contact === null) {
    if (!runActive(run.status)) {
      if (run.logCollection === "pending") return { kind: "log_pending", host };
      if (run.logCollection === "failed") return { kind: "log_failed", host, cause: run.logCollectionError ?? null };
      return run.noticeError ? { kind: "notice_failed", error: run.noticeError } : null;
    }
    if (runCancelling(run) && run.cancelError) return { kind: "cancel_refused", detail: run.cancelError };
    if (runCancelling(run)) return run.status === "submitting" ? { kind: "cancel_pending_launch", detail: null } : { kind: "cancelling" };
    const error = run.backend?.monitoringError;
    return typeof error === "string" && error ? { kind: "monitoring", error } : null;
  }
  if (contact.type === "stopping") return null;
  if (!runActive(run.status)) {
    return runAbandoned(run)
      ? { kind: "abandoned", host }
      : { kind: "unreconciled", detail: contact.type === "unreconciled" ? contact.detail : contact.cause };
  }

  const submitting = run.status === "submitting";
  if (runCancelling(run)) {
    if (contact.type !== "unreconciled") return { kind: "cancel_pending", host, since: contact.since, contact: contact.type };
    return submitting
      ? { kind: "cancel_pending_launch", detail: contact.detail }
      : { kind: "cancel_unconfirmed", detail: contact.detail };
  }
  if (contact.type === "unreconciled") {
    return submitting ? { kind: "unconfirmed_launch", detail: contact.detail } : { kind: "unreconciled", detail: contact.detail };
  }
  return { kind: contact.type, host, since: contact.since, cause: contact.cause };
}

/** The run a notice is about: the newest one with something to say, since a forced relaunch can leave an older run live. */
export function noticeRun<R extends Parameters<typeof runNotice>[0] & Pick<Run, "createdAt">>(runs: R[]): R | null {
  const newestFirst = [...runs].sort((a, b) => b.createdAt - a.createdAt);
  return newestFirst.find((run) => runNotice(run) !== null) ?? null;
}

export function runApiProvider(run: Pick<Run, "backend">): string | null {
  const handle = run.backend?.handle;
  if (typeof handle === "object" && handle !== null && "type" in handle && handle.type === "Api" && "provider" in handle && typeof handle.provider === "string") return handle.provider;
  const descriptor = typeof handle === "object" && handle !== null && "descriptor" in handle ? handle.descriptor : run.backend;
  const kind = typeof descriptor === "object" && descriptor !== null && "kind" in descriptor ? descriptor.kind : null;
  return typeof kind === "string" && ["hf_job", "ray_job", "vercel_job", "modal_job", "k8s_job"].includes(kind) ? kind : null;
}

/** The host whose Connect this run is waiting on, if any. */
export function runConnectHost(run: Pick<Run, "host"> & Partial<Pick<Run, "contact" | "backend">>): string | null {
  return run.contact?.type === "needs_auth" && !runApiProvider(run) ? run.host : null;
}

/** Stop is offered until a cancel is pending; the status badge carries it from then on. */
export function runStoppable(run: RunState): boolean {
  return runActive(run.status) && !runCancelling(run);
}

/** Abandon is the way out for an active run whose outcome we cannot observe or whose stop keeps failing. */
export function runAbandonable(run: Pick<Run, "status"> & Partial<Pick<Run, "contact" | "cancelError">>): boolean {
  return runActive(run.status) && (Boolean(run.contact) || Boolean(run.cancelError));
}
