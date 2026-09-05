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
    (value, unloading) => visit === epoch ? saveGlobalWorkspace(value, unloading) : Promise.resolve(),
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
    rememberedGlobalWorkspace = value;
    writer.queue(value, delay);
  },
};

window.addEventListener("pagehide", () => void globalWorkspaceWriter.flush(true));
