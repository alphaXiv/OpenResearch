import type { OverleafResolution, OverleafSettings, OverleafState } from "../api";
import type { Workspace } from "../workspace";
import { mutations } from "./mutations";

/** The token and cookie are machine-wide, so every paper's state shows them as Settings does. */
function showCredentials(ws: Workspace, credentials: Partial<OverleafSettings>) {
  ws.setScopedQueryData(ws.queries.getOverleafSettingsQuery().queryKey, (settings) => settings && { ...settings, ...credentials });
  ws.queryClient.setQueriesData<OverleafState>(
    { queryKey: [...ws.workspaceScope(), "getOverleafState"] },
    (state) => state && { ...state, ...credentials },
  );
}

export const mutSaveOverleafToken = mutations.define({
  id: "overleaf/token-save",
  mutate: (ws: Workspace, token: string) => ws.api.saveOverleafToken(token),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess((saved) => showCredentials(ws, saved));
  },
  describe: "save the Overleaf token",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutDeleteOverleafToken = mutations.define({
  id: "overleaf/token-delete",
  mutate: (ws: Workspace) => ws.api.deleteOverleafToken(),
  optimistic({ helpers, args: [ws], onSuccess }) {
    helpers(ws.queryClient).objSet(ws.queries.getOverleafSettingsQuery(), ["hasToken"], false);
    onSuccess((saved) => showCredentials(ws, saved));
  },
  describe: "remove the Overleaf token",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSaveOverleafSession = mutations.define({
  id: "overleaf/session-save",
  mutate: (ws: Workspace, session: string, host: string | null) => ws.api.saveOverleafSession(session, { host: host ?? undefined }),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess((saved) => showCredentials(ws, saved));
  },
  describe: "save the Overleaf session",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutImportOverleafSession = mutations.define({
  id: "overleaf/session-import",
  mutate: (ws: Workspace, host: string | null) => ws.api.importOverleafSession({ host: host ?? undefined }),
  optimistic({ args: [ws], onSuccess }) {
    onSuccess(({ hasSession }) => showCredentials(ws, { hasSession }));
  },
  describe: "read the Overleaf session from a browser",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutDeleteOverleafSession = mutations.define({
  id: "overleaf/session-delete",
  mutate: (ws: Workspace) => ws.api.deleteOverleafSession(),
  optimistic({ helpers, args: [ws], onSuccess }) {
    helpers(ws.queryClient).objSet(ws.queries.getOverleafSettingsQuery(), ["hasSession"], false);
    onSuccess((saved) => showCredentials(ws, saved));
  },
  describe: "remove the Overleaf session",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutLinkOverleaf = mutations.define({
  id: "overleaf/link",
  mutate: (ws: Workspace, projectId: string, path: string, project: string, sessionId: string | null) =>
    ws.api.linkOverleaf(projectId, path, { project, sessionId: sessionId ?? undefined }),
  optimistic({ args: [ws, projectId, path, , sessionId], onSuccess }) {
    const options = ws.queries.getOverleafStateQuery(projectId, path, { sessionId: sessionId ?? undefined });
    onSuccess((state) => ws.setScopedQueryData(options.queryKey, state));
  },
  key: ({ args: [ws, projectId, path] }) => [ws.identity, "overleaf", projectId, path],
  describe: "link the Overleaf project",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutUnlinkOverleaf = mutations.define({
  id: "overleaf/unlink",
  mutate: (ws: Workspace, projectId: string, path: string, sessionId: string | null) =>
    ws.api.unlinkOverleaf(projectId, path, { sessionId: sessionId ?? undefined }),
  optimistic({ helpers, args: [ws, projectId, path, sessionId], onSuccess }) {
    const options = ws.queries.getOverleafStateQuery(projectId, path, { sessionId: sessionId ?? undefined });
    helpers(ws.queryClient).objSet(options, ["link"], null);
    onSuccess((state) => ws.setScopedQueryData(options.queryKey, state));
  },
  key: ({ args: [ws, projectId, path] }) => [ws.identity, "overleaf", projectId, path],
  describe: "unlink the Overleaf project",
  describeResult: null,
  refetchOnSuccess: false,
});

export const mutSyncOverleaf = mutations.define({
  id: "overleaf/sync",
  mutate: (ws: Workspace, projectId: string, path: string, sessionId: string | null, resolve: Record<string, OverleafResolution> | null) =>
    ws.api.syncOverleaf(projectId, path, { sessionId: sessionId ?? undefined, resolve: resolve ?? undefined }),
  optimistic: () => {},
  key: ({ args: [ws, projectId, path] }) => [ws.identity, "overleaf", projectId, path],
  describe: "sync with Overleaf",
  describeResult: null,
  refetchOnSuccess: false,
});
