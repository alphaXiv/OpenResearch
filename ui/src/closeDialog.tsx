import { useQuery } from "@tanstack/react-query";
import { useEffect, useRef, type ReactNode } from "react";
import type { AfterQuit, PendingRun, PendingWork } from "./api";
import { BackendLogo } from "./components/BackendLogos";
import { HarnessLogo } from "./components/HarnessLogo";
import { Button, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, MenuItem } from "./components/ui";
import type { createDialogs, DialogContentProps } from "./dialog";
import { m } from "./paraglide/messages.js";
import type { Workspace } from "./workspace";
import { taskLocation } from "./workspaceState";

interface CloseRequest {
  /** What the window found when asked to close; null when it could not count the work. */
  work: PendingWork | null;
  /** Whether keeping the work running minimizes the window, where no Dock shows a hidden app. */
  minimizes: boolean;
}

type CloseAnswer = "background" | "quit";

const needsAppReasons: Record<string, () => string> = {
  local_job: m.close_pending_reason_local,
  tinker_job: m.close_pending_reason_tinker,
  vercel_job: m.close_pending_reason_vercel,
  openresearch_job: m.close_pending_reason_box,
};

function PendingRow({ icon, name, project, detail, disabled, onOpen }: {
  icon: ReactNode;
  name: string;
  project: string | null;
  detail?: string;
  disabled: boolean;
  onOpen: () => void;
}) {
  return (
    <li>
      <MenuItem className="items-start gap-2.5 text-text" disabled={disabled} onClick={onOpen}>
        <span className="flex h-5 shrink-0 items-center">{icon}</span>
        <span className="flex min-w-0 flex-1 flex-col">
          <span dir="auto" className="wrap-anywhere">{name}</span>
          {detail && <span className="text-xs text-muted">{detail}</span>}
        </span>
        {project && <span dir="auto" className="max-w-40 shrink-0 text-muted wrap-anywhere">{project}</span>}
      </MenuItem>
    </li>
  );
}

interface Machine {
  /** The machine whose work the window would interrupt. */
  machine: Workspace;
  open: (href: string) => void;
}

function CloseDialogContent({ work: found, minimizes, machine, open, confirm, cancel, isDisabled }: DialogContentProps<CloseAnswer> & CloseRequest & Machine) {
  const live = useQuery({ ...machine.queries.getPendingWorkQuery(), initialData: found ?? undefined, gcTime: 0 }, machine.queryClient);
  const work = live.data ?? null;
  const quitRef = useRef<HTMLButtonElement>(null);
  const backgroundRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    asking = true;
    window.ipc?.postMessage("close:asking");
    // A workspace that goes away takes its dialog with it, and the next close must ask again.
    return () => void (asking = false);
  }, []);

  const runs = (fate: AfterQuit) => work?.runs.filter((run) => run.afterQuit === fate) ?? [];
  const blocked = runs("needsApp").length > 0;
  const finished = work !== null && work.agents.length === 0 && work.runs.length === 0;
  // Nothing left to ask about: the close is dropped and the user stays where they were.
  useEffect(() => {
    if (finished) cancel();
  }, [finished, cancel]);
  const go = (href: string) => {
    cancel();
    open(href);
  };

  const agentRows = (work?.agents ?? []).map((agent) => {
    const href = agent.sideParentId
      ? taskLocation(agent.projectId, agent.sideParentId, { kind: "side", sessionId: agent.sessionId })
      : taskLocation(agent.projectId, agent.sessionId);
    const untitled = agent.sideParentId ? m.side_chat_untitled() : m.chat_untitled();
    return (
      <PendingRow
        key={agent.sessionId}
        icon={<HarnessLogo harness={agent.harness} />}
        name={agent.title ?? untitled}
        project={agent.project}
        disabled={isDisabled}
        onOpen={() => go(href)}
      />
    );
  });
  const runRow = (run: PendingRun) => {
    const pane = { kind: "experiment", experimentId: run.experimentId, view: "overview", runId: run.runId } as const;
    const reason = run.afterQuit === "needsApp" ? needsAppReasons[run.backend ?? ""]?.() : undefined;
    return (
      <PendingRow
        key={run.runId}
        icon={<BackendLogo kind={run.backend ?? ""} />}
        name={run.name}
        project={run.project}
        detail={reason}
        disabled={isDisabled}
        onOpen={() => go(taskLocation(run.projectId, run.sessionId, pane))}
      />
    );
  };
  const groups = [
    { heading: m.close_pending_agents(), rows: agentRows },
    { heading: m.close_pending_needs_open(), rows: runs("needsApp").map(runRow) },
    { heading: m.close_pending_continues(), rows: runs("continues").map(runRow) },
    { heading: m.close_pending_unsent(), rows: runs("reports").map(runRow) },
  ].filter((group) => group.rows.length > 0);

  return (
    <DialogContent initialFocus={blocked ? backgroundRef : quitRef} className="flex max-h-[calc(100vh-2.5rem)] flex-col">
      <DialogHeader>
        <DialogTitle>{m.close_pending_title()}</DialogTitle>
        {(work === null || blocked) && (
          <DialogDescription>{work === null ? m.close_pending_uncounted() : m.close_pending_blocked()}</DialogDescription>
        )}
      </DialogHeader>
      {groups.length > 0 && (
        <div className="-mx-3 mt-4 flex min-h-0 flex-col gap-4 overflow-y-auto px-1 pb-1">
          {groups.map((group) => (
            <section key={group.heading}>
              <h3 className="m-0 mb-1 px-2 text-xs font-medium text-subtext">{group.heading}</h3>
              <ul className="m-0 flex list-none flex-col p-0">{group.rows}</ul>
            </section>
          ))}
        </div>
      )}
      <DialogFooter>
        <Button onClick={cancel} disabled={isDisabled}>
          {m.common_cancel()}
        </Button>
        <Button ref={backgroundRef} disabled={isDisabled} onClick={() => confirm("background")}>
          {minimizes ? m.close_pending_minimize() : m.close_pending_background()}
        </Button>
        <Button ref={quitRef} variant="primary" disabled={isDisabled} onClick={() => confirm("quit")}>
          {blocked ? m.close_pending_force_quit() : m.close_pending_quit()}
        </Button>
      </DialogFooter>
    </DialogContent>
  );
}

let asking = false;

/**
 * Answers the window's `orx:close-request`: it waits for `close:asking` once the dialog shows, then for the choice.
 * Opening a listed item cancels the close and hands its path to `open`.
 */
export function askBeforeClosing({ showCustomDialog }: ReturnType<typeof createDialogs>, detail: unknown, { machine, open }: Machine) {
  if (asking) {
    window.ipc?.postMessage("close:asking");
    return;
  }
  asking = true;
  showCustomDialog({
    component: CloseDialogContent,
    props: { ...(detail as CloseRequest), machine, open },
    onConfirm: (choice) => window.ipc?.postMessage(`close:${choice}`),
    onCancel: () => window.ipc?.postMessage("close:cancel"),
  });
}
