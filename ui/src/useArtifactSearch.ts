import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { artifactSearchClient, type ArtifactSearch, type ArtifactSearchJob } from "./api";
import { ArtifactSearchSession } from "./artifactSearch";
import { getWorkspaceGeneration, subscribeWorkspace } from "./queries/client";

export function useArtifactSearch(projectId: string, query: string) {
  const generation = useSyncExternalStore(subscribeWorkspace, getWorkspaceGeneration);
  const key = JSON.stringify([projectId, query, generation]);
  const [owner, setOwner] = useState(key);
  const [pages, setPages] = useState<ArtifactSearch[]>([]);
  const [job, setJob] = useState<ArtifactSearchJob | null>(null);
  const [error, setError] = useState<Error | null>(null);
  const session = useRef<ArtifactSearchSession | null>(null);
  const load = useRef<(after?: string) => void>(() => {});
  useEffect(() => {
    let alive = true;
    setOwner(key);
    const client = artifactSearchClient(projectId);
    load.current = (after?: string) => {
      if (!alive || !query) return;
      session.current?.dispose();
      setJob(null);
      setError(null);
      const next = new ArtifactSearchSession(client, progress => {
        if (!alive) return;
        setJob(progress);
        if (progress.status === "complete" && progress.result) {
          setPages(previous => [...previous, progress.result!]);
        }
        if (progress.status === "failed") setError(new Error(progress.error ?? "Search failed"));
      }, failure => { if (alive) setError(failure); });
      session.current = next;
      void next.start(query, after);
    };
    setPages([]);
    setJob(null);
    setError(null);
    load.current();
    return () => { alive = false; session.current?.dispose(); session.current = null; };
  }, [projectId, query, generation, key]);
  const current = owner === key;
  return {
    pages: current ? pages : [], job: current ? job : null, error: current ? error : null,
    pending: !!query && (!current || (!error && (!job || job.status === "running"))),
    resume: () => void session.current?.resume(),
    cancel: () => session.current?.cancel(),
    loadMore: () => load.current(pages.at(-1)?.nextCursor ?? undefined),
  };
}
