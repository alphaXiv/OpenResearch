import { useQuery } from "@tanstack/react-query";

import { getChatMessagesQuery } from "../queries/chat";
import { m } from "../paraglide/messages.js";
import { useEffect, useLayoutEffect, useRef } from "react";
import { getSubagentTranscript } from "../api";
import { workspaceKey } from "../queries/client";
import { agentTranscript } from "../chatRendering";

import { SubagentTranscript } from "./ChatPanel";
import type { TabOpenIntent } from "../tabPreview";
import { ChatImageScope } from "./Md";
import { TabBody } from "./layout/TabBody";

const PANE_CONTENT_CLASS_NAME = [
  "pane-content flex-1 min-h-0 relative subagent-tab-content overflow-y-auto",
  // Same breathing room top and bottom (matches the main chat thread's pb) so
  // the transcript isn't cramped under the tab strip or flush at the end.
  "bg-background py-8 px-4",
].join(" ");

/** Resolves agent events to one transcript and follows the live message cache. */
export function SubagentTab({
  projectId,
  sessionId,
  spawnPartId,
  expansionState,
  onOpenFile,
  onOpenRun,
  runExperimentName,
  onOpenExperiment,
  experimentName,
  onOpenSubagent,
}: {
  projectId: string;
  sessionId: string;
  spawnPartId: string;
  expansionState: Map<string, boolean>;
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
  const selectedSpawn = agentTranscript(messages ?? [], spawnPartId);
  const nativeTranscript = useQuery({
    queryKey: workspaceKey("getSubagentTranscript", sessionId, spawnPartId, selectedSpawn?.state?.input?.nativeTranscript ?? null, selectedSpawn?.state?.output?.match(/\(sessionID: (ses_\w+)\)/)?.[1] ?? null),
    queryFn: ({ signal }) => getSubagentTranscript(sessionId, spawnPartId, signal),
    refetchInterval: (query) => query.state.data?.state?.status === "running" ? 1500 : false,
    refetchIntervalInBackground: true,
    refetchOnMount: "always",
  });
  const loading = messages === null || nativeTranscript.isPending;
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
  }, [messages, nativeTranscript.data]);

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
  }, [loading]);

  if (loading) {
    return (
      <TabBody>
        <div className={PANE_CONTENT_CLASS_NAME}>
          <div className="subagent-empty py-[3px] px-1 text-sm text-muted">{m.subagent_tab_loading()}</div>
        </div>
      </TabBody>
    );
  }

  const spawn = nativeTranscript.data ?? selectedSpawn;

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
            <ChatImageScope projectId={projectId} sessionId={sessionId}>
              <SubagentTranscript
                spawn={spawn}
                expansionState={expansionState}
                onOpenFile={onOpenFile}
                onOpenRun={onOpenRun}
                runExperimentName={runExperimentName}
                onOpenExperiment={onOpenExperiment}
                experimentName={experimentName}
                onOpenSubagent={onOpenSubagent}
              />
            </ChatImageScope>
          ) : (
            <div className="subagent-empty py-[3px] px-1 text-sm text-muted">{m.subagent_tab_this_sub_agent_is_no_longer_available()}</div>
          )}
        </div>
      </div>
    </TabBody>
  );
}
