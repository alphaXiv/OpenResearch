import { m } from "./paraglide/messages.js";
import { showAlert } from "./components/ui/Toast";

export async function copyToClipboard(text: string) {
  try {
    if (!navigator.clipboard) throw new Error(m.file_tree_clipboard_unavailable());
    await navigator.clipboard.writeText(text);
    showAlert(m.common_copied(), "success");
  } catch (error) {
    showAlert(error instanceof Error ? error.message : String(error), "error");
  }
}
