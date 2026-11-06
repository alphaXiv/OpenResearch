import type { WorkspaceQueries } from "./workspace";
import type { Api } from "./api";
import type { WorkspaceClient } from "./queries/client";

import { showAlert } from "./components/ui";
import { m } from "./paraglide/messages.js";
import { createWorkspaceWriter, type GlobalWorkspace } from "./workspaceState";

export function createGlobalPersistence(
  client: WorkspaceClient,
  api: Api,
  queries: WorkspaceQueries,
) {
  const { queryClient, workspaceScope, isCurrentScope } = client;
  const scope = workspaceScope();
  const { saveGlobalWorkspace } = api;
  const { getUiStateQuery } = queries;

  let rememberedGlobalWorkspace: GlobalWorkspace | null = null;
  const getRememberedGlobalWorkspace = () => rememberedGlobalWorkspace;

  const writer = createWorkspaceWriter<GlobalWorkspace>(
    async (value, unloading) => {
      if (!isCurrentScope(scope)) return;
      const saved = await saveGlobalWorkspace(value, unloading);
      if (isCurrentScope(scope))
        queryClient.setQueryData(getUiStateQuery().queryKey, (current) => current ? { ...current, workspace: saved.workspace } : saved);
    },
    (error) => {
      if (!isCurrentScope(scope)) return;
      showAlert(error instanceof Error ? error.message : String(error), "error", {
        id: "workspace-save",
        action: { label: m.app_retry(), onClick: () => void globalWorkspaceWriter.retry() },
      });
    },
  );

  const globalWorkspaceWriter = {
    flush: (unloading = false) => writer.flush(unloading),
    retry: () => writer.retry(),
    queue(value: GlobalWorkspace, delay = 0) {
      const old = rememberedGlobalWorkspace;
      // Avoid writing the acknowledged snapshot back to the server.
      if (old && old.lastLocation === value.lastLocation && old.railOpen === value.railOpen && old.panelWidth === value.panelWidth && old.railWidth === value.railWidth && old.experimentsView === value.experimentsView) return;
      rememberedGlobalWorkspace = value;
      writer.queue(value, delay);
    },
  };

  return { getRememberedGlobalWorkspace, globalWorkspaceWriter };
}
