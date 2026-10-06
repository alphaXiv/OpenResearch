// The macOS app gives its window the color the page shows under the titlebar
// (src/commands/app.rs), so the two read as one surface.

import type { ThemePreference } from "./theme";

const MAC_TITLEBAR = "__ORX_MAC_TITLEBAR__" in window;
// Windows has no native titlebar; the page draws it (WindowControls).
export const WINDOWS_TITLEBAR = "__ORX_WIN_TITLEBAR__" in window;

let preference: ThemePreference = "system";
let sent = "";
let pending = false;
const swatch = document
  .createElement("canvas")
  .getContext("2d", { willReadFrequently: true });

/** Any CSS color as rrggbb hex, or null when it is not fully opaque. */
function opaqueHex(color: string): string | null {
  if (!swatch) return null;
  swatch.clearRect(0, 0, 1, 1);
  // An unparseable color leaves fillStyle as it was.
  swatch.fillStyle = "transparent";
  swatch.fillStyle = color;
  swatch.fillRect(0, 0, 1, 1);
  const [r, g, b, a] = swatch.getImageData(0, 0, 1, 1).data;
  if (a !== 255) return null;
  return [r, g, b].map((v) => v.toString(16).padStart(2, "0")).join("");
}

function topEdgeColor(): string | null {
  const hits = document.elementsFromPoint(window.innerWidth / 2, 0);
  // Content scrolled under the titlebar shouldn't recolor it; match the surface it scrolls over.
  const scroller = hits.findIndex(
    (el) => el.scrollHeight > el.clientHeight && /auto|scroll/.test(getComputedStyle(el).overflowY),
  );
  for (const element of hits.slice(Math.max(scroller, 0))) {
    const hex = opaqueHex(getComputedStyle(element).backgroundColor);
    if (hex) return hex;
  }
  return opaqueHex(getComputedStyle(document.documentElement).backgroundColor);
}

/** Re-sends the titlebar color; pass the theme preference when it changes. */
export function syncDesktopTitlebar(nextPreference?: ThemePreference): void {
  if (nextPreference) preference = nextPreference;
  if (!MAC_TITLEBAR || pending) return;
  pending = true;
  setTimeout(() => {
    pending = false;
    const color = topEdgeColor();
    const message = `titlebar:${preference}:${color}`;
    if (!color || message === sent) return;
    sent = message;
    window.ipc?.postMessage(message);
  }, 100);
}

if (MAC_TITLEBAR) {
  new MutationObserver(() => syncDesktopTitlebar()).observe(
    document.documentElement,
    {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ["class", "style", "data-theme"],
    },
  );
  window.addEventListener("resize", () => syncDesktopTitlebar());
}

const INTERACTIVE =
  "a, button, input, select, textarea, label, summary, [role='button'], [role='tab'], [role='menuitem'], [role='option'], [role='switch'], [role='checkbox'], [contenteditable]:not([contenteditable='false']), [tabindex]:not([tabindex='-1'])";

// The macOS and Windows apps run the page under the titlebar, so the page's top
// strip drags and zooms the window in its place.
const DRAG_STRIP_HEIGHT = MAC_TITLEBAR ? 28 : WINDOWS_TITLEBAR ? 32 : 0;
if (DRAG_STRIP_HEIGHT) {
  document.documentElement.classList.add(MAC_TITLEBAR ? "mac-titlebar" : "win-titlebar");
  // Dragging starts on the first move: the OS move loop would swallow a double-click's second press.
  let pressed = false;
  window.addEventListener("mousedown", (event) => {
    pressed = false;
    if (event.button !== 0 || event.defaultPrevented || event.clientY >= DRAG_STRIP_HEIGHT) return;
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
