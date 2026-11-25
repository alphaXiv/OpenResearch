import { ArrowUpRight } from "lucide-react";
import { useEffect, useRef, useState, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";
import { paperUrl } from "../components/LitSourceLogo";
import { autoDir, fmtNumber, ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { Boundary } from "../components/ui/Boundary";
import { Tooltip } from "../components/ui/Tooltip";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";
import type { ActivityFile, ToolActivity } from "./activity";

export type OpenTranscriptFile = (
  path: string,
  line: number | undefined,
  exp: string | undefined,
  ref: string | undefined,
  intent: TabOpenIntent,
) => void;
export type OpenTranscriptTarget = (id: string, intent: TabOpenIntent) => void;

function ToolTargetOverflow({
  items,
  onOpen,
  onSelect,
  targetType,
}: {
  items: Array<{ id: string; label: string }>;
  onOpen?: OpenTranscriptTarget;
  onSelect?: (id: string) => void;
  targetType: string;
}) {
  const [open, setOpen] = useState(false);
  const revealRef = useRef<HTMLSpanElement>(null);
  const focusReveal = useRef(false);

  useEffect(() => {
    if (!open || !focusReveal.current) return;
    focusReveal.current = false;
    revealRef.current?.querySelector<HTMLButtonElement>("button")?.focus();
  }, [open]);

  return (
    <span className="tool-target-overflow inline">
      {open && (
        <span className="tool-target-reveal" ref={revealRef}>
          {items.map((item, index) => (
            <span key={item.id}>
              {index > 0 && ", "}
              {onOpen || onSelect ? (
                <button
                  className="tool-target"
                  {...(onOpen
                    ? tabOpenGestureHandlers<HTMLButtonElement>(
                      (intent) => onOpen(item.id, intent),
                      { stopPropagation: true },
                    )
                    : {
                      onClick: (event: ReactMouseEvent<HTMLButtonElement>) => {
                        event.stopPropagation();
                        onSelect?.(item.id);
                      },
                    })}
                >
                  {item.label}
                </button>
              ) : (
                <span>{item.label}</span>
              )}
            </span>
          ))}
        </span>
      )}
      {open && ", "}
      <button
        className="tool-target-more"
        aria-expanded={open}
        aria-label={open ? m.a11y_hide_additional({ target: targetType }) : m.a11y_show_more_targets({ count: fmtNumber(items.length), target: targetType })}
        onClick={(event) => {
          event.preventDefault();
          event.stopPropagation();
          focusReveal.current = !open && event.detail === 0;
          setOpen((value) => !value);
        }}
      >
        {open ? m.common_show_less() : m.common_more_count({ count: fmtNumber(items.length) })}
      </button>
    </span>
  );
}

/** The label with each file it names a link to open that file, or the whole label when it names none of them by name. */
function FileLinks({ label, files, onOpenFile }: { label: string; files: ActivityFile[]; onOpenFile: OpenTranscriptFile }) {
  const link = (file: ActivityFile, text: string, key?: number) => (
    <span
      key={key}
      className="tool-target"
      role="button"
      tabIndex={0}
      {...tabOpenGestureHandlers<HTMLSpanElement>((intent) =>
        onOpenFile(file.path, undefined, undefined, file.ref, intent),
        { stopPropagation: true })}
    >
      {text}
    </span>
  );
  const pieces: ReactNode[] = [];
  let rest = label;
  for (const [index, file] of files.entries()) {
    // Names go into labels inside isolates (`ltr`), which mark exactly where each one is.
    const name = ltr(file.name);
    const at = rest.indexOf(name);
    if (at < 0) continue;
    pieces.push(rest.slice(0, at), link(file, name, index));
    rest = rest.slice(at + name.length);
  }
  if (pieces.length === 0) return link(files[0], label);
  return <>{pieces}{rest}</>;
}

/** What else a call or group did, as a count after its label; hovering or focusing it lists each step. */
export function MoreSteps({ steps, className = "ms-1.5" }: { steps: string[]; className?: string }) {
  return (
    <Tooltip content={steps.join("\n")} className={`tool-more-steps text-muted font-normal ${className}`}>
      {m.activity_more_steps({ count: fmtNumber(steps.length) })}
    </Tooltip>
  );
}

/** A label that fails to draw reads as its plain words, and the rows around it draw as usual. */
export function ToolActivityLabel(props: Parameters<typeof RichLabel>[0]) {
  return (
    <Boundary name="work log row" rev={props.activity} fallback={props.activity.label}>
      <RichLabel {...props} />
    </Boundary>
  );
}

function RichLabel({
  activity,
  onOpenFile,
  onOpenRun,
  onOpenSpawnedSession,
  runExperimentName,
  onOpenExperiment,
  experimentName,
}: {
  activity: ToolActivity;
  onOpenFile?: OpenTranscriptFile;
  onOpenRun?: OpenTranscriptTarget;
  onOpenSpawnedSession?: (sessionId: string) => void;
  runExperimentName?: (runId: string) => string;
  onOpenExperiment?: OpenTranscriptTarget;
  experimentName?: (experimentId: string) => string;
}) {
  const more = activity.more?.length ? <MoreSteps steps={activity.more} /> : null;
  if (activity.litCall?.kind === "paper" && activity.litCall.id) {
    return (
      <a
        className="tool-target"
        href={paperUrl(activity.litCall.source, activity.litCall.id)}
        target="_blank"
        rel="noopener noreferrer"
      >
        {activity.label}
        <ArrowUpRight className="inline ms-1 opacity-50" size={14} aria-hidden="true" />
      </a>
    );
  }
  if (activity.files?.length && onOpenFile) {
    return <><FileLinks label={activity.label} files={activity.files} onOpenFile={onOpenFile} />{more}</>;
  }
  if (activity.spawnedSessionIds?.length && onOpenSpawnedSession) {
    const sessionIds = activity.spawnedSessionIds;
    const visibleSessionIds = sessionIds.slice(0, 3);
    const hiddenSessions = sessionIds.slice(visibleSessionIds.length).map((sessionId, index) => ({
      id: sessionId,
      label: m.chat_agent_number({ number: fmtNumber(visibleSessionIds.length + index + 1) }),
    }));
    return (
      <>
        {activity.label}{" — "}
        {visibleSessionIds.map((sessionId, index) => (
          <span key={sessionId}>
            {index > 0 && ", "}
            <button
              className="tool-target"
              title={m.chat_panel_open_the_session_this_agent_spawned()}
              onClick={(event) => {
                event.preventDefault();
                event.stopPropagation();
                onOpenSpawnedSession(sessionId);
              }}
            >
              {m.chat_agent_number({ number: fmtNumber(index + 1) })}
            </button>
          </span>
        ))}
        {hiddenSessions.length > 0 && (
          <>
            {", "}
            <ToolTargetOverflow
              items={hiddenSessions}
              onSelect={onOpenSpawnedSession}
              targetType={m.chat_agent_sessions()}
            />
          </>
        )}
      </>
    );
  }
  const onlyRunId = activity.runIds?.length === 1 ? activity.runIds[0] : null;
  const onlyRunName = onlyRunId && runExperimentName?.(onlyRunId);
  if (activity.runLabel && onlyRunId && onlyRunName) {
    const label = activity.runLabel(autoDir(onlyRunName));
    return (
      <>
        {onOpenRun ? (
          <button
            type="button"
            className="tool-target text-start"
            title={m.a11y_open_logs_for_run({ run: ltr(onlyRunId) })}
            {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => onOpenRun(onlyRunId, intent), { stopPropagation: true })}
          >
            {label}
          </button>
        ) : label}
        {more}
      </>
    );
  }
  const linkedTargets = (
    ids: string[],
    name: ((id: string) => string) | undefined,
    onOpen: OpenTranscriptTarget | undefined,
    title: (id: string) => string,
    targetType: string,
  ) => {
    const known = name ? ids.filter((id) => Boolean(name(id))) : ids;
    if (known.length === 0) return <>{activity.label}{more}</>;
    const visible = known.slice(0, 3);
    const hidden = known.slice(visible.length).map((id) => ({ id, label: name?.(id) || m.tree_experiment() }));
    return (
      <>
        {activity.label}{" — "}
        {visible.map((id, index) => (
          <span key={id}>
            {index > 0 && ", "}
            {onOpen ? (
              <button
                className="tool-target"
                title={title(id)}
                {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => onOpen(id, intent), { stopPropagation: true })}
              >
                {name?.(id) || m.tree_experiment()}
              </button>
            ) : (
              <span>{name?.(id) || m.tree_experiment()}</span>
            )}
          </span>
        ))}
        {hidden.length > 0 && (
          <>
            {", "}
            <ToolTargetOverflow items={hidden} onOpen={onOpen} targetType={targetType} />
          </>
        )}
        {more}
      </>
    );
  };
  if (activity.runIds?.length) {
    return linkedTargets(activity.runIds, runExperimentName, onOpenRun, (id) => m.a11y_open_logs_for_run({ run: ltr(id) }), m.chat_run_logs());
  }
  if (activity.experimentIds?.length) {
    return linkedTargets(
      activity.experimentIds,
      experimentName,
      onOpenExperiment,
      (id) => m.a11y_open_experiment({ name: experimentName?.(id) || ltr(id) }),
      m.chat_experiments(),
    );
  }
  return <>{activity.label}{more}</>;
}
