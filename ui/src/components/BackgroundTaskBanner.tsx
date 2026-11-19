import { useMutate } from "@clo/react-mutation";
import { ChevronRight, CircleStop, Hourglass } from "lucide-react";
import { useEffect, useId, useState } from "react";
import { fmtDuration, fmtNumber, type ChatSession } from "../api";
import { autoDir } from "../i18n";
import { m } from "../paraglide/messages.js";
import { mutStopBackgroundTask } from "../queries/chat";
import { useWorkspace } from "../workspace";
import { Button } from "./ui";

type BackgroundTask = NonNullable<NonNullable<ChatSession["background"]>["tasks"]>[number];

function BackgroundTaskStop({ sessionId, taskId, nameId }: { sessionId: string; taskId: string; nameId?: string }) {
  const workspace = useWorkspace();
  const stop = useMutate(mutStopBackgroundTask);
  const labelId = useId();
  const error = stop.error === undefined ? null : stop.error instanceof Error ? stop.error.message : String(stop.error);
  return (
    <>
      <Button
        size="small"
        variant="ghost"
        aria-labelledby={nameId ? `${labelId} ${nameId}` : undefined}
        disabled={stop.isPending}
        onClick={() => stop.run(workspace, sessionId, taskId)}
      >
        <CircleStop size={14} aria-hidden="true" />
        <span id={labelId}>{m.common_stop()}</span>
      </Button>
      {error && <span className="w-full text-accent-red" role="alert">{error}</span>}
    </>
  );
}

const taskName = (task: BackgroundTask) => task.name || (task.chat ? m.chat_untitled() : "");

/** Tasks the agent left running when its turn ended, each with a Stop; it picks the chat back up when they finish. */
export function BackgroundTaskBanner({ sessionId, background }: { sessionId: string; background: ChatSession["background"] }) {
  const [now, setNow] = useState(Date.now);
  const nameId = useId();
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  if (!background) return null;
  const tasks = background.tasks ?? [];
  const elapsed = <span className="ms-auto shrink-0 tabular-nums text-subtext">{fmtDuration(now - background.since)}</span>;
  const icon = <Hourglass size={16} className="shrink-0 text-accent-blue" aria-hidden="true" />;
  const className = "mb-2.5 rounded-lg border border-border bg-accent-blue-subtle px-3 py-2 text-sm text-text";

  if (tasks.length > 1) {
    return (
      <details className={`${className} [&[open]_.background-chevron]:rotate-90`}>
        <summary className="flex min-h-6 cursor-pointer list-none items-center gap-2 rounded-sm focus-visible:outline-2 focus-visible:outline-focus [&::-webkit-details-marker]:hidden">
          {icon}
          <span role="status" className="font-medium">{m.chat_status_background_tasks({ count: fmtNumber(tasks.length) })}</span>
          {elapsed}
          <ChevronRight size={14} className="background-chevron shrink-0 text-subtext" aria-hidden="true" />
        </summary>
        <div className="mt-2 flex max-h-48 flex-col gap-1 overflow-y-auto">
          {tasks.map((task, index) => {
            const name = taskName(task);
            const id = `${nameId}-${index}`;
            return (
              <div key={task.id} className="flex min-w-0 flex-wrap items-center gap-2 ps-6">
                <span id={id} className="min-w-0 flex-1 truncate" title={name}>{autoDir(name)}</span>
                <BackgroundTaskStop sessionId={sessionId} taskId={task.id} nameId={name ? id : undefined} />
              </div>
            );
          })}
        </div>
      </details>
    );
  }

  const task = tasks[0];
  const name = task ? taskName(task) : "";
  return (
    <div className={`${className} flex min-w-0 flex-wrap items-center gap-2`}>
      {icon}
      <span role="status" className="shrink-0 font-medium">
        {background.count > 1 ? m.chat_status_background_tasks({ count: fmtNumber(background.count) }) : m.chat_status_background_task()}
      </span>
      {name && (
        <>
          <span aria-hidden="true">·</span>
          <span id={nameId} className="min-w-0 truncate text-subtext" title={name}>{autoDir(name)}</span>
        </>
      )}
      {elapsed}
      {task && <BackgroundTaskStop key={task.id} sessionId={sessionId} taskId={task.id} nameId={name ? nameId : undefined} />}
    </div>
  );
}
