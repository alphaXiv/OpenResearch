import { useEffect, type RefObject } from "react";

// Negative tabindex controls cannot be endpoints of the Tab cycle.
const FOCUSABLE = [
  "button:not([disabled])",
  "input:not([disabled])",
  "textarea:not([disabled])",
  "select:not([disabled])",
  "a[href]",
  "[tabindex]",
].map((selector) => `${selector}:not([tabindex="-1"])`).join(", ");

export function useDialogFocus(
  dialogRef: RefObject<HTMLElement | null>,
  initialSelector = "[data-initial-focus]",
) {
  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const previousFocus =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const focusable = () => [...dialog.querySelectorAll<HTMLElement>(FOCUSABLE)]
      .filter((control) => control.tabIndex >= 0 && !control.matches(":disabled") && !control.closest("[inert]")
        && control.getClientRects().length > 0 && getComputedStyle(control).visibility !== "hidden");
    const controls = focusable();
    (controls.find((control) => control.matches(initialSelector)) ?? controls[0] ?? dialog).focus();

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.key !== "Tab") return;
      const controls = focusable();
      const first = controls[0];
      const last = controls.at(-1);
      if (!first || !last) {
        event.preventDefault();
        dialog.focus();
      } else if (event.shiftKey && (document.activeElement === first || !dialog.contains(document.activeElement))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (document.activeElement === last || !dialog.contains(document.activeElement))) {
        event.preventDefault();
        first.focus();
      }
    };

    dialog.addEventListener("keydown", onKeyDown);
    return () => {
      dialog.removeEventListener("keydown", onKeyDown);
      previousFocus?.focus();
    };
  }, [dialogRef, initialSelector]);
}
