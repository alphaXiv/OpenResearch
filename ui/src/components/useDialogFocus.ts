import { useEffect, useRef, type RefObject } from "react";

// `tabindex="-1"` opts an element out of Tab, so the trap must skip it too —
// otherwise its last "focusable" is one Tab never reaches and focus escapes.
const FOCUSABLE = [
  "button:not([disabled])",
  "input:not([disabled])",
  "textarea:not([disabled])",
  "[tabindex]",
].map((selector) => `${selector}:not([tabindex="-1"])`).join(", ");

export function useDialogFocus(
  dialogRef: RefObject<HTMLElement | null>,
  onClose: () => void,
  initialSelector = "[data-initial-focus]",
) {
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const previousFocus =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const focusable = () => [...dialog.querySelectorAll<HTMLElement>(FOCUSABLE)];
    (dialog.querySelector<HTMLElement>(initialSelector) ?? focusable()[0] ?? dialog).focus();

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        if (dialog.querySelector(":popover-open")) return;
        event.preventDefault();
        event.stopPropagation();
        onCloseRef.current();
        return;
      }
      if (event.key !== "Tab") return;
      const controls = focusable();
      const first = controls[0];
      const last = controls.at(-1);
      if (!first || !last) {
        event.preventDefault();
        dialog.focus();
      } else if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };

    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("keydown", onKeyDown, true);
      previousFocus?.focus();
    };
  }, [dialogRef, initialSelector]);
}
