import { m } from "../paraglide/messages.js";
import { Check, ChevronDown, Circle, CircleSlash, ListChecks, LoaderCircle } from "lucide-react";
import { type ReactNode, useState } from "react";
import { fmtNumber } from "../i18n";
import { taskAllDone, type TaskItem, type TaskList } from "../taskProgress";
import type { OutlineStep } from "../turnOutline";

function TaskStatusIcon({ status, live }: { status: TaskItem["status"]; live: boolean }) {
  const props = { size: 15, strokeWidth: 1.75, "aria-hidden": true as const };
  const [icon, color] =
    status === "completed"
      ? [<Check {...props} strokeWidth={2.25} />, "text-accent-green"]
      : status === "in_progress"
        ? [<LoaderCircle {...props} className={live ? "animate-spin" : ""} />, "text-primary"]
        : status === "cancelled"
          ? [<CircleSlash {...props} />, "text-muted"]
          : [<Circle {...props} />, "text-muted"];
  return <span className={`flex h-5 w-4 shrink-0 items-center justify-center ${color}`}>{icon}</span>;
}

function TaskItems({ items, live }: { items: TaskItem[]; live: boolean }) {
  return (
    <ol className="task-items m-0 flex list-none flex-col gap-0.5 p-0">
      {items.map((item, index) => {
        const active = item.status === "in_progress";
        const label = active ? item.activeText ?? item.text : item.text;
        return (
          <li
            key={index}
            className="flex items-start gap-2 text-sm leading-5"
            aria-current={active ? "step" : undefined}
          >
            <TaskStatusIcon status={item.status} live={live} />
            <span
              className={`min-w-0 break-words ${
                item.status === "completed"
                  ? "text-subtext"
                  : item.status === "cancelled"
                    ? "text-muted line-through"
                    : active
                      ? `text-text ${live ? "tool-running-shimmer" : "font-medium"}`
                      : "text-text"
              }`}
            >
              {label}
            </span>
          </li>
        );
      })}
    </ol>
  );
}

function counts(list: TaskList): { done: string; total: string } {
  return { done: fmtNumber(list.done), total: fmtNumber(list.total) };
}

function progressLabel(list: TaskList): string {
  return taskAllDone(list) ? m.tasks_all_done() : m.tasks_progress(counts(list));
}

/** Inline transcript record of the agent's task list at the point it was last
 * updated. `live` animates the in-progress step while the turn streams. */
export function TaskListCard({ list, live }: { list: TaskList; live: boolean }) {
  return (
    <div className="task-list-card my-3.5 flex flex-col gap-2 rounded-md border border-border bg-surface py-2.5 px-3.5">
      <div className="flex items-center gap-2 text-sm">
        <ListChecks size={16} strokeWidth={1.75} className="shrink-0 text-muted" aria-hidden="true" />
        <span className="font-semibold text-text">{m.tasks_title()}</span>
        <span className="text-muted">{progressLabel(list)}</span>
      </div>
      <TaskItems items={list.items} live={live} />
    </div>
  );
}

/** Docked header shared by the strips: one line that stays put above the
 * composer, with the detail expanding beneath it. */
function DockedStrip({
  marker,
  headline,
  shimmer,
  count,
  bar,
  children,
}: {
  marker: string;
  headline: string;
  shimmer: boolean;
  count: string;
  bar?: ReactNode;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div className={`${marker} mb-2 overflow-hidden rounded-md border border-border bg-surface`}>
      <button
        type="button"
        className="flex w-full cursor-pointer items-center gap-2 py-2 px-3 text-start text-sm"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
      >
        <ListChecks size={16} strokeWidth={1.75} className="shrink-0 text-muted" aria-hidden="true" />
        <span className={`min-w-0 flex-1 truncate text-text ${shimmer ? "tool-running-shimmer" : ""}`} title={headline}>
          {headline}
        </span>
        <span className="shrink-0 tabular-nums text-muted">{count}</span>
        <span className="sr-only">{open ? m.tasks_hide_list() : m.tasks_show_list()}</span>
        <ChevronDown
          size={16}
          className={`shrink-0 text-muted transition-transform duration-120 ease-standard ${open ? "rotate-180" : ""}`}
          aria-hidden="true"
        />
      </button>
      {bar}
      {open && <div className="px-3 pt-2 pb-2.5">{children}</div>}
    </div>
  );
}

/** Docked while a turn runs: the current step and a progress bar stay in
 * view as the transcript scrolls; the full list expands on demand. */
export function TaskStrip({ list }: { list: TaskList }) {
  const headline = list.current
    ? list.current.activeText ?? list.current.text
    : taskAllDone(list)
      ? m.tasks_all_done()
      : m.tasks_title();
  const pct = list.total > 0 ? Math.round((list.done / list.total) * 100) : 0;
  return (
    <DockedStrip
      marker="task-strip"
      headline={headline}
      shimmer={list.current !== null}
      count={m.tasks_count(counts(list))}
      bar={
        <div
          className="h-0.5 w-full bg-border"
          role="progressbar"
          aria-valuenow={list.done}
          aria-valuemin={0}
          aria-valuemax={list.total}
          aria-label={m.tasks_progress(counts(list))}
        >
          <div className="h-full bg-accent-green transition-[width] duration-200 ease-standard" style={{ width: `${pct}%` }} />
        </div>
      }
    >
      <TaskItems items={list.items} live />
    </DockedStrip>
  );
}

/** Docked fallback when the agent keeps no task list: the turn's own phases,
 * read off its narration, with the tool activity each one produced. */
export function ProgressStrip({ steps, describe }: { steps: OutlineStep[]; describe: (step: OutlineStep) => string }) {
  const current = steps.at(-1);
  if (!current) return null;
  return (
    <DockedStrip
      marker="progress-strip"
      headline={current.label || describe(current) || m.chat_working()}
      shimmer
      count={m.progress_step({ count: fmtNumber(steps.length) })}
    >
      <ol className="m-0 flex list-none flex-col gap-1 p-0">
        {steps.map((step) => {
          const detail = describe(step);
          return (
            <li key={step.id} className="flex items-start gap-2 text-sm leading-5" aria-current={step.done ? undefined : "step"}>
              <TaskStatusIcon status={step.done ? "completed" : "in_progress"} live />
              <span className="min-w-0 break-words">
                <span className={step.done ? "text-subtext" : "text-text"}>{step.label || detail || m.chat_working()}</span>
                {step.label && detail && <span className="text-muted"> · {detail}</span>}
              </span>
            </li>
          );
        })}
      </ol>
    </DockedStrip>
  );
}
