// The Overleaf side of a .tex tab: which Overleaf project this paper belongs to,
// and keeping the two copies in step. Split out of FileViewer for the same
// reason useLatexCompile was — the component already carries three file
// sources, an editor and five render modes.
//
// Syncing runs in both directions, so it is the file on disk that must be
// current: a sync while the editor holds unsaved edits is refused, not
// resolved, because a pull would land under a draft the user can still see.
//
// Two channels carry the same paper. The git sync is the one every linked
// paper has; the live channel — Overleaf's own editor socket, signed in with a
// session cookie — replaces its polling with edits that arrive as they are
// typed, and sends saves back the same way. The git sync stays for figures,
// for conflicts, and for whenever the live channel is down.

import { useQuery } from "@tanstack/react-query";
import {
  isCurrentScope,
  queryClient,
} from "./queries/client";

import { getOverleafStateQuery, getOverleafStatusQuery } from "./queries/files";

import { useCallback, useMemo, useEffect, useRef, useState } from "react";
import {
  importOverleafSession,
  linkOverleaf,
  overleafUploadUrl,
  saveOverleafSession,
  saveOverleafToken,
  startOverleafLive,
  stopOverleafLive,
  syncOverleaf,
  unlinkOverleaf,
  type OverleafLink,
  type OverleafLiveStatus,
  type OverleafResolution,
  type OverleafState,
  type OverleafSyncResult,
} from "./api";
import { onOverleafEvent } from "./events";

/** How often a linked paper asks whether Overleaf has moved. The question is
 * one request that transfers nothing; a clone follows only when it has. */
const POLL_MS = 30_000;

/** How often a tab tells the server it still wants its live channel. The
 * server closes one that has gone quiet for a few of these. */
const LIVE_HEARTBEAT_MS = 30_000;

/** How often a live paper still runs a git sync, for the figures and other
 * files the editor channel does not carry. */
const LIVE_FILE_SYNC_MS = 5 * 60_000;

export interface OverleafSync {
  /** A Git authentication token is stored on this machine. */
  hasToken: boolean;
  /** A browser session cookie is stored, so the live channel can open. */
  hasSession: boolean;
  /** The live channel's state, null while it has not been asked for. */
  live: OverleafLiveStatus | null;
  /** Open the live channel again after it stopped on an error. */
  retryLive: () => void;
  /** The Overleaf project this paper is linked to, null until it is linked. */
  link: OverleafLink | null;
  loaded: boolean;
  syncing: boolean;
  /** What the last sync moved, in either direction. */
  last: OverleafSyncResult | null;
  error: string | null;
  /** Unsaved edits are in the editor, so nothing may sync yet. */
  blocked: boolean;
  /** A pull replaced this file on disk with something the editor's buffer
   * could not take in, so the buffer no longer matches it. Saving would send
   * the stale draft back to Overleaf, which is why the viewer has to say so. */
  staleOnDisk: boolean;
  reloaded: () => void;
  /** The page that creates a new Overleaf project from this paper — the way in
   * for an account whose plan has no Git integration. */
  uploadUrl: string;
  saveToken: (token: string) => Promise<void>;
  /** Store the Overleaf session cookie the live channel signs in with. */
  saveSession: (session: string) => Promise<void>;
  /** Read the cookie from a signed-in browser instead of pasting it. Resolves
   * with the browser it came from, which is the only sign it worked. */
  importSession: () => Promise<string>;
  linkProject: (project: string) => Promise<void>;
  unlink: () => Promise<void>;
  sync: (resolve?: Record<string, OverleafResolution>) => void;
}

export function useOverleafSync({
  projectId,
  filePath,
  sessionId,
  enabled,
  autoRun = true,
  onManualAction,
  savedSource,
  dirty,
  onPulled,
}: {
  projectId: string;
  filePath: string;
  sessionId?: string;
  /** This is a .tex in the live checkout, so there is a file to sync. */
  enabled: boolean;
  autoRun?: boolean;
  onManualAction?: () => void;
  /** The file as it stands on disk. A push carries the file, not the compile,
   * so this — not the compiled source — is what says our side has moved: a
   * machine with no LaTeX engine never compiles, and is exactly the one this
   * feature is for. */
  savedSource: string;
  /** The editor holds unsaved edits. */
  dirty: boolean;
  /** A sync wrote these files; the viewer reloads when its own is among them. */
  onPulled: (paths: string[]) => void;
}): OverleafSync {
  const options = useMemo(() => getOverleafStateQuery(projectId, filePath, { sessionId }), [projectId, filePath, sessionId]);
  const stateQuery = useQuery({ ...options, enabled, subscribed: enabled });
  const hasToken = stateQuery.data?.hasToken ?? false;
  const hasSession = stateQuery.data?.hasSession ?? false;
  const link = stateQuery.data?.link ?? null;
  const loaded = !stateQuery.isPending;
  const [live, setLive] = useState<OverleafLiveStatus | null>(null);
  const [liveAttempt, setLiveAttempt] = useState(0);
  const [syncing, setSyncing] = useState(false);
  const [last, setLast] = useState<OverleafSyncResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [staleOnDisk, setStaleOnDisk] = useState(false);

  // The cookie is machine-wide, so it rides the same cached state the token
  // does rather than a copy of its own.
  const setSession = useCallback(
    (hasSession: boolean) => {
      if (isCurrentScope(options.queryKey)) {
        queryClient.setQueryData(options.queryKey, (state) => state && { ...state, hasSession });
      }
    },
    [options],
  );

  const apply = useCallback((state: OverleafState) => {
    if (isCurrentScope(options.queryKey)) queryClient.setQueryData(options.queryKey, state);
  }, [options]);
  useEffect(() => {
    setLast(null);
    setLiveAttempt(0);
    agreed.current = false;
    setError(null);
    setStaleOnDisk(false);
    failedRef.current = false;
  }, [enabled, projectId, filePath, sessionId]);

  // Saving answers the stale-buffer banner as well as reloading does — both
  // advance the file. Undoing back to the loaded text does not: that buffer is
  // the stale one, so clearing on `!dirty` would drop the warning while the
  // editor still showed the copy the pull replaced.
  useEffect(() => {
    setStaleOnDisk(false);
  }, [savedSource]);

  // Same StrictMode reasoning as useLatexCompile's compilingRef: a guard held
  // in state would let two syncs clone and commit over each other.
  const syncingRef = useRef(false);
  const pulledRef = useRef(onPulled);
  pulledRef.current = onPulled;
  // The server syncs the file on disk, so a draft in the editor must reach it
  // first; read at call time so the callback identity does not follow typing.
  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;
  // A sync that fails leaves Overleaf's head unrecorded, so the poll would see
  // "changed" forever and clone every thirty seconds. Wait for the user.
  const failedRef = useRef(false);

  const sync = useCallback(
    (resolve?: Record<string, OverleafResolution>) => {
      if (!hasToken || !link || syncingRef.current || dirtyRef.current) return false;
      syncingRef.current = true;
      setSyncing(true);
      setError(null);
      syncOverleaf(projectId, filePath, { sessionId, resolve })
        .then((result) => {
          failedRef.current = false;
          setLast(result);
          // The sync began on a clean file, but a clone takes seconds and the
          // user may have started typing since; reloading now would replace
          // that draft with no way back, so the choice goes to them instead.
          if (!result.pulled.includes(filePath)) return;
          if (dirtyRef.current) setStaleOnDisk(true);
          else pulledRef.current(result.pulled);
        })
        .catch((e: unknown) => {
          failedRef.current = true;
          setLast(null);
          setError(e instanceof Error ? e.message : String(e));
        })
        .finally(() => {
          syncingRef.current = false;
          setSyncing(false);
        });
      return true;
    },
    [projectId, filePath, sessionId, hasToken, link],
  );

  // Once a paper is linked it stays in step on its own: linking syncs, and so
  // does every later compile of different source. The marker only advances when
  // a sync actually started, so one refused mid-flight is not forgotten.
  const syncedMarker = useRef<string | null>(null);
  // While the live channel is up, saves travel over it and a clone per save
  // would only ask the git bridge to rate-limit us; the sync stays manual,
  // and stays so through a reconnect — a clone per network blip is the same
  // waste. Only a channel that is down hands the git sync back its polling.
  const liveActive = live?.state === "live";
  useEffect(() => {
    if (!enabled || !autoRun || !loaded || !hasToken || !link || dirty || liveActive) return;
    const marker = `${filePath}:${link.projectId}:${savedSource}`;
    if (syncedMarker.current === marker) return;
    if (sync()) syncedMarker.current = marker;
    // `syncing` is a dependency so a sync refused while another was in flight
    // is retried when that one finishes, rather than waiting for an edit.
  }, [enabled, autoRun, loaded, hasToken, link, filePath, savedSource, dirty, syncing, sync, liveActive]);

  // The live channel opens once a git sync has brought the two into step with
  // nothing left to resolve; the server starts from what that sync agreed on.
  // Once open it stays open through later syncs — the server pauses it for
  // them — rather than being rebuilt around each one.
  const agreed = useRef(false);
  if (!syncing && !!last && last.conflicts.length === 0 && !error) agreed.current = true;
  const liveReady = enabled && loaded && !!link && hasToken && hasSession && agreed.current;
  const liveKey = useRef<string | null>(null);
  // A start and the stop of the session before it share a key, so each waits
  // for the other: a stop landing second would kill the session it did not
  // mean, a start landing second would open one nothing stops.
  const settling = useRef<Promise<unknown>>(Promise.resolve());
  // A channel Overleaf refused is not asked for again until the user says so —
  // each heartbeat would otherwise be a new handshake with a bad cookie. The
  // say-so is spent on the one ask it triggers.
  const refusedRef = useRef(false);
  refusedRef.current = live?.state === "stopped" && live.error != null;
  const retryRef = useRef(false);
  useEffect(() => {
    if (!liveReady) return;
    let cancelled = false;
    const ask = () => {
      const retry = retryRef.current;
      retryRef.current = false;
      if (!retry && refusedRef.current) return;
      settling.current = settling.current.then(() => {
        if (cancelled) return;
        return startOverleafLive(projectId, filePath, { sessionId, retry })
          .then((result) => {
            if (cancelled) return;
            // A heartbeat's answer is older than any event since it; only
            // the first answer, or a stop, is news.
            if (liveKey.current === null || result.status?.state === "stopped") {
              setLive(result.status);
            }
            liveKey.current = result.key;
          })
          .catch((e: unknown) => {
            if (cancelled) return;
            setLive({
              state: "stopped",
              error: e instanceof Error ? e.message : String(e),
              // The server never answered, so nothing says the cookie is why.
              needsSession: false,
              note: null,
            });
          });
      });
    };
    ask();
    const timer = setInterval(ask, LIVE_HEARTBEAT_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
      liveKey.current = null;
      setLive(null);
      settling.current = settling.current
        .then(() => stopOverleafLive(projectId, filePath, { sessionId }))
        .catch(() => undefined);
    };
  }, [liveReady, liveAttempt, projectId, filePath, sessionId]);

  // What the channel reports: its state, and the files Overleaf just changed.
  // A pull of this file is handled exactly as a git pull is — the buffer is
  // stale if the user was typing, reloaded if not.
  useEffect(() => {
    return onOverleafEvent((ev) => {
      if (ev.key !== liveKey.current) return;
      if (ev.type === "live") {
        setLive(ev.status);
        return;
      }
      // Same care as a git pull: a draft in the editor is not replaced
      // underneath the user, it is flagged for them to settle.
      if (!ev.paths.includes(filePath)) return;
      if (dirtyRef.current) setStaleOnDisk(true);
      else pulledRef.current(ev.paths);
    });
  }, [filePath]);

  // And the other direction: ask whether Overleaf has moved, and sync when it
  // has. The marker is left alone — this is not a change on our side.
  useEffect(() => {
    if (!enabled || !autoRun || !loaded || !hasToken || !link || dirty || liveActive) return;
    const timer = setInterval(() => {
      if (syncingRef.current || failedRef.current) return;
      queryClient.fetchQuery({ ...getOverleafStatusQuery(projectId, filePath, { sessionId }), staleTime: 0 })
        .then((status) => {
          if (status.remoteChanged) sync();
        })
        .catch((e: unknown) => {
          failedRef.current = true;
          setError(e instanceof Error ? e.message : String(e));
        });
    }, POLL_MS);
    return () => clearInterval(timer);
  }, [enabled, autoRun, loaded, hasToken, link, dirty, projectId, filePath, sessionId, sync, liveActive]);

  // The live channel carries Overleaf's documents, not its figures. A slow
  // git sync alongside it keeps those moving on their own; slow because each
  // one is a clone, and the bridge rate-limits a client that asks often.
  // `sync` refuses while one is in flight or the editor is dirty.
  useEffect(() => {
    if (!enabled || !autoRun || !loaded || !hasToken || !link || !liveActive) return;
    const timer = setInterval(sync, LIVE_FILE_SYNC_MS);
    return () => clearInterval(timer);
  }, [enabled, autoRun, loaded, hasToken, link, liveActive, sync]);

  return {
    hasToken,
    hasSession,
    live,
    retryLive: () => {
      retryRef.current = true;
      setLiveAttempt((n) => n + 1);
    },
    link,
    loaded,
    syncing,
    last,
    error: error ?? stateQuery.error?.message ?? null,
    blocked: dirty,
    staleOnDisk,
    reloaded: () => setStaleOnDisk(false),
    uploadUrl: overleafUploadUrl(projectId, filePath, { sessionId }),
    saveToken: async (token: string) => {
      const result = await saveOverleafToken(token);
      syncedMarker.current = null;
      failedRef.current = false;
      setError(null);
      if (isCurrentScope(options.queryKey)) queryClient.setQueryData(options.queryKey, (state) => state && { ...state, hasToken: result.hasToken });
    },
    saveSession: async (session: string) => {
      const result = await saveOverleafSession(session, { host: link?.host });
      setSession(result.hasSession);
      // A new cookie is the answer to a refused channel; ask again with it.
      retryRef.current = true;
      setLiveAttempt((n) => n + 1);
    },
    importSession: async () => {
      const result = await importOverleafSession({ host: link?.host });
      setSession(result.hasSession);
      retryRef.current = true;
      setLiveAttempt((n) => n + 1);
      return result.source;
    },
    linkProject: async (project: string) => {
      // Another project: what the last sync agreed says nothing about it,
      // and the channel waits for the sync that starts a new agreement.
      setLast(null);
      setLive(null);
      agreed.current = false;
      syncedMarker.current = null;
      apply(await linkOverleaf(projectId, filePath, { project, sessionId }));
      onManualAction?.();
    },
    unlink: async () => {
      apply(await unlinkOverleaf(projectId, filePath, { sessionId }));
      syncedMarker.current = null;
      failedRef.current = false;
      setLast(null);
      setError(null);
    },
    sync: (resolve?: Record<string, OverleafResolution>) => {
      failedRef.current = false;
      if (sync(resolve)) {
        syncedMarker.current = `${filePath}:${link?.projectId}:${savedSource}`;
        onManualAction?.();
      }
    },
  };
}
