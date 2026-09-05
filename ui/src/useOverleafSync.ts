// The Overleaf side of a .tex tab: which Overleaf project this paper belongs to,
// and keeping the two copies in step. Split out of FileViewer for the same
// reason useLatexCompile was — the component already carries three file
// sources, an editor and five render modes.
//
// Syncing runs in both directions, so it is the file on disk that must be
// current: a sync while the editor holds unsaved edits is refused, not
// resolved, because a pull would land under a draft the user can still see.

import { useCallback, useEffect, useRef, useState } from "react";
import {
  getOverleafState,
  getOverleafStatus,
  linkOverleaf,
  overleafUploadUrl,
  saveOverleafToken,
  syncOverleaf,
  unlinkOverleaf,
  type OverleafLink,
  type OverleafResolution,
  type OverleafState,
  type OverleafSyncResult,
} from "./api";

/** How often a linked paper asks whether Overleaf has moved. The question is
 * one request that transfers nothing; a clone follows only when it has. */
const POLL_MS = 30_000;

export interface OverleafSync {
  /** A Git authentication token is stored on this machine. */
  hasToken: boolean;
  /** The Overleaf project this paper is linked to, null until it is linked. */
  link: OverleafLink | null;
  loaded: boolean;
  syncing: boolean;
  /** What the last sync moved, in either direction. */
  last: OverleafSyncResult | null;
  error: string | null;
  /** Unsaved edits are in the editor, so nothing may sync yet. */
  blocked: boolean;
  /** A pull replaced this file on disk while the editor held unsaved edits, so
   * the buffer no longer matches it. Saving now would send the stale draft back
   * to Overleaf, which is why the viewer has to say so. */
  staleOnDisk: boolean;
  reloaded: () => void;
  /** The page that creates a new Overleaf project from this paper — the way in
   * for an account whose plan has no Git integration. */
  uploadUrl: string;
  saveToken: (token: string) => Promise<void>;
  linkProject: (project: string) => Promise<void>;
  unlink: () => Promise<void>;
  sync: (resolve?: Record<string, OverleafResolution>) => void;
}

export function useOverleafSync({
  projectId,
  filePath,
  sessionId,
  enabled,
  savedSource,
  dirty,
  onPulled,
}: {
  projectId: string;
  filePath: string;
  sessionId?: string;
  /** This is a .tex in the live checkout, so there is a file to sync. */
  enabled: boolean;
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
  const [hasToken, setHasToken] = useState(false);
  const [link, setLink] = useState<OverleafLink | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [last, setLast] = useState<OverleafSyncResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [staleOnDisk, setStaleOnDisk] = useState(false);

  const apply = useCallback((state: OverleafState) => {
    setHasToken(state.hasToken);
    setLink(state.link);
  }, []);

  // The link does not survive a change of file: showing the previous paper's
  // link while acting on this one would unlink or sync the wrong project.
  // `hasToken` does — it is machine-wide, not this paper's.
  useEffect(() => {
    let cancelled = false;
    setLoaded(false);
    setLink(null);
    setLast(null);
    setError(null);
    setStaleOnDisk(false);
    failedRef.current = false;
    if (!enabled) return;
    getOverleafState(projectId, filePath, { sessionId })
      .then((state) => {
        if (!cancelled) apply(state);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (!cancelled) setLoaded(true);
      });
    return () => {
      cancelled = true;
    };
  }, [enabled, projectId, filePath, sessionId, apply]);

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
  useEffect(() => {
    if (!enabled || !loaded || !hasToken || !link || dirty) return;
    const marker = `${filePath}:${link.projectId}:${savedSource}`;
    if (syncedMarker.current === marker) return;
    if (sync()) syncedMarker.current = marker;
    // `syncing` is a dependency so a sync refused while another was in flight
    // is retried when that one finishes, rather than waiting for an edit.
  }, [enabled, loaded, hasToken, link, filePath, savedSource, dirty, syncing, sync]);

  // And the other direction: ask whether Overleaf has moved, and sync when it
  // has. The marker is left alone — this is not a change on our side.
  useEffect(() => {
    if (!enabled || !loaded || !hasToken || !link || dirty) return;
    const timer = setInterval(() => {
      if (syncingRef.current || failedRef.current) return;
      getOverleafStatus(projectId, filePath, { sessionId })
        .then((status) => {
          if (status.remoteChanged) sync();
        })
        .catch((e: unknown) => {
          failedRef.current = true;
          setError(e instanceof Error ? e.message : String(e));
        });
    }, POLL_MS);
    return () => clearInterval(timer);
  }, [enabled, loaded, hasToken, link, dirty, projectId, filePath, sessionId, sync]);

  return {
    hasToken,
    link,
    loaded,
    syncing,
    last,
    error,
    blocked: dirty,
    staleOnDisk,
    reloaded: () => setStaleOnDisk(false),
    uploadUrl: overleafUploadUrl(projectId, filePath, { sessionId }),
    saveToken: async (token: string) => {
      const result = await saveOverleafToken(token);
      syncedMarker.current = null;
      failedRef.current = false;
      setError(null);
      setHasToken(result.hasToken);
    },
    linkProject: async (project: string) => {
      apply(await linkOverleaf(projectId, filePath, { project, sessionId }));
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
      sync(resolve);
    },
  };
}
