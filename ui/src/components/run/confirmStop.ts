import { autoDir } from "../../i18n";
import { m } from "../../paraglide/messages.js";
import type { Workspace } from "../../workspace";

export function confirmStop({ showConfirmDialog }: Workspace["dialogs"], name: string, stop: () => Promise<unknown> | void) {
  showConfirmDialog({
    title: m.run_stop_title({ name: autoDir(name) }),
    description: m.run_stop_effect(),
    confirmLabel: m.experiments_stop_run(),
    destructive: true,
    onConfirm: async () => {
      await stop();
    },
    onCancel: "close",
  });
}
