import { useWorkspace } from "../workspace";
import { ChevronFirst, ChevronLast, ChevronLeft, ChevronRight } from "lucide-react";
import { useEffect, useRef, useState, type MouseEvent } from "react";
import { type Run } from "../api";
import { hostForRun } from "../engineHosts";
import { fmtBytes, fmtClock, ltr } from "../i18n";
import { createLogFollower, INITIAL_LOG_VIEW, isKept, LOG_WINDOW, parsePosition, shouldRecover } from "../logWindow";
import { m } from "../paraglide/messages.js";
import { runActive, runApiProvider } from "../runDisplay";
import { mountTerminal } from "./terminal";
import { Button, IconButton, StatusIndicator } from "./ui";

/**
 * One run's log, one window at a time. Logs that fit in a window load whole
 * and then follow `run.log` deltas, as before paging. Larger logs open on
 * their last window; the bar above pages by byte offset, reading pages this
 * computer did not keep from the run's host.
 */
export function LogTerminal({ run }: { run: Pick<Run, "id" | "status" | "contact" | "host" | "backend"> }) {
  const { fetchLogPage } = useWorkspace().api;
  const { onRunLog, useOrxEvents } = useWorkspace().events;
  const { useEngineHosts } = useWorkspace().queries;

  const wrapRef = useRef<HTMLDivElement>(null);
  const follower = useRef<ReturnType<typeof createLogFollower> | null>(null);
  const [view, setView] = useState(INITIAL_LOG_VIEW);
  const [editing, setEditing] = useState(false);
  const runId = run.id;

  // Counts the page host's returns to live; a failed read records the count it started under.
  const pageHost = hostForRun(useEngineHosts(), { host: view.unavailable?.host ?? run.host, backend: run.backend });
  const hostLive = pageHost?.status === "live";
  const hostEpoch = useRef(0);
  useEffect(() => {
    if (hostLive) hostEpoch.current += 1;
  }, [hostLive]);

  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    const { terminal: term, dispose } = mountTerminal(wrap, true);
    const log = createLogFollower({
      unstableMessage: m.log_generation_unstable(),
      fetchPage: (span, ident) => fetchLogPage(runId, span.start, span.end, ident),
      sink: {
        reset: () => term.reset(),
        write: (text) => term.write(text),
        // xterm snaps back to the bottom unless its viewport has synced to the new rows, which takes a frame.
        scrollToTop: () => term.write("", () => requestAnimationFrame(() => term.scrollToTop())),
      },
      publish: setView,
      epoch: () => hostEpoch.current,
    });
    follower.current = log;
    const unsubscribe = onRunLog(runId, (event) => log.onLog(event));
    log.follow();

    return () => {
      log.dispose();
      follower.current = null;
      unsubscribe();
      dispose();
    };
  }, [runId]);

  useOrxEvents({ onRun: () => {}, onReconnect: () => follower.current?.onReconnect() });

  useEffect(() => {
    if (shouldRecover(view.unavailable, hostLive, hostEpoch.current)) follower.current?.reload();
  }, [hostLive, view.unavailable]);

  const contactLost = run.contact !== null;
  const { len, kept, span, following, loading, unavailable, error } = view;
  const paged = len !== null && len > LOG_WINDOW;
  const live = runActive(run.status);

  function jumpFromBar(event: MouseEvent<HTMLButtonElement>) {
    if (len === null) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const fraction = (event.clientX - rect.left) / rect.width;
    follower.current?.goTo(Math.floor(len * fraction - LOG_WINDOW / 2));
  }

  const percent = (bytes: number) => len ? `${(bytes / len) * 100}%` : "0%";
  const atEnd = len !== null && span.end >= len;
  const provider = runApiProvider(run);
  const authMessage = provider === "k8s_job" ? m.run_notice_cluster_auth() : provider ? m.run_notice_provider_auth() : null;
  const unavailableHost = ltr(unavailable?.host ?? run.host ?? m.run_notice_the_host());
  // Failures cover only an empty view; a live view keeps its output and retries on the next delta.
  const empty = span.end === span.start;
  const overlay = !empty
    ? null
    : unavailable
      ? unavailable.contact === "needs_auth"
        ? authMessage ?? m.log_unavailable_auth({ host: unavailableHost })
        : m.log_unavailable({ host: unavailableHost, time: fmtClock(unavailable.since) })
      : error ?? (loading && paged && !isKept(kept, span) && run.host ? m.log_loading_from({ host: ltr(run.host) }) : null);

  return (
    <div className="flex h-full w-full flex-col">
      {paged && (
        <div className="shrink-0 bg-background text-xs text-subtext">
          <div className="flex items-center gap-1 border-b border-border-variant px-1.5 py-1">
            <IconButton size="small" aria-label={m.log_go_start()} data-tip={m.log_go_start()} data-tip-align="start" disabled={span.start === 0} onClick={() => follower.current?.goTo(0)}>
              <ChevronFirst size={14} />
            </IconButton>
            <IconButton size="small" aria-label={m.log_earlier()} data-tip={m.log_earlier()} disabled={span.start === 0} onClick={() => follower.current?.goTo(span.start - LOG_WINDOW)}>
              <ChevronLeft size={14} />
            </IconButton>
            {editing ? (
              <input
                autoFocus
                aria-label={m.log_go_to()}
                placeholder={m.log_go_to_placeholder()}
                className="h-6 w-32 rounded-sm border border-border bg-background px-1.5 text-xs text-text"
                onBlur={() => setEditing(false)}
                onKeyDown={(event) => {
                  if (event.key === "Escape") setEditing(false);
                  if (event.key !== "Enter") return;
                  const start = parsePosition(event.currentTarget.value, len);
                  if (start === null) return;
                  setEditing(false);
                  follower.current?.goTo(start);
                }}
              />
            ) : (
              <Button size="small" variant="ghost" className="h-6 px-1.5 text-xs tabular-nums" data-tip={m.log_go_to()} onClick={() => setEditing(true)}>
                {following ? m.log_position_end({ total: fmtBytes(len) }) : m.log_position({ start: fmtBytes(span.start), total: fmtBytes(len) })}
              </Button>
            )}
            <IconButton size="small" aria-label={m.log_later()} data-tip={m.log_later()} disabled={following || atEnd} onClick={() => follower.current?.goTo(span.end)}>
              <ChevronRight size={14} />
            </IconButton>
            <IconButton size="small" aria-label={m.log_go_end()} data-tip={m.log_go_end()} disabled={following} onClick={() => follower.current?.follow()}>
              <ChevronLast size={14} />
            </IconButton>
            <span className="flex-1" />
            {following
              ? live && (contactLost
                ? <span className="pe-1.5 text-accent-amber">{m.log_paused()}</span>
                : <StatusIndicator tone="info" live className="pe-1.5 text-xs font-normal text-subtext">{m.log_following()}</StatusIndicator>)
              : live && <Button size="small" variant="ghost" className="h-6 px-1.5 text-xs" onClick={() => follower.current?.follow()}>{m.log_follow()}</Button>}
          </div>
          <div className="px-2 py-1.5">
            <button
              type="button"
              aria-label={m.log_range()}
              className="relative block h-1.5 w-full overflow-hidden rounded-full bg-[repeating-linear-gradient(135deg,var(--color-border-variant)_0_3px,transparent_3px_6px)]"
              onClick={jumpFromBar}
            >
              {kept.map(([start, end]) => (
                <span key={start} className="absolute inset-y-0 bg-border" style={{ left: percent(start), width: percent(end - start) }} />
              ))}
              <span className="absolute inset-y-0 min-w-0.75 rounded-full bg-text" style={{ left: percent(span.start), width: percent(Math.max(span.end, span.start + LOG_WINDOW) - span.start) }} />
            </button>
          </div>
        </div>
      )}
      <div className="relative min-h-0 flex-1 py-3 px-4">
        <div ref={wrapRef} className="h-full w-full" />
        {overlay && (
          <div role="status" className="absolute inset-0 flex flex-col items-center justify-center gap-3 bg-terminal p-6 text-center text-sm text-muted">
            <p className="m-0 max-w-120">{overlay}</p>
            {unavailable && paged && (
              <div className="flex gap-2">
                <Button size="small" onClick={() => follower.current?.goTo(0)}>{m.log_go_start()}</Button>
                <Button size="small" onClick={() => follower.current?.follow()}>{m.log_go_end()}</Button>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
