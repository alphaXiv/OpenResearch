/// <reference types="vite/client" />

interface Window {
  __ORX_MEDIA_DISABLED__?: boolean;
  /** wry's channel to the desktop app; absent in a browser. */
  ipc?: { postMessage(message: string): void };
  /** public/update-overlay.js, which index.html loads before the app. */
  orxUpdateOverlay?: {
    begin(record: { from: string; to: string; title: string; body: string }): void;
    pending(): { from: string; to: string; title: string; body: string } | null;
    show(state: { title: string; body: string; busy: boolean }, fade?: boolean): HTMLElement;
    actions(): HTMLElement | null;
    details(): HTMLElement | null;
    end(): void;
    captions(names: { minimize: string; maximize: string; restore: string; close: string }): void;
  };
}
