import type { Api } from "../api";
import type { WorkspaceClient } from "./client";
import type { Workspace } from "../workspace";
import { isCommit } from "./invalidation";
import { mutations } from "./mutations";
import { queryOptions, type FetchQueryOptions } from "@tanstack/react-query";
import type * as api from "../api";

type ArtifactPreviewFile = Omit<api.ProjectFile, "root">;
export type LoadedFile =
  | { source: "checkout"; file: api.ProjectFile }
  | { source: "artifact"; file: ArtifactPreviewFile; checkoutRoot?: api.CheckoutRoot }
  | { source: "absolute"; file: api.AbsoluteFile };

export function createFileQueries(client: WorkspaceClient, api: Api) {
  const { workspaceKey, queryClient } = client;

  const getRunDiffQuery = (runId: string) => queryOptions({
    queryKey: workspaceKey("getRunDiff", runId),
    queryFn: ({ signal }) => api.getRunDiff(runId, signal),
    staleTime: 30_000,
  });

  const getExperimentDiffQuery = (experimentId: string) => queryOptions({
    queryKey: workspaceKey("getExperimentDiff", experimentId),
    queryFn: ({ signal }) => api.getExperimentDiff(experimentId, signal),
    staleTime: 30_000,
  });

  const getProjectFileQuery = (projectId: string, path: string, opts: api.CheckoutRef = {}) => queryOptions({
    queryKey: workspaceKey("getProjectFile", projectId, path, { ...opts }),
    queryFn: ({ signal }) => api.getProjectFile(projectId, path, opts, signal),
    staleTime: opts.ref ? (isCommit(opts.ref) ? Infinity : 30_000) : Infinity,
    refetchOnMount: opts.ref ? true : "always",
    refetchOnWindowFocus: opts.ref ? true : "always",
  });

  const getAbsoluteFileQuery = (path: string) => queryOptions({
    queryKey: workspaceKey("getAbsoluteFile", path),
    queryFn: ({ signal }) => api.getAbsoluteFile(path, signal),
    staleTime: Infinity,
    refetchOnMount: "always",
    refetchOnWindowFocus: "always",
  });

  const getCodeTreeQuery = (projectId: string, opts: api.CheckoutRef = {}) => queryOptions({
    queryKey: workspaceKey("getCodeTree", projectId, { ...opts }),
    queryFn: ({ signal }) => api.getCodeTree(projectId, opts, signal),
    staleTime: opts.ref && isCommit(opts.ref) ? Infinity : 30_000,
  });

  const getSessionWorktreeQuery = (sessionId: string) => queryOptions({
    queryKey: workspaceKey("getSessionWorktree", sessionId),
    queryFn: ({ signal }) => api.getSessionWorktree(sessionId, signal),
    staleTime: 30_000,
  });

  const getArtifactsQuery = (projectId: string) => queryOptions({
    queryKey: workspaceKey("getArtifacts", projectId),
    queryFn: ({ signal }) => api.getArtifacts(projectId, signal),
    staleTime: 30_000,
  });

  const getArtifactFileTextQuery = (projectId: string, path: string) => queryOptions({
    queryKey: workspaceKey("getArtifactFileText", projectId, path),
    queryFn: ({ signal }) => api.getArtifactFileText(projectId, path, signal),
    staleTime: Infinity,
    refetchOnMount: "always",
    refetchOnWindowFocus: "always",
  });

  const getArtifactFileMetadataQuery = (projectId: string, path: string) => queryOptions({
    queryKey: workspaceKey("getArtifactFileMetadata", projectId, path),
    queryFn: ({ signal }) => api.getArtifactFileMetadata(projectId, path, signal),
    staleTime: 2_000,
  });

  const getLatexEngineQuery = () => queryOptions({
    queryKey: workspaceKey("getLatexEngine"),
    queryFn: ({ signal }) => api.getLatexEngine(signal),
    staleTime: 300_000,
  });

  const getOverleafSettingsQuery = () => queryOptions({
    queryKey: workspaceKey("getOverleafSettings"),
    queryFn: ({ signal }) => api.getOverleafSettings(signal),
    staleTime: 300_000,
  });

  const getOverleafStateQuery = (projectId: string, path: string, opts: { sessionId?: string } = {}) => queryOptions({
    queryKey: workspaceKey("getOverleafState", projectId, path, { ...opts }),
    queryFn: ({ signal }) => api.getOverleafState(projectId, path, opts, signal),
    staleTime: 300_000,
  });

  const getOverleafStatusQuery = (projectId: string, path: string, opts: { sessionId?: string } = {}) => queryOptions({
    queryKey: workspaceKey("getOverleafStatus", projectId, path, { ...opts }),
    queryFn: ({ signal }) => api.getOverleafStatus(projectId, path, opts, signal),
    staleTime: 30_000,
  });

  const resolvedFileQuery = (
    projectId: string,
    path: string,
    source: "repo" | "artifacts" | "abs",
    sessionId?: string,
    gitRef?: string,
  ) =>
    queryOptions({
      queryKey: workspaceKey(
        "resolvedFile",
        projectId,
        path,
        source,
        sessionId ?? null,
        gitRef ?? null,
      ),
      staleTime: gitRef && !isCommit(gitRef) ? 30_000 : Infinity,
      refetchOnMount: gitRef ? true : "always",
      refetchOnWindowFocus: gitRef ? true : "always",
      queryFn: async ({ signal, client }): Promise<LoadedFile> => {
        const read = async <T>(options: FetchQueryOptions<T, Error, T, ReturnType<typeof workspaceKey>>) => {
          signal.throwIfAborted();
          const data = await client.fetchQuery({ ...options, staleTime: gitRef ? options.staleTime : 0 });
          signal.throwIfAborted();
          return data;
        };
        const isAbsolute = source === "abs";
        const isArtifacts = source === "artifacts";
        const fromArtifacts = async (): Promise<ArtifactPreviewFile> => {
          const metadata = await read(getArtifactFileMetadataQuery(projectId, path));
          const wantsBody = metadata?.presentation === "text" || metadata?.presentation === "unknown";
          const body = metadata && wantsBody
            ? await read(getArtifactFileTextQuery(projectId, path))
            : null;
          const notFound = metadata === null || (wantsBody && body === null);
          return {
            // A missing artifact resolves to null → notFound, so it shows
            // the friendly copy rather than a raw error.
            path,
            content: body?.content ?? "",
            truncated: body?.truncated ?? false,
            binary: body?.binary ?? metadata?.presentation === "download",
            notFound,
            presentation: body
              ? body.binary
                ? "download"
                : "text"
              : (metadata?.presentation ?? "download"),
          };
        };
        // A cited artifact path arrives stripped of its `artifacts/` prefix, which
        // the checkout copy usually keeps — try that first; a throwing probe
        // (unknown session, directory name) just means "not here".
        const fromCheckout = async (): Promise<api.ProjectFile | null> => {
          for (const candidate of [`artifacts/${path}`, path]) {
            const file = await read(getProjectFileQuery(projectId, candidate, { sessionId })).catch(() => { signal.throwIfAborted(); return null; });
            if (file && !file.notFound) return file;
          }
          return null;
        };
        // Branch tabs do not fall back because a ref names a committed tree.
        const loaded: LoadedFile = await ( isAbsolute
          ? read(getAbsoluteFileQuery(path)).then((file) => ({ source: "absolute", file }))
          : isArtifacts
            ? fromArtifacts().then(async (file) => {
              if (!file.notFound) return { source: "artifact", file };
              const checkout = await fromCheckout();
              return checkout ? { source: "checkout", file: checkout } : { source: "artifact", file };
            })
            : read(getProjectFileQuery(projectId, path, { sessionId, ref: gitRef })).then((d) =>
              d.notFound && !gitRef
                ? fromArtifacts().then((f) =>
                  f.notFound
                    ? { source: "checkout", file: d }
                    : { source: "artifact", file: f, checkoutRoot: d.root },
                )
                : { source: "checkout", file: d },
            ));
        signal.throwIfAborted();
        return loaded;
      },
    });

  async function refreshFile(projectId: string, path: string, source: "repo" | "artifacts" | "abs", sessionId?: string, gitRef?: string) {
    if (isCommit(gitRef)) return;
    const options = resolvedFileQuery(projectId, path, source, sessionId, gitRef);
    const keys = [options.queryKey, ...(source === "abs" ? [getAbsoluteFileQuery(path).queryKey] : [
      getProjectFileQuery(projectId, path, { sessionId, ref: gitRef }).queryKey,
      getProjectFileQuery(projectId, `artifacts/${path}`, { sessionId }).queryKey,
      getArtifactFileTextQuery(projectId, path).queryKey,
      getArtifactFileMetadataQuery(projectId, path).queryKey,
    ])];
    await Promise.all(keys.map((queryKey) => queryClient.cancelQueries({ queryKey, exact: true })));
    await Promise.all(keys.map((queryKey) => queryClient.invalidateQueries({ queryKey, exact: true, refetchType: "none" })));
    await queryClient.invalidateQueries({ queryKey: options.queryKey, exact: true });
  }

  return {
    getRunDiffQuery,
    getExperimentDiffQuery,
    getProjectFileQuery,
    getAbsoluteFileQuery,
    getCodeTreeQuery,
    getSessionWorktreeQuery,
    getArtifactsQuery,
    getArtifactFileTextQuery,
    getArtifactFileMetadataQuery,
    getLatexEngineQuery,
    getOverleafSettingsQuery,
    getOverleafStateQuery,
    getOverleafStatusQuery,
    resolvedFileQuery,
    refreshFile,
  };
}

/** Applies `edit` to the entry at `path` in a nested artifact listing; `null` removes it. */
function editArtifact(entries: api.ArtifactEntry[], path: string, edit: (entry: api.ArtifactEntry) => api.ArtifactEntry | null): api.ArtifactEntry[] {
  return entries.flatMap((entry) => {
    if (entry.path === path) return edit(entry) ?? [];
    if (!entry.children || !path.startsWith(`${entry.path}/`)) return [entry];
    return [{ ...entry, children: editArtifact(entry.children, path, edit) }];
  });
}

function movedArtifact(entry: api.ArtifactEntry, from: string, to: string): api.ArtifactEntry {
  const path = to + entry.path.slice(from.length);
  const children = entry.children?.map((child) => movedArtifact(child, from, to));
  return { ...entry, path, ...(children && { children }) };
}

const renamedPath = (path: string, newName: string) => {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? newName : `${path.slice(0, slash + 1)}${newName}`;
};

export const mutDeleteArtifact = mutations.define({
  id: "files/artifact-delete",
  mutate: (ws: Workspace, projectId: string, path: string) => ws.api.deleteArtifact(projectId, path),
  optimistic({ helpers, args: [ws, projectId, path] }) {
    helpers(ws.queryClient).updateExisting(ws.queries.getArtifactsQuery(projectId), (artifacts) =>
      ({ ...artifacts, entries: editArtifact(artifacts.entries, path, () => null) }));
  },
  key: ({ args: [ws, projectId] }) => [ws.identity, projectId, "artifacts"],
  describe: "delete the artifact",
  describeResult: null,
  refetchOnSuccess: true,
});

export const mutManageArtifactFile = mutations.define({
  id: "files/artifact-manage",
  mutate: (ws: Workspace, projectId: string, path: string, action: api.FileAction) => ws.api.manageArtifactFile(projectId, path, action),
  optimistic({ helpers, args: [ws, projectId, path, action] }) {
    if (action.action === "duplicate") return;
    const newName = action.action === "rename" ? action.newName : null;
    const edit = (entry: api.ArtifactEntry) =>
      newName === null ? null : { ...movedArtifact(entry, path, renamedPath(path, newName)), name: newName };
    helpers(ws.queryClient).updateExisting(ws.queries.getArtifactsQuery(projectId), (artifacts) =>
      ({ ...artifacts, entries: editArtifact(artifacts.entries, path, edit) }));
  },
  key: ({ args: [ws, projectId] }) => [ws.identity, projectId, "artifacts"],
  describe: "change the artifact",
  describeResult: null,
  refetchOnSuccess: true,
});

export const mutManageProjectFile = mutations.define({
  id: "files/checkout-manage",
  mutate: (ws: Workspace, projectId: string, sessionId: string | undefined, path: string, action: api.FileAction) =>
    ws.api.manageProjectFile(projectId, path, action, { sessionId }),
  optimistic({ helpers, args: [ws, projectId, sessionId, path, action] }) {
    if (action.action === "duplicate") return;
    const inside = (entry: string) => entry === path || entry.startsWith(`${path}/`);
    const to = action.action === "rename" ? renamedPath(path, action.newName) : null;
    helpers(ws.queryClient).updateExisting(ws.queries.getCodeTreeQuery(projectId, { sessionId }), (tree) => ({
      ...tree,
      entries: to === null
        ? tree.entries.filter((entry) => !inside(entry))
        : tree.entries.map((entry) => inside(entry) ? to + entry.slice(path.length) : entry).sort(),
    }));
  },
  key: ({ args: [ws, projectId, sessionId] }) => [ws.identity, projectId, sessionId ?? ""],
  describe: "change the file",
  describeResult: null,
  refetchOnSuccess: true,
});

/** Rejects with `FileChangedError` when the file changed on disk since `expectedVersion`. */
export const mutSaveProjectFile = mutations.define({
  id: "files/checkout-save",
  mutate: (ws: Workspace, projectId: string, sessionId: string | undefined, path: string, content: string, expectedVersion: string) =>
    ws.api.saveProjectFile(projectId, path, content, { sessionId, expectedVersion }),
  optimistic({ args: [ws, projectId, sessionId, path, content], onSuccess }) {
    onSuccess(({ version }) => ws.queryClient.setQueriesData<LoadedFile>({
      queryKey: ws.workspaceKey("resolvedFile", projectId),
      predicate: (query) => query.queryKey[6] === (sessionId ?? null) && query.queryKey[7] === null,
    }, (loaded) => loaded?.source === "checkout" && loaded.file.path === path
      ? { source: "checkout", file: { ...loaded.file, content, version } }
      : loaded));
  },
  key: ({ args: [ws, projectId, sessionId, path] }) => [ws.identity, projectId, sessionId ?? "", path],
  describe: "save the file",
  describeResult: null,
  refetchOnSuccess: false,
});
