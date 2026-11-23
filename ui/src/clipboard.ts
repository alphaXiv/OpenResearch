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

async function pngOf(image: HTMLImageElement): Promise<Blob> {
  await image.decode().catch(() => { throw new Error(m.image_copy_not_loaded()); });
  const canvas = document.createElement("canvas");
  // An SVG with only a viewBox has no natural size; it copies at its drawn size.
  canvas.width = image.naturalWidth || image.width;
  canvas.height = image.naturalHeight || image.height;
  if (!canvas.width || !canvas.height) throw new Error(m.image_copy_not_loaded());
  const context = canvas.getContext("2d");
  if (!context) throw new Error(m.image_copy_too_large());
  context.drawImage(image, 0, 0, canvas.width, canvas.height);
  return new Promise((resolve, reject) => canvas.toBlob(
    (blob) => blob ? resolve(blob) : reject(new Error(m.image_copy_too_large())),
    "image/png",
  ));
}

/** Copies only the pixels: WebKit's own Copy Image adds a local URL other apps paste instead. */
export async function copyImageToClipboard(image: HTMLImageElement) {
  if (!navigator.clipboard?.write || typeof ClipboardItem === "undefined") {
    showAlert(m.file_tree_clipboard_unavailable(), "error");
    return;
  }
  // The browser may report a failed pending blob as its own error, so keep ours; the write reports the failure.
  let cause: string | undefined;
  const png = pngOf(image).catch((error: unknown) => {
    cause = error instanceof Error ? error.message : String(error);
    throw error;
  });
  png.catch(() => {});
  try {
    // WebKit needs the write started inside the click, with a pending blob, and accepts only image/png.
    await navigator.clipboard.write([new ClipboardItem({ "image/png": png })]);
    showAlert(m.common_copied(), "success");
  } catch (error) {
    if (cause) showAlert(cause, "error");
    else showAlert(m.image_copy_failed(), "error", { description: error instanceof Error ? error.message : String(error) });
  }
}
