/// <reference types="vite/client" />

interface Window {
  __ORX_MEDIA_DISABLED__?: boolean;
  /** wry's channel to the desktop app; absent in a browser. */
  ipc?: { postMessage(message: string): void };
}
