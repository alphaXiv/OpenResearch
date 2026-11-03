export type DesktopClipboardPayload = {
  id: string;
  path: string;
  name: string;
  mediaType: string;
  size: number;
};

type PendingPaste = {
  resolve: (payload: DesktopClipboardPayload | null) => void;
  timeout: number;
};

const pendingPastes = new Map<string, PendingPaste>();
let nextPasteId = 0;

export function desktopClipboardBridgeAvailable(): boolean {
  return (
    typeof window !== "undefined" &&
    window.__ORX_CLIPBOARD_BRIDGE__ === true &&
    Boolean(window.ipc)
  );
}

export function installDesktopClipboardBridge(): void {
  if (!desktopClipboardBridgeAvailable() || window.__ORX_CLIPBOARD_PASTE__) {
    return;
  }
  window.__ORX_CLIPBOARD_PASTE__ = (value: unknown) => {
    if (!value || typeof value !== "object") {
      return;
    }
    const response = value as Partial<DesktopClipboardPayload> & {
      id?: unknown;
      error?: unknown;
    };
    if (typeof response.id !== "string") {
      return;
    }
    const pending = pendingPastes.get(response.id);
    if (!pending) {
      if (typeof response.path === "string") {
        deleteDesktopClipboardTemp(response.path);
      }
      return;
    }
    pendingPastes.delete(response.id);
    window.clearTimeout(pending.timeout);
    if (
      typeof response.error === "string" ||
      typeof response.path !== "string" ||
      typeof response.name !== "string" ||
      typeof response.mediaType !== "string"
    ) {
      pending.resolve(null);
      return;
    }
    pending.resolve(response as DesktopClipboardPayload);
  };
}

export function requestDesktopClipboardPaste(
  timeoutMs = 3000,
): Promise<DesktopClipboardPayload | null> {
  if (!desktopClipboardBridgeAvailable()) {
    return Promise.resolve(null);
  }
  installDesktopClipboardBridge();
  const id = String(++nextPasteId);
  return new Promise((resolve) => {
    const timeout = window.setTimeout(() => {
      pendingPastes.delete(id);
      resolve(null);
    }, timeoutMs);
    pendingPastes.set(id, { resolve, timeout });
    window.ipc?.postMessage(`clipboard-paste:${id}`);
  });
}

export function desktopClipboardFileUrl(path: string): string {
  return `/api/files/abs/raw?path=${encodeURIComponent(path)}`;
}

export function deleteDesktopClipboardTemp(path: string): void {
  if (!desktopClipboardBridgeAvailable() || !path) {
    return;
  }
  window.ipc?.postMessage(`clipboard-temp-delete:${path}`);
}

export async function fileFromDesktopClipboardPayload(
  payload: DesktopClipboardPayload,
  fetcher: typeof fetch = fetch,
): Promise<File | null> {
  try {
    const response = await fetcher(desktopClipboardFileUrl(payload.path), {
      cache: "no-store",
    });
    if (!response.ok) {
      return null;
    }
    const blob = await response.blob();
    if (blob.size === 0) {
      return null;
    }
    return new File([blob], payload.name || "pasted-image.png", {
      type: payload.mediaType || blob.type || "image/png",
      lastModified: Date.now(),
    });
  } catch {
    return null;
  } finally {
    deleteDesktopClipboardTemp(payload.path);
  }
}
