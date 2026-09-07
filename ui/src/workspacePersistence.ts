import { queryClient } from "./queries/client";
import { getUiStateQuery } from "./queries/projects";
import { saveGlobalWorkspace } from "./api";
import { showAlert } from "./components/ui";
import { m } from "./paraglide/messages.js";
import { createWorkspaceWriter, type GlobalWorkspace } from "./workspaceState";

let rememberedGlobalWorkspace: GlobalWorkspace | null = null;
let epoch = 0;
export const getRememberedGlobalWorkspace = () => rememberedGlobalWorkspace;

function createWriter() {
  const visit = epoch;
  return createWorkspaceWriter<GlobalWorkspace>(
    async (value, unloading) => {
      if (visit !== epoch) return;
      const saved = await saveGlobalWorkspace(value, unloading);
      if (visit === epoch) queryClient.setQueryData(getUiStateQuery().queryKey, (current) => current ? { ...current, workspace: saved.workspace } : saved);
    },
    (error) => {
      if (visit !== epoch) return;
      showAlert(error instanceof Error ? error.message : String(error), "error", {
        id: "workspace-save",
        action: { label: m.app_retry(), onClick: () => void globalWorkspaceWriter.retry() },
      });
    },
  );
}
let writer = createWriter();

export function resetGlobalWorkspace() {
  epoch++;
  rememberedGlobalWorkspace = null;
  writer = createWriter();
}

export const globalWorkspaceWriter = {
  flush: (unloading = false) => writer.flush(unloading),
  retry: () => writer.retry(),
  queue(value: GlobalWorkspace, delay = 0) {
    const old = rememberedGlobalWorkspace;
    // Avoid writing the acknowledged snapshot back to the server.
    if (old && old.lastLocation === value.lastLocation && old.railOpen === value.railOpen && old.panelWidth === value.panelWidth && old.experimentsView === value.experimentsView) return;
    rememberedGlobalWorkspace = value;
    writer.queue(value, delay);
  },
};

window.addEventListener("pagehide", () => void globalWorkspaceWriter.flush(true));
