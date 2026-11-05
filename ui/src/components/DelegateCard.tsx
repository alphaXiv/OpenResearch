import { ChevronRight } from "lucide-react";

import type { ChatSession } from "../api";
import { delegateModelLabel, type DelegateStatus } from "../delegates";
import { m } from "../paraglide/messages.js";
import { HarnessLogo } from "./HarnessLogo";
import { Spinner } from "./ui";

export function delegateStatusLabel(status: DelegateStatus): string {
  switch (status) {
    case "running":
      return m.status_running();
    case "asking":
      return m.subagent_status_asking();
    case "approval":
      return m.subagent_status_approval();
    case "finished":
      return m.subagent_status_finished();
    case "failed":
      return m.status_failed();
    case "stopped":
      return m.subagent_status_stopped();
  }
}

/** A chat this one delegated to, in the transcript where it was started. */
export function DelegateCard({ session, status, onOpen }: {
  session: ChatSession;
  status: DelegateStatus;
  onOpen: (sessionId: string) => void;
}) {
  const model = delegateModelLabel(session);
  const preview = status === "running" || status === "asking" || status === "approval"
    ? null
    : session.delegate?.answer || session.delegate?.error;
  return (
    <button
      type="button"
      className="delegate-card flex w-full cursor-pointer items-start gap-3 rounded-lg border border-border bg-background px-3 py-2.5 text-start hover:bg-surface focus-visible:outline-2 focus-visible:outline-focus"
      title={m.subagent_open()}
      onClick={() => onOpen(session.id)}
    >
      <span className="flex h-6 shrink-0 items-center text-text">
        <HarnessLogo harness={session.harness} size={16} />
      </span>
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="flex min-w-0 items-baseline gap-2">
          <span className="truncate text-base font-medium text-text">{session.title?.trim() || model}</span>
          <span className={`shrink-0 text-sm ${status === "failed" ? "text-accent-red" : status === "asking" || status === "approval" ? "text-accent-amber" : "text-subtext"}`}>
            {delegateStatusLabel(status)}
          </span>
        </span>
        <span className="truncate text-sm text-subtext">{preview || model}</span>
      </span>
      <span className="flex h-6 shrink-0 items-center gap-2 text-muted">
        {status === "running" && <Spinner />}
        <ChevronRight size={14} aria-hidden="true" />
      </span>
    </button>
  );
}
