/// <reference types="vite/client" />

interface Window {
  /** wry's channel to the desktop app; absent in a browser. */
  ipc?: { postMessage(message: string): void };
}
