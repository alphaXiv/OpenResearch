import type { KeyboardEvent } from "react";

export function navigateKeyboardList(event: KeyboardEvent<HTMLElement>) {
  if (event.defaultPrevented || event.nativeEvent.isComposing || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
  const target = event.target;
  if (!(target instanceof HTMLElement)) return;
  const item = target.hasAttribute("data-keyboard-item") ? target : null;
  if (!item && target !== event.currentTarget) return;
  const list = item?.closest<HTMLElement>("[data-keyboard-list]") ?? event.currentTarget.querySelector<HTMLElement>("[data-keyboard-list]");
  if (!list) return;
  const items = [...list.querySelectorAll<HTMLElement>("[data-keyboard-item]")].filter((element) =>
    element.getClientRects().length > 0 && !element.matches(":disabled, [aria-disabled='true']"));
  if (items.length === 0) return;
  const index = item ? items.indexOf(item) : -1;
  if (item && index < 0) return;

  if (item && (event.key === "ArrowRight" || event.key === "ArrowLeft")) {
    const path = item.dataset.keyboardPath;
    const expanded = item.getAttribute("aria-expanded");
    if (!path) return;
    if ((event.key === "ArrowRight" && expanded === "false") || (event.key === "ArrowLeft" && expanded === "true")) {
      event.preventDefault();
      item.click();
      return;
    }
    const next = event.key === "ArrowRight"
      ? items[index + 1]?.dataset.keyboardPath?.startsWith(`${path}/`) ? items[index + 1] : undefined
      : items.find((element) => element.dataset.keyboardPath === path.slice(0, path.lastIndexOf("/")));
    if (next) {
      event.preventDefault();
      next.focus();
    }
    return;
  }

  let next: number;
  if (event.key === "ArrowDown") next = item ? Math.min(index + 1, items.length - 1) : 0;
  else if (event.key === "ArrowUp") next = item ? Math.max(index - 1, 0) : items.length - 1;
  else if (event.key === "Home") next = 0;
  else if (event.key === "End") next = items.length - 1;
  else return;
  event.preventDefault();
  items[next].focus();
}

export function navigateKeyboardRow(event: KeyboardEvent<HTMLElement>) {
  if (event.defaultPrevented || event.nativeEvent.isComposing || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
  const target = event.target;
  if (!(target instanceof HTMLElement) || !target.hasAttribute("data-keyboard-horizontal-item")) return;
  const items = [...event.currentTarget.querySelectorAll<HTMLElement>("[data-keyboard-horizontal-item]")].filter((element) =>
    element.getClientRects().length > 0 && !element.matches(":disabled, [aria-disabled='true']"));
  const index = items.indexOf(target);
  if (index < 0) return;
  const direction = getComputedStyle(event.currentTarget).direction === "rtl" ? -1 : 1;
  let next: number;
  if (event.key === "ArrowRight") next = (index + direction + items.length) % items.length;
  else if (event.key === "ArrowLeft") next = (index - direction + items.length) % items.length;
  else if (event.key === "Home") next = 0;
  else if (event.key === "End") next = items.length - 1;
  else return;
  event.preventDefault();
  items[next].focus();
}
