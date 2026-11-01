import type { ThemePreference } from "./theme";

const MAC_TITLEBAR = "__ORX_MAC_TITLEBAR__" in window;
// Windows has no native titlebar; the page draws it (WindowControls).
export const WINDOWS_TITLEBAR = "__ORX_WIN_TITLEBAR__" in window;

/** The macOS app gives its window, and so the sidebar material, this appearance. */
export function syncDesktopAppearance(preference: ThemePreference): void {
  if (MAC_TITLEBAR) window.ipc?.postMessage(`appearance:${preference}`);
}

const INTERACTIVE =
  "a, button, input, select, textarea, label, summary, [role='button'], [role='tab'], [role='menuitem'], [role='option'], [role='switch'], [role='checkbox'], [contenteditable]:not([contenteditable='false']), [tabindex]:not([tabindex='-1'])";

// The macOS and Windows apps run the page under the titlebar, so the page's top
// strip drags and zooms the window in its place.
const DRAG_STRIP_HEIGHT = MAC_TITLEBAR ? 44 : WINDOWS_TITLEBAR ? 32 : 0;
if (DRAG_STRIP_HEIGHT) {
  document.documentElement.classList.add(MAC_TITLEBAR ? "mac-titlebar" : "win-titlebar");
  // Dragging starts on the first move: the OS move loop would swallow a double-click's second press.
  let pressed = false;
  window.addEventListener("mousedown", (event) => {
    pressed = false;
    // The window controls and drag strip keep their size at every page zoom.
    const zoom = Number(document.documentElement.style.getPropertyValue("--desktop-page-zoom")) || 1;
    const height = DRAG_STRIP_HEIGHT / zoom;
    if (event.button !== 0 || event.defaultPrevented || event.clientY >= height) return;
    if (event.target instanceof Element && event.target.closest(INTERACTIVE)) return;
    if (event.detail === 2) window.ipc?.postMessage("titlebar:zoom");
    else pressed = true;
  });
  window.addEventListener("mousemove", (event) => {
    if (!pressed || event.buttons !== 1) return;
    pressed = false;
    window.ipc?.postMessage("titlebar:drag");
  });
  window.addEventListener("mouseup", () => {
    pressed = false;
  });
}
