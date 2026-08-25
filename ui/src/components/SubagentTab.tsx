import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { getChatMessages, type ChatMessage, type ChatPart } from "../api";
import { onChatEvent } from "../events";
import { findPartById, SubagentTranscript } from "./ChatPanel";
import { TAB_BODY_CLASS_NAME } from "../styleClasses";
import type { TabOpenIntent } from "../tabPreview";

const PANE_CONTENT_CLASS_NAME = [
  "pane-content flex-1 min-h-0 relative subagent-tab-content overflow-y-auto",
  // Same breathing room top and bottom (matches the main chat thread's pb) so
  // the transcript isn't cramped under the tab strip or flush at the end.
  "bg-background py-8 px-4",
].join(" ");

/** Right-pane tab body for a sub-agent transcript. The spawn part (and its
 * streamed `children`) lives on the parent session's chat messages, so this
 * seeds from `getChatMessages` and then follows the live `chat.message` stream —
 * the same source the inline block renders from, so it stays in sync as the
 * sub-agent works. No dedicated fetch endpoint needed. */
export function SubagentTab({
  sessionId,
  spawnPartId,
  onOpenFile,
  onOpenRun,
  runExperimentName,
  onOpenExperiment,
  experimentName,
  onOpenSubagent,
}: {
  sessionId: string;
  spawnPartId: string;
  onOpenFile?: (
    path: string,
    line: number | undefined,
    exp: string | undefined,
    ref: string | undefined,
    intent: TabOpenIntent,
  ) => void;
  onOpenRun?: (runId: string, intent: TabOpenIntent) => void;
  runExperimentName?: (runId: string) => string;
  onOpenExperiment?: (experimentId: string, intent: TabOpenIntent) => void;
  experimentName?: (experimentId: string) => string;
  onOpenSubagent?: (
    spawnPartId: string,
    label: string | undefined,
    intent: TabOpenIntent,
  ) => void;
}) {
  const [messages, setMessages] = useState<ChatMessage[] | null>(null);
  // Same stick-to-bottom contract as the main transcript: pinned on mount,
  // unpinned when the user scrolls up, re-pinned within 60px of the bottom.
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const innerRef = useRef<HTMLDivElement | null>(null);
  const stickToBottom = useRef(true);

  useLayoutEffect(() => {
    stickToBottom.current = true;
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [sessionId, spawnPartId]);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && stickToBottom.current) el.scrollTop = el.scrollHeight;
  }, [messages]);

  // Re-pin on growth without a message change — tool rows expanding, images
  // loading, the pane resizing.
  useEffect(() => {
    const el = scrollRef.current;
    const inner = innerRef.current;
    if (!el || !inner) return;
    const ro = new ResizeObserver(() => {
      if (stickToBottom.current) el.scrollTop = el.scrollHeight;
    });
    ro.observe(inner);
    ro.observe(el);
    return () => ro.disconnect();
  }, [messages === null]);

  useEffect(() => {
    let live = true;
    // Message ids a live stream frame already updated: the initial fetch can
    // resolve after newer frames and must not roll those messages back. A
    // reconnect refetch clears the set — frames lost in the outage may
    // include the terminal update, so there the fresh snapshot wins.
    const liveUpdated = new Set<string>();
    let seedGen = 0;
    const seed = () => {
      const gen = ++seedGen;
      getChatMessages(sessionId)
        .then(({ messages }) => {
          if (!live || gen !== seedGen) return;
          setMessages((prev) => {
            if (!prev) return messages;
            // Keep any message a live frame updated since this fetch began;
            // the fetch supplies history and everything else.
            const merged = messages.map((m) =>
              liveUpdated.has(m.id) ? (prev.find((p) => p.id === m.id) ?? m) : m,
            );
            const known = new Set(messages.map((m) => m.id));
            return [...merged, ...prev.filter((m) => !known.has(m.id))];
          });
        })
        .catch(() => live && setMessages((prev) => prev ?? []));
    };
    seed();
    // Live updates: replace the message the event carries (assistant turns
    // re-broadcast the whole message on every flush).
    const off = onChatEvent((ev) => {
      if (ev.type === "reconnected") {
        liveUpdated.clear();
        seed();
        return;
      }
      if (ev.type !== "message" || ev.sessionId !== sessionId) return;
      liveUpdated.add(ev.message.id);
      setMessages((prev) => {
        const next = prev ? prev.slice() : [];
        const idx = next.findIndex((m) => m.id === ev.message.id);
        if (idx === -1) next.push(ev.message);
        else next[idx] = ev.message;
        return next;
      });
    });
    return () => {
      live = false;
      off();
    };
  }, [sessionId]);

  if (messages === null) {
    return (
      <div className={TAB_BODY_CLASS_NAME}>
        <div className={PANE_CONTENT_CLASS_NAME}>
          <div className="subagent-empty py-[3px] px-1 text-md text-muted">Loading…</div>
        </div>
      </div>
    );
  }

  // Locate the spawn part across all messages; its `children` are the transcript.
  let spawn: ChatPart | null = null;
  for (const m of messages) {
    spawn = findPartById(m.parts, spawnPartId);
    if (spawn) break;
  }

  return (
    <div className={TAB_BODY_CLASS_NAME}>
      <div
        className={PANE_CONTENT_CLASS_NAME}
        ref={scrollRef}
        onScroll={(e) => {
          const el = e.currentTarget;
          stickToBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
        }}
      >
        <div ref={innerRef}>
          {spawn ? (
            <SubagentTranscript
              spawn={spawn}
              onOpenFile={onOpenFile}
              onOpenRun={onOpenRun}
              runExperimentName={runExperimentName}
              onOpenExperiment={onOpenExperiment}
              experimentName={experimentName}
              onOpenSubagent={onOpenSubagent}
            />
          ) : (
            <div className="subagent-empty py-[3px] px-1 text-md text-muted">This sub-agent is no longer available.</div>
          )}
        </div>
      </div>
    </div>
  );
}
