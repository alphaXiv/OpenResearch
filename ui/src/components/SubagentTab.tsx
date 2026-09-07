import { useQuery } from "@tanstack/react-query";

import { getChatMessagesQuery } from "../queries/chat";
import { m } from "../paraglide/messages.js";
import { useEffect, useLayoutEffect, useRef } from "react";
import { type ChatPart } from "../api";

import { findPartById, SubagentTranscript } from "./ChatPanel";
import type { TabOpenIntent } from "../tabPreview";
import { TabBody } from "./layout/TabBody";

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
  const query = useQuery(getChatMessagesQuery(sessionId));
  const messages = query.data?.messages ?? (query.isError ? [] : null);
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

  if (messages === null) {
    return (
      <TabBody>
        <div className={PANE_CONTENT_CLASS_NAME}>
          <div className="subagent-empty py-[3px] px-1 text-sm text-muted">{m.subagent_tab_loading()}</div>
        </div>
      </TabBody>
    );
  }

  // Locate the spawn part across all messages; its `children` are the transcript.
  let spawn: ChatPart | null = null;
  for (const m of messages) {
    spawn = findPartById(m.parts, spawnPartId);
    if (spawn) break;
  }

  return (
    <TabBody>
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
            <div className="subagent-empty py-[3px] px-1 text-sm text-muted">{m.subagent_tab_this_sub_agent_is_no_longer_available()}</div>
          )}
        </div>
      </div>
    </TabBody>
  );
}
