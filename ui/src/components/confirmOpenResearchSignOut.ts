import { showConfirmDialog } from "../dialog";
import { m } from "../paraglide/messages.js";

export function confirmOpenResearchSignOut(onConfirm: () => Promise<void>) {
  showConfirmDialog({
    title: m.settings_openresearch_sign_out_title(),
    description: m.settings_openresearch_sign_out_description(),
    confirmLabel: m.settings_openresearch_sign_out(),
    destructive: true,
    onConfirm,
    onCancel: "close",
  });
}
