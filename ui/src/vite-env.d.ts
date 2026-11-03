/// <reference types="vite/client" />

interface Window {
  /** wry's channel to the desktop app; absent in a browser. */
  ipc?: { postMessage(message: string): void };
  __ORX_CLIPBOARD_BRIDGE__?: boolean;
  __ORX_CLIPBOARD_PASTE__?: (value: unknown) => void;
}
