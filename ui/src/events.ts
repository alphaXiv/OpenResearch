// One EventSource('/api/events') for the whole app. Entity updates go to the
// caller's handlers; run.log deltas fan out through a tiny per-run emitter so
// terminals can subscribe without threading props everywhere.

import { useEffect, useRef } from "react";
import type {
  ChatMessage,
  ChatSession,
  ContextUsage,
  Experiment,
  Project,
  QueuedMessage,
  Run,
  UpdateStatus,
} from "./api";

export interface RunLogEvent {
  runId: string;
  dataBase64: string;
  offset: number;
}

type LogListener = (ev: RunLogEvent) => void;
const logListeners = new Map<string, Set<LogListener>>();

export function onRunLog(runId: string, fn: LogListener): () => void {
  let set = logListeners.get(runId);
  if (!set) {
    set = new Set();
    logListeners.set(runId, set);
  }
  set.add(fn);
  return () => {
    set.delete(fn);
    if (set.size === 0) logListeners.delete(runId);
  };
}

function emitRunLog(ev: RunLogEvent) {
  logListeners.get(ev.runId)?.forEach((fn) => fn(ev));
}

// Chat events fan out the same way so ChatPanel shares the one EventSource.
export type ChatEvent =
  | { type: "session"; session: ChatSession }
  | { type: "sessionDeleted"; sessionId: string }
  | { type: "message"; sessionId: string; message: ChatMessage }
  | { type: "busy"; sessionId: string; busy: boolean }
  | { type: "usage"; sessionId: string; usage: ContextUsage }
  | { type: "queued"; sessionId: string; items: QueuedMessage[] }
  /** A different fork of a turn is now the branch on screen. */
  | { type: "branch"; sessionId: string; activeLeafId: string | null }
  /** The EventSource re-connected after a drop. Chat events are edge-only (no
   * snapshot on connect), so frames emitted during the gap are lost for good —
   * subscribers must refetch whatever they render from chat events. */
  | { type: "reconnected" };

type ChatListener = (ev: ChatEvent) => void;
const chatListeners = new Set<ChatListener>();

export function onChatEvent(fn: ChatListener): () => void {
  chatListeners.add(fn);
  return () => {
    chatListeners.delete(fn);
  };
}

function emitChat(ev: ChatEvent) {
  chatListeners.forEach((fn) => fn(ev));
}

const projectActivityListeners = new Set<() => void>();

export function onProjectActivityEvent(fn: () => void): () => void {
  projectActivityListeners.add(fn);
  return () => {
    projectActivityListeners.delete(fn);
  };
}

function emitProjectActivityEvent() {
  projectActivityListeners.forEach((fn) => fn());
}

export interface HarnessAuthEvent {
  harness: string;
  authState: string;
}

type HarnessAuthListener = (ev: HarnessAuthEvent) => void;
const harnessAuthListeners = new Set<HarnessAuthListener>();

export function onHarnessAuth(fn: HarnessAuthListener): () => void {
  harnessAuthListeners.add(fn);
  return () => {
    harnessAuthListeners.delete(fn);
  };
}

function emitHarnessAuth(ev: HarnessAuthEvent) {
  harnessAuthListeners.forEach((fn) => fn(ev));
}

// Data-dir move progress fans out the same way so the Storage settings card can
// subscribe without touching the shared useOrxEvents handler set.
export type DataDirMoveEvent =
  | { type: "progress"; phase: string; copiedBytes: number; totalBytes: number }
  | { type: "done"; path: string; oldPathLeft?: string }
  | { type: "error"; error: string };

type DataDirMoveListener = (ev: DataDirMoveEvent) => void;
const dataDirMoveListeners = new Set<DataDirMoveListener>();

export function onDataDirMove(fn: DataDirMoveListener): () => void {
  dataDirMoveListeners.add(fn);
  return () => {
    dataDirMoveListeners.delete(fn);
  };
}

function emitDataDirMove(ev: DataDirMoveEvent) {
  dataDirMoveListeners.forEach((fn) => fn(ev));
}

// Update status fans out the same way: the restart banner and the Updates
// settings card both render it, and neither owns the other.
type UpdateStatusListener = (status: UpdateStatus) => void;
const updateStatusListeners = new Set<UpdateStatusListener>();

export function onUpdateStatus(fn: UpdateStatusListener): () => void {
  updateStatusListeners.add(fn);
  return () => {
    updateStatusListeners.delete(fn);
  };
}

function emitUpdateStatus(status: UpdateStatus) {
  updateStatusListeners.forEach((fn) => fn(status));
}

// Connection state for the whole dashboard: the one EventSource is also the one
// signal that the local server is still there.
//
// Starts connected — this page was served by that server moments ago.
let connected = true;
const connectionListeners = new Set<(connected: boolean) => void>();

export function onConnectionChange(fn: (connected: boolean) => void): () => void {
  connectionListeners.add(fn);
  return () => {
    connectionListeners.delete(fn);
  };
}

export function isConnected(): boolean {
  return connected;
}

function setConnected(next: boolean) {
  if (next === connected) return;
  connected = next;
  connectionListeners.forEach((fn) => fn(next));
}

// Chrome and Safari re-try a dropped stream after ~3s, so an outage is only
// distinguishable from a blip once a couple of those attempts have failed.
const OFFLINE_AFTER_MS = 8_000;
const REOPEN_AFTER_MS = 3_000;

export interface OrxEventHandlers {
  onRun: (run: Run) => void;
  onExperiment: (experiment: Experiment) => void;
  onProject: (project: Project) => void;
  onReconnect?: () => void;
  /** The project's artifacts changed on disk — refetch the listing. */
  onArtifacts?: (projectId: string) => void;
}

export function useOrxEvents(handlers: OrxEventHandlers) {
  // Keep the latest handlers without re-opening the stream every render.
  const ref = useRef(handlers);
  ref.current = handlers;
  useEffect(() => {
    let source: EventSource | null = null;
    let stopped = false;
    let offlineTimer: number | undefined;
    let reopenTimer: number | undefined;
    // The browser auto-reconnects a dropped EventSource and fires `open` again
    // on each re-open. Emit on any open that follows a drop — including a
    // FAILED first connect (page loaded while the backend was briefly down):
    // the initial session-list/transcript fetches likely failed too, so the
    // first successful open needs the same repair. A clean first open emits
    // nothing.
    let needsRepair = false;

    const connect = () => {
      const es = new EventSource("/api/events");
      source = es;
      es.onerror = () => {
        needsRepair = true;
        offlineTimer ??= window.setTimeout(() => setConnected(false), OFFLINE_AFTER_MS);
        // CLOSED is the browser giving up for good, which it does for any
        // non-SSE response — including the 500 the dev proxy returns while the
        // backend restarts. Nothing reopens the stream after that but us.
        if (es.readyState === EventSource.CLOSED && reopenTimer === undefined && !stopped) {
          reopenTimer = window.setTimeout(() => {
            reopenTimer = undefined;
            connect();
          }, REOPEN_AFTER_MS);
        }
      };
      es.onopen = () => {
        window.clearTimeout(offlineTimer);
        offlineTimer = undefined;
        setConnected(true);
        if (needsRepair) {
          emitChat({ type: "reconnected" });
          emitProjectActivityEvent();
          emitHarnessAuth({ harness: "*", authState: "unknown" });
          ref.current.onReconnect?.();
        }
        // Every open after this one follows a drop by definition.
        needsRepair = true;
      };
      const parse = <T>(e: MessageEvent): T | null => {
        try {
          return JSON.parse(e.data as string) as T;
        } catch {
          return null;
        }
      };
      es.addEventListener("run.updated", (e) => {
        const d = parse<{ run: Run }>(e as MessageEvent);
        if (d?.run) {
          emitProjectActivityEvent();
          ref.current.onRun(d.run);
        }
      });
      es.addEventListener("experiment.updated", (e) => {
        const d = parse<{ experiment: Experiment }>(e as MessageEvent);
        if (d?.experiment) {
          emitProjectActivityEvent();
          ref.current.onExperiment(d.experiment);
        }
      });
      es.addEventListener("project.updated", (e) => {
        const d = parse<{ project: Project }>(e as MessageEvent);
        if (d?.project) {
          emitProjectActivityEvent();
          ref.current.onProject(d.project);
        }
      });
      es.addEventListener("files.updated", (e) => {
        const d = parse<{ projectId: string }>(e as MessageEvent);
        if (d?.projectId) ref.current.onArtifacts?.(d.projectId);
      });
      es.addEventListener("run.log", (e) => {
        const d = parse<RunLogEvent>(e as MessageEvent);
        if (d?.runId) emitRunLog(d);
      });
      es.addEventListener("chat.session", (e) => {
        const d = parse<{ session: ChatSession }>(e as MessageEvent);
        if (d?.session) {
          emitProjectActivityEvent();
          emitChat({ type: "session", session: d.session });
        }
      });
      es.addEventListener("chat.session.deleted", (e) => {
        const d = parse<{ sessionId: string }>(e as MessageEvent);
        if (d?.sessionId) {
          emitProjectActivityEvent();
          emitChat({ type: "sessionDeleted", sessionId: d.sessionId });
        }
      });
      es.addEventListener("chat.message", (e) => {
        const d = parse<{ sessionId: string; message: ChatMessage }>(e as MessageEvent);
        if (d?.message) {
          emitProjectActivityEvent();
          emitChat({ type: "message", sessionId: d.sessionId, message: d.message });
        }
      });
      es.addEventListener("chat.busy", (e) => {
        const d = parse<{ sessionId: string; busy: boolean }>(e as MessageEvent);
        if (d?.sessionId) {
          emitProjectActivityEvent();
          emitChat({ type: "busy", sessionId: d.sessionId, busy: d.busy });
        }
      });
      es.addEventListener("chat.usage", (e) => {
        const d = parse<{ sessionId: string; usage: ContextUsage }>(e as MessageEvent);
        if (d?.sessionId && d.usage) emitChat({ type: "usage", sessionId: d.sessionId, usage: d.usage });
      });
      es.addEventListener("chat.queued", (e) => {
        const d = parse<{ sessionId: string; items: QueuedMessage[] }>(e as MessageEvent);
        if (d?.sessionId) emitChat({ type: "queued", sessionId: d.sessionId, items: d.items ?? [] });
      });
      es.addEventListener("chat.branch", (e) => {
        const d = parse<{ sessionId: string; activeLeafId: string | null }>(e as MessageEvent);
        if (d?.sessionId)
          emitChat({ type: "branch", sessionId: d.sessionId, activeLeafId: d.activeLeafId ?? null });
      });
      es.addEventListener("harness.auth", (e) => {
        const d = parse<HarnessAuthEvent>(e as MessageEvent);
        if (d?.harness && d.authState) emitHarnessAuth(d);
      });
      es.addEventListener("datadir.move.progress", (e) => {
        const d = parse<{ phase: string; copiedBytes: number; totalBytes: number }>(
          e as MessageEvent,
        );
        if (d) emitDataDirMove({ type: "progress", ...d });
      });
      es.addEventListener("datadir.move.done", (e) => {
        const d = parse<{ path: string; oldPathLeft?: string }>(e as MessageEvent);
        if (d) emitDataDirMove({ type: "done", path: d.path, oldPathLeft: d.oldPathLeft });
      });
      es.addEventListener("datadir.move.error", (e) => {
        const d = parse<{ error: string }>(e as MessageEvent);
        if (d) emitDataDirMove({ type: "error", error: d.error });
      });
      es.addEventListener("update.status", (e) => {
        const d = parse<UpdateStatus>(e as MessageEvent);
        if (d) emitUpdateStatus(d);
      });
    };
    connect();
    return () => {
      stopped = true;
      window.clearTimeout(offlineTimer);
      window.clearTimeout(reopenTimer);
      source?.close();
    };
  }, []);
}
