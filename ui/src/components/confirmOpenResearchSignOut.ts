import type { Workspace } from "../workspace";
import { m } from "../paraglide/messages.js";

export function confirmOpenResearchSignOut({ showConfirmDialog }: Workspace["dialogs"], onConfirm: () => Promise<void>) {
  showConfirmDialog({
    title: m.settings_openresearch_sign_out_title(),
    description: m.settings_openresearch_sign_out_description(),
    confirmLabel: m.settings_openresearch_sign_out(),
    destructive: true,
    onConfirm,
    onCancel: "close",
  });
}
