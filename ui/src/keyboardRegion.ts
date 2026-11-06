const MENU = "[role=menubar], [role=menu]";
const listeners = new Set<() => void>();

let lastRegion: "left" | "right" | null = null;

function elementRegion(target: Element): "left" | "right" | null {
  if (target.closest(".right-pane")) return "right";
  if (target.closest(".session-rail, .chat-pane")) return "left";
  return null;
}

window.addEventListener("pointerdown", (event) => {
  if (!(event.target instanceof Element)) return;
  if (event.target.closest(MENU)) return;
  lastRegion = elementRegion(event.target);
  for (const listener of listeners) listener();
}, true);

/** Native menu commands have no keyboard target; transcript clicks can leave focus on body. */
export function commandRegion(target: EventTarget | null = document.activeElement): "left" | "right" | null {
  if (!(target instanceof Element) || target === document.body || target.closest(MENU)) return lastRegion;
  return elementRegion(target);
}

export function subscribeCommandRegion(listener: () => void): () => void {
  listeners.add(listener);
  const changed = (event: FocusEvent) => {
    if (event.type === "focusin") {
      const target = event.target instanceof Element && event.target.closest(MENU) ? event.relatedTarget : event.target;
      lastRegion = commandRegion(target);
    }
    queueMicrotask(listener);
  };
  window.addEventListener("focusin", changed);
  window.addEventListener("focusout", changed);
  return () => {
    listeners.delete(listener);
    window.removeEventListener("focusin", changed);
    window.removeEventListener("focusout", changed);
  };
}
