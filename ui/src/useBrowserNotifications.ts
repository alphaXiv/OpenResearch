import { useEffect, useSyncExternalStore } from "react";
import { useRouter } from "@tanstack/react-router";
import { getChatMessages, getChatSession, listAllChatSessions, type ChatMessage } from "./api";
import { onChatEvent, useOrxEvents, type ChatEvent } from "./events";
import { ChatCompletionTracker } from "./chatCompletion";
import { NOTIFICATIONS_CHANGED, notificationSettingsSnapshot, notificationsEnabled, newPendingPrompts, notify, resetNotificationTracking, runJustFinished } from "./notifications";
import { isCurrentScope, queryClient, workspaceScope } from "./queries/client";
import { listExperimentsQuery } from "./queries/projects";
import { taskLocation } from "./workspaceState";
import { m } from "./paraglide/messages.js";

function subscribeSettings(listener: () => void): () => void {
  window.addEventListener(NOTIFICATIONS_CHANGED, listener);
  window.addEventListener("storage", listener);
  window.addEventListener("focus", listener);
  return () => {
    window.removeEventListener(NOTIFICATIONS_CHANGED, listener);
    window.removeEventListener("storage", listener);
    window.removeEventListener("focus", listener);
  };
}

export const useNotificationSettings = () => useSyncExternalStore(subscribeSettings, notificationSettingsSnapshot);

/** A global consumer of the existing stream, independent of the selected project
 * and the paginated sidebar. It never opens a second EventSource. */
export function useBrowserNotifications(): void {
  const scope = workspaceScope();
  const router = useRouter();
  const settings = useNotificationSettings();
  const open = (href: string) => { if (isCurrentScope(scope)) void router.navigate({ href }); };
  useOrxEvents({
    onRun(run) {
      if (!isCurrentScope(scope) || !runJustFinished(run)) return;
      const experiment = queryClient.getQueryData(listExperimentsQuery(run.projectId).queryKey)?.find((row) => row.id === run.experimentId);
      const href = experiment?.chatSessionId ? taskLocation(run.projectId, experiment.chatSessionId) : `/projects/${encodeURIComponent(run.projectId)}`;
      void notify("runs", `run:${run.id}`, run.status === "done" ? m.notifications_run_done() : m.notifications_run_failed(), experiment?.title || experiment?.slug || "", () => open(href), () => isCurrentScope(scope));
    },
  });
  useEffect(() => () => resetNotificationTracking(), [scope[1]]);

  useEffect(() => {
    if (!notificationsEnabled("turns") && !notificationsEnabled("prompts")) return;
    const tracker = new ChatCompletionTracker();
    const pending = new Map<string, number>();
    const revisions = new Map<string, number>();
    const messages = new Map<string, ChatMessage>();
    const controller = new AbortController();
    let stopped = false;
    let refreshing = false;
    const buffered: ChatEvent[] = [];
    const active = () => !stopped && isCurrentScope(scope);
    const cancel = (id: string) => {
      clearTimeout(pending.get(id)); pending.delete(id);
      revisions.set(id, (revisions.get(id) ?? 0) + 1);
    };
    const schedule = (id: string, attempt = 0) => {
      clearTimeout(pending.get(id));
      pending.set(id, window.setTimeout(() => { pending.delete(id); void check(id, attempt); }, attempt ? attempt * 3000 : 350));
    };
    async function check(id: string, attempt: number) {
      const revision = revisions.get(id);
      try {
        const session = await getChatSession(id);
        if (!active() || session.busy || !session.activeLeafId || session.awaitingInput) return;
        const valid = () => {
          const latest = tracker.sessions.get(id);
          return active() && revisions.get(id) === revision && !latest?.busy && (!latest || latest.activeLeafId === session.activeLeafId);
        };
        if (!valid()) return;
        let message = messages.get(session.activeLeafId);
        if (!message) message = (await getChatMessages(id, controller.signal)).messages.find((row) => row.id === session.activeLeafId);
        // Selecting an old branch is not a newly finished reply.
        if (!message || message.role !== "assistant" || !message.completedAt || message.completedAt < (tracker.completionAfter.get(id) ?? session.createdAt)) return;
        if (message.parts.some((part) => part.type === "prompt" && part.prompt && !part.prompt.resolved)) return;
        await notify("turns", `turn:${id}:${message.id}`, m.notifications_turn_done(), session.title ?? "", () => open(taskLocation(session.projectId, id)), valid);
      } catch {
        if (active() && revisions.get(id) === revision && attempt < 3) schedule(id, attempt + 1);
      }
    }
    async function notifyPrompt(sessionId: string, message: ChatMessage) {
      const parts = newPendingPrompts(sessionId, message);
      if (!parts.length || !notificationsEnabled("prompts")) return;
      try {
        const session = tracker.sessions.get(sessionId) ?? await getChatSession(sessionId);
        for (const part of parts) {
          await notify("prompts", `prompt:${message.id}:${part.id}`, m.notifications_prompt_waiting(), session.title ?? "", () => open(taskLocation(session.projectId, session.id)), active);
        }
      } catch { /* A deleted session cannot be opened from a notification. */ }
    }
    function apply(event: ChatEvent) {
      if (!active()) return;
      if (event.type === "session") {
        if (tracker.update(event.session)) schedule(event.session.id);
      } else if (event.type === "busy") {
        if (event.busy) { tracker.busy(event.sessionId); cancel(event.sessionId); }
        else schedule(event.sessionId);
      } else if (event.type === "branch") {
        cancel(event.sessionId); tracker.branch(event.sessionId, event.activeLeafId);
      } else if (event.type === "sessionDeleted") {
        cancel(event.sessionId); tracker.sessions.delete(event.sessionId); tracker.completionAfter.delete(event.sessionId);
      }
    }
    async function refresh() {
      if (refreshing || !active()) return;
      refreshing = true;
      try {
        const sessions = await listAllChatSessions(AbortSignal.any([controller.signal, AbortSignal.timeout(12000)]));
        if (active()) for (const session of tracker.snapshot(sessions)) schedule(session.id);
      } catch { /* The reconnect event and periodic refresh retry this snapshot. */ }
      finally {
        refreshing = false;
        for (const event of buffered.splice(0)) apply(event);
      }
    }
    const off = onChatEvent((event) => {
      if (!active()) return;
      if (event.type === "reconnected") { void refresh(); return; }
      if (event.type === "message") {
        void notifyPrompt(event.sessionId, event.message);
        if (event.message.completedAt) {
          // Retain only metadata, not completed research text or tool output.
          const { id, role, createdAt, completedAt, parts } = event.message;
          messages.set(id, { id, role, createdAt, completedAt, parts: parts.filter((part) => part.type === "prompt") });
          if (messages.size > 100) messages.delete(messages.keys().next().value!);
        }
        return;
      }
      if (refreshing) buffered.push(event);
      else apply(event);
    });
    void refresh();
    const interval = window.setInterval(() => void refresh(), 30000);
    return () => {
      stopped = true; controller.abort(); off(); clearInterval(interval);
      for (const timer of pending.values()) clearTimeout(timer);
    };
  }, [scope[1], router, settings]);
}
